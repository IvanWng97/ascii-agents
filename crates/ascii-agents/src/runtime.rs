use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ascii_agents_core::state::MAX_FLOORS;

use anyhow::Result;
use ascii_agents_core::source::antigravity::AntigravitySource;
use ascii_agents_core::source::claude_code::ClaudeCodeSource;
use ascii_agents_core::source::manager::SourceManager;
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentEvent, Reducer, SceneState, TaggedReceiver, Transport};
use tokio::sync::{mpsc, watch};

/// The reducer publishes a fresh `Arc<SceneState>` on every mutation through
/// this watch channel. Consumers (renderer, headless summary loop) hold a
/// `Receiver`, call `borrow()` for an O(1) pointer read, and never block
/// the writer. Replaces the old `Arc<RwLock<SceneState>>` so:
///   - cloning is a pointer copy (Arc::clone), not a heap allocation per
///     field (thanks to interned `Arc<str>` strings in `AgentSlot`)
///   - the renderer never holds a lock that could block the reducer
///   - swapping in a v2 daemon means publishing the same Arc over a socket
pub type SceneRx = watch::Receiver<Arc<SceneState>>;

/// Bootstrap desk capacity — used only before the first TUI frame
/// auto-computes the real per-floor capacity from terminal dimensions.
const BOOTSTRAP_DESKS: usize = 16;

#[allow(clippy::too_many_arguments)]
pub fn run(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    pack_dir: Option<PathBuf>,
    desk_cap: Option<usize>,
    headless: bool,
    theme_name: String,
    config_path: PathBuf,
    enabled_pets: Vec<crate::tui::pet::PetKind>,
) -> Result<()> {
    let theme = crate::tui::theme::theme_by_name(&theme_name).ok_or_else(|| {
        let valid: Vec<&str> = crate::tui::theme::ALL_THEMES
            .iter()
            .map(|t| t.name)
            .collect();
        anyhow::anyhow!("unknown theme: {theme_name}. Valid: {}", valid.join(", "))
    })?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        run_async(
            socket,
            projects_root,
            pack_dir,
            desk_cap,
            headless,
            theme,
            config_path,
            enabled_pets,
        )
        .await
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_async(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    pack_dir: Option<PathBuf>,
    desk_cap: Option<usize>,
    headless: bool,
    theme: &'static crate::tui::theme::Theme,
    config_path: PathBuf,
    enabled_pets: Vec<crate::tui::pet::PetKind>,
) -> Result<()> {
    let mut cc_src = ClaudeCodeSource::default_paths();
    if let Some(s) = socket {
        cc_src.socket_path = s;
    }
    if let Some(p) = projects_root {
        cc_src.projects_root = p;
    }

    let ag_src = AntigravitySource::default_paths();

    let (tx, rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    let boot = desk_cap.unwrap_or(BOOTSTRAP_DESKS);
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::uniform(boot)));

    let floor_caps: Arc<[AtomicUsize; MAX_FLOORS]> =
        Arc::new(std::array::from_fn(|_| AtomicUsize::new(boot)));

    tokio::spawn(reducer_task(rx, scene_tx, Arc::clone(&floor_caps)));

    let _source_handles = SourceManager::new()
        .with_source(Box::new(cc_src))
        .with_source(Box::new(ag_src))
        .spawn(tx);

    if headless {
        headless_loop(scene_rx).await
    } else {
        crate::tui::run_tui(
            scene_rx,
            pack_dir,
            floor_caps,
            theme,
            config_path,
            desk_cap,
            enabled_pets,
        )
        .await
    }
}

async fn reducer_task(
    mut rx: TaggedReceiver,
    scene_tx: watch::Sender<Arc<SceneState>>,
    floor_caps: Arc<[AtomicUsize; MAX_FLOORS]>,
) {
    let mut reducer = Reducer::new();
    let initial_cap = floor_caps[0].load(Ordering::Relaxed);
    let mut scene = SceneState::uniform(initial_cap);
    // 1-Hz tick so exit-grace sweeps run even when no new events arrive.
    let mut sweep_interval = tokio::time::interval(Duration::from_secs(1));
    sweep_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        // Sync per-floor capacities from the shared atomics so the
        // auto-computed layout capacity propagates to next_free_desk().
        for (i, a) in floor_caps.iter().enumerate() {
            scene.floor_capacities[i] = a.load(Ordering::Relaxed);
        }
        tokio::select! {
            event = rx.recv() => {
                let Some((transport, ev)) = event else { break };
                let now = SystemTime::now();
                tracing::debug!(?transport, ?ev, "event");
                reducer.apply(&mut scene, ev, now, transport);
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
            _ = sweep_interval.tick() => {
                reducer.tick(&mut scene, SystemTime::now());
                if scene_tx.send(Arc::new(scene.clone())).is_err() {
                    tracing::warn!("scene channel closed — renderer dropped");
                    break;
                }
            }
        }
    }
}

async fn headless_loop(mut scene_rx: SceneRx) -> Result<()> {
    tracing::info!("ascii-agents headless mode — Ctrl-C to quit");
    let mut prev_summary = String::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                let snapshot = scene_rx.borrow_and_update().clone();
                let summary = summarize(&snapshot);
                if summary != prev_summary {
                    println!("{summary}");
                    prev_summary = summary;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

fn summarize(scene: &SceneState) -> String {
    let agents: Vec<String> = scene
        .agents
        .values()
        .map(|a| {
            let state = match &a.state {
                ActivityState::Idle => "idle".to_string(),
                ActivityState::Active { detail, .. } => {
                    format!("active({})", detail.as_deref().unwrap_or("?"))
                }
                ActivityState::Waiting { reason } => format!("waiting({reason})"),
            };
            format!("{}@{}:{}", a.label, a.desk_index, state)
        })
        .collect();
    format!("agents=[{}]", agents.join(", "))
}
