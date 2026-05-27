use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::id::AgentId;

/// Which transport produced an event — used by the reducer for hook-wins
/// dedup. Lives on the source side because every `Source` implementor must
/// tag its own events; the reducer is downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Hook,
    Jsonl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activity {
    Typing,
    Reading,
    Thinking,
}

/// Structured tool detail. Replaces the free-form `Option<String>` so the
/// reducer can pattern-match (instead of string-scanning) on semantic
/// categories like Task-delegation, which is load-bearing for subagent
/// suppression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolDetail {
    /// CC `Task` tool — kicks off a subagent. Reducer suppresses
    /// hook-sourced Activity events for the parent until the matching
    /// `ActivityEnd` arrives (subagent leak suppression).
    Task,
    /// Any other tool. `display` is the user-facing label
    /// (e.g. `"Bash: ls"`, `"Edit foo.rs"`) used for the AgentSlot detail.
    Generic { display: String },
}

impl ToolDetail {
    pub fn display(&self) -> &str {
        match self {
            ToolDetail::Task => "Delegating",
            ToolDetail::Generic { display } => display,
        }
    }
    pub fn is_task(&self) -> bool {
        matches!(self, ToolDetail::Task)
    }
}

/// Test-ergonomic conversion. `"Task"` maps to `Task`; everything else
/// maps to `Generic`. Production code should call `decoder::make_tool_detail`
/// directly so it sees `tool_name` and `target` as separate inputs, but
/// tests that build `AgentEvent::ActivityStart` manually benefit from
/// `Some("Task".into())` working as expected.
impl From<&str> for ToolDetail {
    fn from(s: &str) -> Self {
        if s == "Task" {
            ToolDetail::Task
        } else {
            ToolDetail::Generic {
                display: s.to_string(),
            }
        }
    }
}

impl From<String> for ToolDetail {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    SessionStart {
        agent_id: AgentId,
        source: String,
        session_id: String,
        cwd: PathBuf,
        parent_id: Option<AgentId>,
    },
    ActivityStart {
        agent_id: AgentId,
        activity: Activity,
        tool_use_id: Option<String>,
        detail: Option<ToolDetail>,
    },
    ActivityEnd {
        agent_id: AgentId,
        tool_use_id: Option<String>,
    },
    Waiting {
        agent_id: AgentId,
        reason: String,
    },
    /// Late-discovered display name (e.g. CC subagent `attributionAgent`).
    /// Reducer overrides the slot label; noop if the slot doesn't exist.
    Rename {
        agent_id: AgentId,
        label: String,
    },
    SessionEnd {
        agent_id: AgentId,
    },
}

impl AgentEvent {
    pub fn agent_id(&self) -> AgentId {
        match self {
            AgentEvent::SessionStart { agent_id, .. } => *agent_id,
            AgentEvent::ActivityStart { agent_id, .. } => *agent_id,
            AgentEvent::ActivityEnd { agent_id, .. } => *agent_id,
            AgentEvent::Waiting { agent_id, .. } => *agent_id,
            AgentEvent::Rename { agent_id, .. } => *agent_id,
            AgentEvent::SessionEnd { agent_id, .. } => *agent_id,
        }
    }
}

/// Events sent on a tagged channel so the reducer knows which transport produced them.
pub type TaggedSender = mpsc::Sender<(Transport, AgentEvent)>;
pub type TaggedReceiver = mpsc::Receiver<(Transport, AgentEvent)>;

/// A `Source` produces `AgentEvent`s from one agent CLI flavor (Claude Code,
/// Codex, Cursor, Gemini, Copilot, etc.) and sends them on a `Transport`-
/// tagged channel.
///
/// ## Implementor contract
///
/// 1. **`name()`** — returns a stable, lowercase identifier for this source
///    (e.g. `"claude-code"`, `"codex"`, `"cursor"`). Used both as the
///    `AgentSlot.source` field and as the first argument to
///    [`AgentId::from_parts`] so two sources with the same opaque session
///    id never collide.
///
/// 2. **`AgentId` derivation** — every `AgentEvent::SessionStart` MUST carry
///    an `agent_id` constructed via [`AgentId::from_parts(self.name(),
///    opaque_id)`][`AgentId::from_parts`]. `opaque_id` is whatever your source uses to uniquely
///    identify a session: a JSONL transcript path for CC, a session UUID
///    for SDK-based sources, the socket path for hook-based sources.
///    Constructing `AgentId`s any other way risks cross-source collisions.
///
/// 3. **Transport tagging** — every event you send must be tagged with the
///    appropriate [`Transport`] enum variant. The reducer relies on this
///    tag for hook-vs-JSONL dedup; sending the wrong tag silently breaks
///    that logic.
///
/// 4. **Never panic** — sources run inside a tokio task that doesn't
///    propagate panics cleanly. Log + continue on malformed input rather
///    than `unwrap`.
///
/// [`AgentId::from_parts`]: crate::AgentId::from_parts
#[async_trait]
pub trait Source: Send + 'static {
    fn name(&self) -> &str;
    async fn run(self: Box<Self>, tx: TaggedSender) -> anyhow::Result<()>;
}

pub mod antigravity;
pub mod claude_code;
pub mod decoder;
pub mod hook;
pub mod jsonl;
pub mod manager;
