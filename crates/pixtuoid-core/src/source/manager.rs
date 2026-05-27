use tokio::task::JoinHandle;

use crate::source::{Source, TaggedSender};

/// Owns a set of `Source` implementations and spawns each as its own tokio
/// task, multiplexing their events onto a single `TaggedSender`. The single-
/// source case is just `SourceManager::new().with_source(Box::new(src)).spawn(tx)`.
/// Adding a second CLI (Codex, Cursor, Gemini, …) is a one-line addition.
#[derive(Default)]
pub struct SourceManager {
    sources: Vec<Box<dyn Source>>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one more `Source`. Builder-style — chain to add several.
    /// Named `with_source` (not `add`) to avoid clippy's
    /// `should_implement_trait` confusing it with `std::ops::Add`.
    pub fn with_source(mut self, source: Box<dyn Source>) -> Self {
        self.sources.push(source);
        self
    }

    /// Spawn one tokio task per source. Each task gets its own clone of `tx`,
    /// so the channel stays open as long as any source is alive. Errors from
    /// individual sources are logged via `tracing` and do not abort siblings.
    pub fn spawn(self, tx: TaggedSender) -> Vec<JoinHandle<()>> {
        self.sources
            .into_iter()
            .map(|src| {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = src.run(tx).await {
                        tracing::error!("source died: {e}");
                    }
                })
            })
            .collect()
    }
}
