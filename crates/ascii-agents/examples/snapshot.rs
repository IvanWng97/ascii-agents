//! Renders the TUI off-screen via ratatui's TestBackend, then converts every
//! cell into an 8x16-px tile in a PNG so we can verify the visual output
//! without needing a real terminal. Used to validate the TUI after code-review
//! fixes — see `cargo run --example snapshot --release`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use ascii_agents::tui::embedded_pack::load_default_pack;
use ascii_agents::tui::frame_cache::FrameCache;
use ascii_agents::tui::renderer::draw_scene;
use ascii_agents_core::source::jsonl::JsonlWatcher;
use ascii_agents_core::source::{Activity, AgentEvent};
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentId, AgentSlot, Reducer, SceneState, Transport};
use clap::Parser;
use image::{Rgb as ImgRgb, RgbImage};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use tokio::sync::{mpsc, RwLock};

const COLS: u16 = 96;
const ROWS: u16 = 36;
const CELL_W: u32 = 8;
const CELL_H: u32 = 16;

#[derive(Debug, Parser)]
#[command(about = "Render the TUI off-screen to a PNG for verification")]
struct SnapshotArgs {
    /// Output PNG path.
    #[arg(default_value = "snapshot.png")]
    out: PathBuf,

    /// Capture real CC events by watching --projects-root for --listen-secs.
    #[arg(long)]
    live: bool,

    /// CC project root to watch (only with --live).
    #[arg(long, default_value_t = default_projects_root())]
    projects_root: String,

    /// How many seconds to listen for events (only with --live).
    #[arg(long, default_value_t = 5)]
    listen_secs: u64,
}

fn default_projects_root() -> String {
    format!(
        "{}/.claude/projects",
        std::env::var("HOME").unwrap_or_else(|_| ".".into())
    )
}

fn main() -> Result<()> {
    let args = SnapshotArgs::parse();

    let pack = load_default_pack()?;
    let now = SystemTime::now();

    let scene = if args.live {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(capture_live_scene(&args.projects_root, args.listen_secs))?
    } else {
        sample_scene(now)
    };

    let backend = TestBackend::new(COLS, ROWS);
    let mut term = Terminal::new(backend)?;
    let mut buf = RgbBuffer::filled(0, 0, Rgb(0, 0, 0));
    let mut cache = FrameCache::new();
    draw_scene(&mut term, &scene, &pack, now, &mut buf, &mut cache)?;

    save_backend_as_png(&term, &args.out)?;
    println!("wrote {}", args.out.display());

    // Also dump a text-only preview so you can eyeball without an image viewer.
    println!("\n--- text preview (symbols only) ---");
    let buf = term.backend().buffer();
    for y in 0..ROWS {
        for x in 0..COLS {
            print!("{}", buf[(x, y)].symbol());
        }
        println!();
    }
    Ok(())
}

async fn capture_live_scene(projects_root: &str, listen_secs: u64) -> Result<SceneState> {
    println!(
        "listening for real CC events under {} for {}s...",
        projects_root, listen_secs
    );
    let scene: Arc<RwLock<SceneState>> = Arc::new(RwLock::new(SceneState::new(12)));
    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(1024);
    let root = PathBuf::from(projects_root);
    let watcher = JsonlWatcher::new(root);
    let watcher_handle = tokio::spawn(async move { watcher.run(tx).await });

    let mut reducer = Reducer::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(listen_secs);
    let mut event_count: u64 = 0;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some((transport, ev))) => {
                let now = SystemTime::now();
                let mut s = scene.write().await;
                reducer.apply(&mut s, ev, now, transport);
                event_count += 1;
            }
            _ => break,
        }
    }
    let snapshot = scene.read().await.clone();
    println!(
        "captured {} events; final scene has {} agents",
        event_count,
        snapshot.agents.len()
    );
    for (id, slot) in &snapshot.agents {
        println!(
            "  {} ({}) at desk {}: {:?}",
            slot.label,
            id,
            slot.desk_index,
            slot.state
        );
    }
    watcher_handle.abort();
    Ok(snapshot)
}

fn sample_scene(now: SystemTime) -> SceneState {
    let mut s = SceneState::new(12);
    let agents = [
        // 'state_started_at' is set to `now - offset` so we can place each
        // agent at different points in the wander cycle.
        ("seated", ActivityState::Idle, Duration::from_millis(10_000)),
        (
            "typing-rs",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write: src/foo.rs".into()),
            },
            Duration::from_millis(60),
        ),
        // Mid walk-out — should be partly toward wander spot.
        ("walk-out", ActivityState::Idle, Duration::from_millis(1200)),
        (
            "waiting",
            ActivityState::Waiting {
                reason: "permission?".into(),
            },
            Duration::from_millis(0),
        ),
        // Standing at the wander spot.
        ("at-spot", ActivityState::Idle, Duration::from_millis(3000)),
        (
            "typing-py",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_b".into()),
                detail: Some("Edit: main.py".into()),
            },
            Duration::from_millis(280),
        ),
        // Walking back to seat.
        ("walk-back", ActivityState::Idle, Duration::from_millis(5000)),
    ];
    for (i, (key, state, offset)) in agents.iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: "claude-code".into(),
                session_id: format!("demo-{key}"),
                cwd: PathBuf::from("/demo"),
                label: format!("cc#{}", i + 1),
                state: state.clone(),
                state_started_at: now - *offset,
                desk_index: i,
            },
        );
    }
    s
}

fn save_backend_as_png(term: &Terminal<TestBackend>, path: &PathBuf) -> Result<()> {
    let buf = term.backend().buffer();
    let img_w = COLS as u32 * CELL_W;
    let img_h = ROWS as u32 * CELL_H;
    let mut img = RgbImage::new(img_w, img_h);

    for y in 0..ROWS {
        for x in 0..COLS {
            let cell = &buf[(x, y)];
            let symbol = cell.symbol();
            let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
            let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));

            // For the half-block character "▀", the cell is split: top half = fg, bottom half = bg.
            // For other characters, we approximate by drawing the cell as one bg-color tile and
            // overlaying a roughly-centered fg-color glyph rectangle.
            let x0 = x as u32 * CELL_W;
            let y0 = y as u32 * CELL_H;

            if symbol == "▀" {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H / 2, fg);
                fill_rect(&mut img, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, bg);
            } else if symbol.trim().is_empty() {
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
            } else {
                // Background, then a small fg square in the middle to represent text.
                fill_rect(&mut img, x0, y0, CELL_W, CELL_H, bg);
                // Tiny glyph box — gives a visible indication of where text lives.
                let pad_x = 1;
                let pad_y = 3;
                fill_rect(
                    &mut img,
                    x0 + pad_x,
                    y0 + pad_y,
                    CELL_W - pad_x * 2,
                    CELL_H - pad_y * 2,
                    fg,
                );
            }
        }
    }

    img.save(path)?;
    Ok(())
}

fn fill_rect(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: ImgRgb<u8>) {
    let (img_w, img_h) = (img.width(), img.height());
    for j in 0..h {
        for i in 0..w {
            let px_x = x + i;
            let px_y = y + j;
            if px_x < img_w && px_y < img_h {
                img.put_pixel(px_x, px_y, color);
            }
        }
    }
}

fn color_to_rgb(c: Color, default: ImgRgb<u8>) -> ImgRgb<u8> {
    match c {
        Color::Rgb(r, g, b) => ImgRgb([r, g, b]),
        Color::Black => ImgRgb([0, 0, 0]),
        Color::Red => ImgRgb([180, 50, 50]),
        Color::Green => ImgRgb([60, 180, 60]),
        Color::Yellow => ImgRgb([220, 200, 50]),
        Color::Blue => ImgRgb([60, 120, 220]),
        Color::Magenta => ImgRgb([200, 60, 200]),
        Color::Cyan => ImgRgb([50, 200, 220]),
        Color::Gray => ImgRgb([160, 160, 160]),
        Color::DarkGray => ImgRgb([80, 80, 80]),
        Color::White => ImgRgb([240, 240, 240]),
        Color::LightRed => ImgRgb([230, 100, 100]),
        Color::LightGreen => ImgRgb([100, 230, 100]),
        Color::LightYellow => ImgRgb([240, 230, 100]),
        Color::LightBlue => ImgRgb([130, 180, 250]),
        Color::LightMagenta => ImgRgb([240, 130, 240]),
        Color::LightCyan => ImgRgb([130, 240, 240]),
        Color::Indexed(_) | Color::Reset => default,
    }
}
