use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use ascii_agents_core::source::claude_code::ClaudeCodeSource;
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{
    AgentEvent, Reducer, SceneState, TaggedReceiver, Transport,
};
use tokio::sync::{mpsc, RwLock};

pub fn run(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    max_desks: usize,
    headless: bool,
) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move { run_async(socket, projects_root, max_desks, headless).await })
}

async fn run_async(
    socket: Option<PathBuf>,
    projects_root: Option<PathBuf>,
    max_desks: usize,
    headless: bool,
) -> Result<()> {
    let mut src = ClaudeCodeSource::default_paths();
    if let Some(s) = socket {
        src.socket_path = s;
    }
    if let Some(p) = projects_root {
        src.projects_root = p;
    }

    let (tx, rx) = mpsc::channel::<(Transport, AgentEvent)>(256);
    let scene: Arc<RwLock<SceneState>> = Arc::new(RwLock::new(SceneState::new(max_desks)));

    let scene_for_reducer = scene.clone();
    tokio::spawn(reducer_task(rx, scene_for_reducer));

    let src_box: Box<dyn ascii_agents_core::source::Source> = Box::new(src);
    tokio::spawn(async move {
        if let Err(e) = src_box.run(tx).await {
            tracing::error!("source died: {e}");
        }
    });

    if headless {
        headless_loop(scene).await
    } else {
        crate::tui::run_tui(scene).await
    }
}

async fn reducer_task(mut rx: TaggedReceiver, scene: Arc<RwLock<SceneState>>) {
    let mut reducer = Reducer::new();
    while let Some((transport, ev)) = rx.recv().await {
        let now = SystemTime::now();
        tracing::info!(?transport, ?ev, "event");
        let mut s = scene.write().await;
        reducer.apply(&mut s, ev, now, transport);
    }
}

async fn headless_loop(scene: Arc<RwLock<SceneState>>) -> Result<()> {
    eprintln!("ascii-agents headless mode — Ctrl-C to quit");
    let mut prev_summary = String::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                let s = scene.read().await;
                let summary = summarize(&s);
                if summary != prev_summary {
                    println!("{summary}");
                    prev_summary = summary;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("shutting down");
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
                ActivityState::Active { detail, .. } => format!(
                    "active({})",
                    detail.as_deref().unwrap_or("?")
                ),
                ActivityState::Waiting { reason } => format!("waiting({reason})"),
            };
            format!("{}@{}:{}", a.label, a.desk_index, state)
        })
        .collect();
    format!("agents=[{}]", agents.join(", "))
}
