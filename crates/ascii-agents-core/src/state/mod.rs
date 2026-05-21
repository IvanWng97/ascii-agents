use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::id::AgentId;
use crate::source::Activity;

pub mod reducer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityState {
    Idle,
    Active {
        activity: Activity,
        tool_use_id: Option<String>,
        detail: Option<String>,
    },
    Waiting {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct AgentSlot {
    pub agent_id: AgentId,
    pub source: String,
    pub session_id: String,
    pub cwd: PathBuf,
    pub label: String,
    pub state: ActivityState,
    pub state_started_at: SystemTime,
    pub desk_index: usize,
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
                    source: "claude-code".into(),
                    session_id: format!("s{i}"),
                    cwd: PathBuf::from("/"),
                    label: format!("cc#{i}"),
                    state: ActivityState::Idle,
                    state_started_at: now,
                    desk_index: i,
                },
            );
        }
        assert_eq!(s.next_free_desk(), None);
    }
}
