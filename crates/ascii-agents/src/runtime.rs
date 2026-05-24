use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
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

pub fn run(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    pack_dir: Option<PathBuf>,
    max_desks: usize,
    headless: bool,
    theme_name: String,
) -> Result<()> {
    let theme = crate::tui::theme::theme_by_name(&theme_name)
        .ok_or_else(|| anyhow::anyhow!("unknown theme: {theme_name}"))?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        run_async(socket, projects_root, pack_dir, max_desks, headless, theme).await
    })
}

async fn run_async(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    pack_dir: Option<PathBuf>,
    max_desks: usize,
    headless: bool,
    theme: &'static crate::tui::theme::Theme,
) -> Result<()> {
    let mut src = ClaudeCodeSource::default_paths();
    if let Some(s) = socket {
        src.socket_path = s;
    }
    if let Some(p) = projects_root {
        src.projects_root = p;
    }

    let (tx, rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    let (scene_tx, scene_rx) = watch::channel(Arc::new(SceneState::new(max_desks)));

    tokio::spawn(reducer_task(rx, scene_tx, max_desks));

    let _source_handles = SourceManager::new().with_source(Box::new(src)).spawn(tx);

    let max_desks_shared = Arc::new(AtomicUsize::new(max_desks));

    if headless {
        headless_loop(scene_rx).await
    } else {
        crate::tui::run_tui(scene_rx, pack_dir, max_desks_shared, theme).await
    }
}

async fn reducer_task(
    mut rx: TaggedReceiver,
    scene_tx: watch::Sender<Arc<SceneState>>,
    max_desks: usize,
) {
    let mut reducer = Reducer::new();
    let mut scene = SceneState::new(max_desks);
    // 1-Hz tick so exit-grace sweeps run even when no new events arrive.
    let mut sweep_interval = tokio::time::interval(Duration::from_secs(1));
    sweep_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = rx.recv() => {
                let Some((transport, ev)) = event else { break };
                let now = SystemTime::now();
                tracing::debug!(?transport, ?ev, "event");
                reducer.apply(&mut scene, ev, now, transport);
                // Send a fresh Arc snapshot. send() ignores errors when
                // there are no active receivers — that's fine.
                let _ = scene_tx.send(Arc::new(scene.clone()));
            }
            _ = sweep_interval.tick() => {
                reducer.tick(&mut scene, SystemTime::now());
                let _ = scene_tx.send(Arc::new(scene.clone()));
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
