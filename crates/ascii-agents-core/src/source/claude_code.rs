use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;

use crate::source::hook::HookSocketListener;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Source, TaggedSender};

/// Source that listens for Claude Code activity via hooks (primary) and
/// transcript JSONL files (fallback).
pub struct ClaudeCodeSource {
    pub socket_path: PathBuf,
    pub projects_root: PathBuf,
}

impl ClaudeCodeSource {
    pub fn default_paths() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Self {
            socket_path: PathBuf::from("/tmp/ascii-agents.sock"),
            projects_root: PathBuf::from(format!("{home}/.claude/projects")),
        }
    }
}

#[async_trait]
impl Source for ClaudeCodeSource {
    fn name(&self) -> &str {
        "claude-code"
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let socket = HookSocketListener::bind(self.socket_path.clone()).await?;
        let watcher = JsonlWatcher::new(self.projects_root.clone());

        let tx_hook = tx.clone();
        let tx_jsonl = tx.clone();
        let hook_task = tokio::spawn(async move { socket.run(tx_hook).await });
        let jsonl_task = tokio::spawn(async move { watcher.run(tx_jsonl).await });

        // When either sub-task ends, explicitly abort the other so it doesn't
        // keep running orphaned in the tokio runtime. Surface whichever inner
        // error (if any) triggered the early exit.
        let hook_abort = hook_task.abort_handle();
        let jsonl_abort = jsonl_task.abort_handle();

        let inner: Result<()> = tokio::select! {
            r = hook_task => {
                tracing::warn!("hook listener exited first; aborting jsonl watcher");
                jsonl_abort.abort();
                r?
            }
            r = jsonl_task => {
                tracing::warn!("jsonl watcher exited first; aborting hook listener");
                hook_abort.abort();
                r?
            }
        };
        inner
    }
}
