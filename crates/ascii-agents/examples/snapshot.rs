//! Renders the TUI off-screen via ratatui's TestBackend, then converts every
//! cell into an 8x16-px tile in a PNG so we can verify the visual output
//! without needing a real terminal. Used to validate the TUI after code-review
//! fixes — see `cargo run --example snapshot --release`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use ascii_agents::tui::embedded_pack::load_sprite_pack;
use ascii_agents::tui::frame_cache::FrameCache;
use ascii_agents::tui::renderer::{draw_scene, TickerQueue};
use ascii_agents_core::source::jsonl::JsonlWatcher;
use ascii_agents_core::source::{Activity, AgentEvent};
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentId, AgentSlot, Reducer, SceneState, Transport};
use clap::Parser;
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame as GifFrame, Rgb as ImgRgb, RgbImage, Rgba, RgbaImage};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use tokio::sync::{mpsc, RwLock};

const COLS: u16 = 192;
const ROWS: u16 = 64;
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

    /// After rendering the scene normally, overlay every non-walkable
    /// pixel in semi-transparent red so the restricted zones are visible.
    /// Use this to verify that all open areas are connected (no isolated
    /// pockets that would cause an A* fallback / character teleport).
    #[arg(long)]
    debug_walkable: bool,

    /// Override the snapshot terminal width (cells). Default 192.
    #[arg(long)]
    cols: Option<u16>,

    /// Override the snapshot terminal height (cells). Default 64.
    #[arg(long)]
    rows: Option<u16>,

    /// Cap on home desks for the sample scene — lowering this forces
    /// agents past `max_desks` onto overflow seating (meeting sofas
    /// first, then floor seats). Use `--max-desks 2` to make the
    /// `working_couch` / `working_floor` poses visible from the
    /// default 7-agent scene.
    #[arg(long, default_value_t = 12)]
    max_desks: usize,

    /// Output an animated GIF instead of a static PNG. Renders
    /// `--gif-duration` seconds at `--gif-fps` frames per second,
    /// advancing the clock each frame so animations (typing bob,
    /// walking, wander cycles) play out.
    #[arg(long)]
    gif: bool,

    /// GIF duration in seconds (only with --gif).
    #[arg(long, default_value_t = 5)]
    gif_duration: u64,

    /// GIF frame rate (only with --gif). 10 fps is a good balance of
    /// smoothness vs file size (~2-5 MB for a 5s clip).
    #[arg(long, default_value_t = 10)]
    gif_fps: u64,

    /// Color theme name.
    #[arg(long, default_value = "normal")]
    theme: String,
}

fn default_projects_root() -> String {
    format!(
        "{}/.claude/projects",
        std::env::var("HOME").unwrap_or_else(|_| ".".into())
    )
}

fn main() -> Result<()> {
    let args = SnapshotArgs::parse();

    let now = SystemTime::now();
    let scene = if args.live {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        rt.block_on(capture_live_scene(&args.projects_root, args.listen_secs))?
    } else {
        sample_scene(now, args.max_desks)
    };

    let cols = args.cols.unwrap_or(COLS);
    let rows = args.rows.unwrap_or(ROWS);
    let backend = TestBackend::new(cols, rows);
    let mut term = Terminal::new(backend)?;
    let mut buf = RgbBuffer::filled(0, 0, Rgb(0, 0, 0));
    let pack = load_sprite_pack(None)?;
    let mut cache = FrameCache::new();
    let mut router = ascii_agents::tui::pathfind::AStarRouter::new();
    let mut overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
    let mut history = ascii_agents::tui::pose::PoseHistory::new();
    let theme = ascii_agents::tui::theme::theme_by_name(&args.theme)
        .unwrap_or(&ascii_agents::tui::theme::NORMAL);
    let ticker = TickerQueue::new();

    if args.gif {
        save_as_gif(
            &mut term,
            &scene,
            &pack,
            now,
            &args.out,
            cols,
            rows,
            &mut buf,
            &mut cache,
            &mut router,
            &mut overlay,
            &mut history,
            args.gif_fps,
            args.gif_duration,
            theme,
        )?;
        println!("wrote {}", args.out.display());
        return Ok(());
    }

    draw_scene(
        &mut term,
        &scene,
        &pack,
        now,
        &mut buf,
        &mut cache,
        &mut router,
        &mut overlay,
        &mut history,
        None,
        None,
        &ticker,
        theme,
        None,
    )?;

    if args.debug_walkable {
        debug_paint_walkable_overlay(&mut term, &scene)?;
    }

    save_backend_as_png(&term, &args.out, cols, rows)?;
    println!("wrote {}", args.out.display());

    println!("\n--- text preview (symbols only) ---");
    let buf = term.backend().buffer();
    for y in 0..rows {
        for x in 0..cols {
            print!("{}", buf[(x, y)].symbol());
        }
        println!();
    }
    Ok(())
}

/// Tint every non-walkable terminal cell red and print a connectedness
/// report. A non-walkable cell = either of its two half-block pixels is
/// blocked in the mask. Bright red FG = top pixel blocked; bright red BG
/// = bottom pixel blocked.
///
/// Also runs a BFS from the door threshold and prints how many walkable
/// pixels are reachable vs total — if the two numbers differ, the mask
/// has an isolated region and A* will fall back to a straight line when
/// crossing into it. That's the root cause of any remaining "闪现"
/// (character teleport) the user sees.
fn debug_paint_walkable_overlay(
    term: &mut Terminal<TestBackend>,
    scene: &SceneState,
) -> Result<()> {
    use ascii_agents::tui::layout::SceneLayout;

    let size = term.size()?;
    let scene_w = size.width;
    let scene_h = size.height.saturating_sub(1);
    let buf_w = scene_w;
    let buf_h = scene_h * 2;
    let Some(layout) = SceneLayout::compute(buf_w, buf_h, scene.max_desks) else {
        println!("(debug_walkable) layout too small to compute");
        return Ok(());
    };

    // BFS reachability from door_threshold (always inside the corridor,
    // always walkable by construction).
    let reach_mask = compute_reachable(&layout);
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut reachable = 0usize;
    let mut walkable_total = 0usize;
    let mut sample_disconnects: Vec<(u16, u16)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if layout.is_walkable(x as u16, y as u16) {
                walkable_total += 1;
                if reach_mask[y * w + x] {
                    reachable += 1;
                } else if sample_disconnects.len() < 10 {
                    sample_disconnects.push((x as u16, y as u16));
                }
            }
        }
    }
    let disconnected = walkable_total.saturating_sub(reachable);
    println!(
        "--- walkability report ---\n\
        total walkable pixels   : {walkable_total}\n\
        reachable from threshold: {reachable}\n\
        disconnected pixels     : {disconnected}{}",
        if disconnected == 0 {
            "  ✓ all open areas connected"
        } else {
            "  ⚠ disconnected components present"
        }
    );
    if !sample_disconnects.is_empty() {
        print!("sample disconnected   : ");
        for (i, (x, y)) in sample_disconnects.iter().enumerate() {
            if i > 0 {
                print!(", ");
            }
            print!("({x},{y})");
        }
        println!();
        // Probe the door-threshold neighborhood + the suspected bridge
        // pixel so we can spot which step of the chain is actually blocked.
        let probe = |x: u16, y: u16, name: &str| {
            let wk = layout.is_walkable(x, y);
            let r = is_reachable(&reach_mask, &layout, x, y);
            println!("  probe {name} ({x},{y}): walkable={wk} reachable={r}");
        };
        if let Some(t) = layout.door_threshold {
            probe(t.x, t.y, "threshold");
        }
        probe(0, layout.top_margin, "MR top-left");
        // Probe the row y=66 (pantry's last row above baseboard).
        println!("row y=66 walkability:");
        for x in 0..30u16 {
            let w = layout.is_walkable(x, 66);
            let r = is_reachable(&reach_mask, &layout, x, 66);
            println!("  x={x}: walk={w} reach={r}");
        }
    }

    // (reach_mask was computed above for the report.)

    term.draw(|f| {
        let term_buf = f.buffer_mut();
        for cy in 0..scene_h {
            for cx in 0..scene_w {
                let py_top = cy * 2;
                let py_bot = cy * 2 + 1;
                let top_walk = layout.is_walkable(cx, py_top);
                let bot_walk = layout.is_walkable(cx, py_bot);
                let top_reach = top_walk && is_reachable(&reach_mask, &layout, cx, py_top);
                let bot_reach = bot_walk && is_reachable(&reach_mask, &layout, cx, py_bot);

                let top_color = if !top_walk {
                    Some(Color::Rgb(230, 70, 70)) // obstacle = red
                } else if !top_reach {
                    Some(Color::Rgb(80, 120, 240)) // isolated = blue
                } else {
                    None
                };
                let bot_color = if !bot_walk {
                    Some(Color::Rgb(230, 70, 70))
                } else if !bot_reach {
                    Some(Color::Rgb(80, 120, 240))
                } else {
                    None
                };

                if top_color.is_none() && bot_color.is_none() {
                    continue;
                }
                let cell = &mut term_buf[(cx, cy)];
                if let Some(c) = top_color {
                    cell.fg = c;
                }
                if let Some(c) = bot_color {
                    cell.bg = c;
                }
                cell.set_symbol("▀");
            }
        }
    })?;
    Ok(())
}

fn compute_reachable(layout: &ascii_agents::tui::layout::SceneLayout) -> Vec<bool> {
    use std::collections::VecDeque;
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let mut visited = vec![false; w * h];
    let Some(start) = layout.door_threshold else {
        return visited;
    };
    if !layout.is_walkable(start.x, start.y) {
        return visited;
    }
    let (sx, sy) = (start.x as usize, start.y as usize);
    visited[sy * w + sx] = true;
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    queue.push_back((sx, sy));
    while let Some((x, y)) = queue.pop_front() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if nx >= w || ny >= h || visited[ny * w + nx] {
                continue;
            }
            if !layout.is_walkable(nx as u16, ny as u16) {
                continue;
            }
            visited[ny * w + nx] = true;
            queue.push_back((nx, ny));
        }
    }
    visited
}

fn is_reachable(
    mask: &[bool],
    layout: &ascii_agents::tui::layout::SceneLayout,
    x: u16,
    y: u16,
) -> bool {
    let w = layout.buf_w as usize;
    let h = layout.buf_h as usize;
    let (xi, yi) = (x as usize, y as usize);
    if xi >= w || yi >= h {
        return false;
    }
    mask[yi * w + xi]
}

/// BFS from `layout.door_threshold` and count visited vs total walkable
/// pixels. If the two differ, the mask has multiple connected components
/// — that's the structural cause of A*'s "no path found" fallback, which
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
            slot.label, id, slot.desk_index, slot.state
        );
    }
    watcher_handle.abort();
    Ok(snapshot)
}

fn sample_scene(now: SystemTime, max_desks: usize) -> SceneState {
    use std::time::Duration as D;
    let mut s = SceneState::new(max_desks);
    // 12-agent scene — when the buffer is small enough that not every
    // agent gets a home desk (true at the default 96×36), the trailing
    // entries fall through to overflow seating (2 meeting sofas + floor
    // seats). The Active variants at indexes 6 and 8/11 exercise the
    // working_couch / working_floor sprites; the Waiting at 7 falls
    // onto the mirrored second sofa (back_couch).
    let agents: [(&str, ActivityState, D); 12] = [
        (
            "working",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write: src/foo.rs".into()),
            },
            D::from_millis(0),
        ),
        (
            "waiting",
            ActivityState::Waiting {
                reason: "permission?".into(),
            },
            D::from_secs(10),
        ),
        ("thinking", ActivityState::Idle, D::from_secs(5)), // 5s ago — within thinking window
        ("idle-a", ActivityState::Idle, D::from_secs(300)), // 5 min — wander/sleep cycle
        ("idle-b", ActivityState::Idle, D::from_secs(301)),
        ("idle-c", ActivityState::Idle, D::from_secs(303)),
        (
            "couch-act",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_c".into()),
                detail: Some("Read: README.md".into()),
            },
            D::from_millis(140),
        ),
        (
            "couch-bk",
            ActivityState::Waiting {
                reason: "review".into(),
            },
            D::from_millis(0),
        ),
        (
            "floor-act",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_d".into()),
                detail: Some("Bash: cargo test".into()),
            },
            D::from_millis(140),
        ),
        ("floor-idle", ActivityState::Idle, D::from_millis(2_000)),
        (
            "floor-act2",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_e".into()),
                detail: Some("Grep: TODO".into()),
            },
            D::from_millis(280),
        ),
        ("floor-idle2", ActivityState::Idle, D::from_millis(3_000)),
    ];
    for (i, (key, state, age)) in agents.iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from(format!("demo-{key}").as_str()),
                cwd: std::sync::Arc::from(PathBuf::from("/demo").as_path()),
                label: std::sync::Arc::from(*key),
                state: state.clone(),
                state_started_at: now - *age,
                created_at: now - *age,
                last_event_at: now - *age,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: i,
                tool_call_count: 0,
                active_ms: 0,
            },
        );
    }
    s
}

fn save_backend_as_png(
    term: &Terminal<TestBackend>,
    path: &PathBuf,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let buf = term.backend().buffer();
    let img_w = cols as u32 * CELL_W;
    let img_h = rows as u32 * CELL_H;
    let mut img = RgbImage::new(img_w, img_h);

    for y in 0..rows {
        for x in 0..cols {
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

#[allow(clippy::too_many_arguments)]
fn save_as_gif(
    term: &mut Terminal<TestBackend>,
    scene: &SceneState,
    pack: &ascii_agents_core::sprite::format::Pack,
    start_now: SystemTime,
    path: &PathBuf,
    cols: u16,
    rows: u16,
    buf: &mut RgbBuffer,
    cache: &mut FrameCache,
    router: &mut ascii_agents::tui::pathfind::AStarRouter,
    overlay: &mut ascii_agents_core::walkable::OccupancyOverlay,
    history: &mut ascii_agents::tui::pose::PoseHistory,
    fps: u64,
    duration_secs: u64,
    theme: &ascii_agents::tui::theme::Theme,
) -> Result<()> {
    let frame_count = (duration_secs * fps) as usize;
    let frame_ms = 1000 / fps.max(1);
    let img_w = cols as u32 * CELL_W;
    let img_h = rows as u32 * CELL_H;
    let ticker = TickerQueue::new();

    let file = std::fs::File::create(path)?;
    let mut encoder = GifEncoder::new(file);
    encoder.set_repeat(Repeat::Infinite)?;

    for i in 0..frame_count {
        let now = start_now + Duration::from_millis(i as u64 * frame_ms);
        draw_scene(
            term, scene, pack, now, buf, cache, router, overlay, history, None, None, &ticker,
            theme, None,
        )?;

        let term_buf = term.backend().buffer();
        let mut rgba = RgbaImage::new(img_w, img_h);
        for y in 0..rows {
            for x in 0..cols {
                let cell = &term_buf[(x, y)];
                let symbol = cell.symbol();
                let fg = color_to_rgb(cell.fg, ImgRgb([220, 220, 220]));
                let bg = color_to_rgb(cell.bg, ImgRgb([20, 22, 28]));
                let x0 = x as u32 * CELL_W;
                let y0 = y as u32 * CELL_H;
                if symbol == "▀" {
                    fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H / 2, fg);
                    fill_rgba_rect(&mut rgba, x0, y0 + CELL_H / 2, CELL_W, CELL_H / 2, bg);
                } else if symbol.trim().is_empty() {
                    fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
                } else {
                    fill_rgba_rect(&mut rgba, x0, y0, CELL_W, CELL_H, bg);
                    let pad_x = 1;
                    let pad_y = 3;
                    fill_rgba_rect(
                        &mut rgba,
                        x0 + pad_x,
                        y0 + pad_y,
                        CELL_W - pad_x * 2,
                        CELL_H - pad_y * 2,
                        fg,
                    );
                }
            }
        }
        let delay = Delay::from_numer_denom_ms(frame_ms as u32, 1);
        let frame = GifFrame::from_parts(rgba, 0, 0, delay);
        encoder.encode_frame(frame)?;
        if (i + 1) % (fps as usize) == 0 {
            eprint!(
                "\r  encoding: {}/{}s",
                (i + 1) / fps as usize,
                duration_secs
            );
        }
    }
    eprintln!("\r  encoded {frame_count} frames @ {fps}fps");
    Ok(())
}

fn fill_rgba_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: ImgRgb<u8>) {
    let (img_w, img_h) = (img.width(), img.height());
    let rgba = Rgba([color[0], color[1], color[2], 255]);
    for j in 0..h {
        for i in 0..w {
            let px_x = x + i;
            let px_y = y + j;
            if px_x < img_w && px_y < img_h {
                img.put_pixel(px_x, px_y, rgba);
            }
        }
    }
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
