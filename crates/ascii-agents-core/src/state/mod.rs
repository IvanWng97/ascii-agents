use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crate::id::AgentId;
use crate::source::Activity;

pub mod reducer;

/// `AgentSlot` strings (label, source, session_id) and paths (cwd) are
/// stored as `Arc<str>` / `Arc<Path>` so `SceneState::clone()` is a series
/// of pointer copies instead of heap allocations. At 30 fps with N agents
/// this turns ~5N allocations/frame into 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityState {
    Idle,
    Active {
        activity: Activity,
        tool_use_id: Option<Arc<str>>,
        detail: Option<Arc<str>>,
    },
    Waiting {
        reason: Arc<str>,
    },
}

#[derive(Debug, Clone)]
pub struct AgentSlot {
    pub agent_id: AgentId,
    pub source: Arc<str>,
    pub session_id: Arc<str>,
    pub cwd: Arc<Path>,
    pub label: Arc<str>,
    pub state: ActivityState,
    pub state_started_at: SystemTime,
    /// Wall-clock time of the most recent event (any type) from this
    /// agent. The stale-agent sweep uses this as the primary liveness
    /// signal — if `now - last_event_at` exceeds a state-dependent
    /// threshold, the agent is presumed dead and begins the exit
    /// animation. Updated on every `reducer::apply` that touches the slot.
    pub last_event_at: SystemTime,
    /// Wall-clock time the slot was first created. Distinct from
    /// `state_started_at` (updated on every state change) so the renderer
    /// can play a one-shot entry animation for the first few seconds of
    /// an agent's life regardless of later state transitions.
    pub created_at: SystemTime,
    /// Set when the reducer has received `SessionEnd` for this agent but
    /// is keeping the slot alive long enough for the exit animation to
    /// play. The reducer sweeps expired slots on subsequent events.
    pub exiting_at: Option<SystemTime>,
    /// Active→Idle debounce mark. Set by `ActivityEnd` instead of an
    /// immediate state flip; cleared by any later `ActivityStart`/Waiting.
    /// `reducer.tick` expires it after `ACTIVE_GRACE_WINDOW` and flips
    /// state to Idle. Hides the per-tool-call Active flicker that rapid
    /// PreToolUse → PostToolUse chains produce in CC.
    pub pending_idle_at: Option<SystemTime>,
    pub desk_index: usize,
    pub tool_call_count: u32,
    pub active_ms: u64,
}

#[derive(Debug, Default, Clone)]
pub struct SceneState {
    pub agents: BTreeMap<AgentId, AgentSlot>,
    pub max_desks: usize,
}

impl SceneState {
    pub fn new(max_desks: usize) -> Self {
        Self {
            agents: BTreeMap::new(),
            max_desks,
        }
    }

    /// Lowest free desk index, or `None` if all desks are occupied.
    pub fn next_free_desk(&self) -> Option<usize> {
        let occupied: std::collections::BTreeSet<usize> =
            self.agents.values().map(|a| a.desk_index).collect();
        (0..self.max_desks).find(|i| !occupied.contains(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_free_desk_starts_at_zero() {
        let s = SceneState::new(4);
        assert_eq!(s.next_free_desk(), Some(0));
    }

    #[test]
    fn next_free_desk_returns_none_when_full() {
        let mut s = SceneState::new(2);
        let now = SystemTime::now();
        for i in 0..2 {
            let id = AgentId::from_transcript_path(&format!("p{i}"));
            s.agents.insert(
                id,
                AgentSlot {
                    agent_id: id,
                    source: Arc::from("claude-code"),
                    session_id: Arc::from(format!("s{i}").as_str()),
                    cwd: Arc::from(Path::new("/")),
                    label: Arc::from(format!("cc#{i}").as_str()),
                    state: ActivityState::Idle,
                    state_started_at: now,
                    created_at: now,
                    last_event_at: now,
                    exiting_at: None,
                    pending_idle_at: None,
                    desk_index: i,
                    tool_call_count: 0,
                    active_ms: 0,
                },
            );
        }
        assert_eq!(s.next_free_desk(), None);
    }
}
