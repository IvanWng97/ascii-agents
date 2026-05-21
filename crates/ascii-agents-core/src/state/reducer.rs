use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::source::AgentEvent;
use crate::state::{ActivityState, AgentSlot, SceneState};
use crate::AgentId;

/// Which transport produced an event — used for dedup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Hook,
    Jsonl,
}

/// Window in which a Hook event suppresses a later Jsonl event with the same tool_use_id.
pub const HOOK_WINS_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub struct Reducer {
    /// Track recent hook-derived events so JSONL duplicates can be dropped.
    recent_hook_tool_uses: HashMap<(AgentId, String), SystemTime>,
    /// Monotonic counter for human-readable labels (cc#1, cc#2, ...).
    next_label_n: u32,
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(
        &mut self,
        scene: &mut SceneState,
        event: AgentEvent,
        now: SystemTime,
        from: Transport,
    ) {
        self.gc(now);
        let id = event.agent_id();

        // Dedup: drop JSONL events that match a recent Hook event by tool_use_id.
        // NOTE: CC's hook payloads do NOT carry tool_use_id today (only JSONL does),
        // so the hook side rarely populates the dedup map. We still try, in case a
        // future CC version adds it, and to keep the logic correct on both sides.
        if from == Transport::Jsonl {
            if let Some(tuid) = event_tool_use_id(&event) {
                if self
                    .recent_hook_tool_uses
                    .contains_key(&(id, tuid.to_string()))
                {
                    return;
                }
            }
        }

        if from == Transport::Hook {
            if let Some(tuid) = event_tool_use_id(&event) {
                self.recent_hook_tool_uses
                    .insert((id, tuid.to_string()), now);
            }
        }

        match event {
            AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
            } => {
                if scene.agents.contains_key(&agent_id) {
                    return;
                }
                let Some(desk_index) = scene.next_free_desk() else {
                    return;
                };
                self.next_label_n += 1;
                let label = format!("cc#{}", self.next_label_n);
                scene.agents.insert(
                    agent_id,
                    AgentSlot {
                        agent_id,
                        source,
                        session_id,
                        cwd,
                        label,
                        state: ActivityState::Idle,
                        state_started_at: now,
                        desk_index,
                    },
                );
            }
            AgentEvent::ActivityStart {
                agent_id,
                activity,
                tool_use_id,
                detail,
            } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    slot.state = ActivityState::Active {
                        activity,
                        tool_use_id,
                        detail,
                    };
                    slot.state_started_at = now;
                }
            }
            AgentEvent::ActivityEnd { agent_id, .. } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    slot.state = ActivityState::Idle;
                    slot.state_started_at = now;
                }
            }
            AgentEvent::Waiting { agent_id, reason } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    slot.state = ActivityState::Waiting { reason };
                    slot.state_started_at = now;
                }
            }
            AgentEvent::SessionEnd { agent_id } => {
                scene.agents.remove(&agent_id);
            }
        }
    }

    fn gc(&mut self, now: SystemTime) {
        // SystemTime::duration_since returns Err when `ts` is in the future
        // (clock went backwards). Drop those — stale entries either way.
        self.recent_hook_tool_uses.retain(|_, ts| {
            now.duration_since(*ts)
                .is_ok_and(|d| d < HOOK_WINS_WINDOW)
        });
    }
}

fn event_tool_use_id(ev: &AgentEvent) -> Option<&str> {
    match ev {
        AgentEvent::ActivityStart { tool_use_id, .. }
        | AgentEvent::ActivityEnd { tool_use_id, .. } => tool_use_id.as_deref(),
        _ => None,
    }
}
