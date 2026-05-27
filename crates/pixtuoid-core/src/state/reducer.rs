use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::source::{AgentEvent, Transport};
use crate::state::{ActivityState, AgentSlot, SceneState};
use crate::AgentId;

/// Window in which a Hook event suppresses a later Jsonl event with the same tool_use_id.
pub const HOOK_WINS_WINDOW: Duration = Duration::from_millis(500);

/// How long to keep an exiting agent's slot alive after `SessionEnd` so the
/// walkout-to-door animation has time to play before the slot is removed.
pub const EXIT_GRACE_WINDOW: Duration = Duration::from_millis(4500);

/// How long the slot stays visually Active after an `ActivityEnd` before
/// the reducer's tick flips it to Idle. Hides the per-tool-call Active
/// flicker that rapid PreToolUse → PostToolUse chains produce in CC; any
/// `ActivityStart` arriving within this window cancels the pending idle,
/// so the slot reads as continuously Active for chained tool work.
pub const ACTIVE_GRACE_WINDOW: Duration = Duration::from_millis(1500);

/// State-adaptive stale-agent thresholds. If `now - last_event_at`
/// exceeds the threshold for the agent's current state, the reducer
/// marks it exiting. Modeled after Kubernetes liveness probes (detect
/// failure to respond, not the act of dying) + Prometheus staleness
/// (5-min scrape gap = stale target).
///
/// Active: CC fires tool events every few seconds when working. 10 min
///   of silence means the process died mid-tool.
/// Idle: users legitimately pause for breaks. 30 min catches "closed
///   terminal" without reaping lunch-break idle.
/// Waiting: user could be in a meeting reviewing the permission prompt.
///   60 min is generous but still GCs eventually.
/// Unknown cwd (cc#N label): almost always a ghost from startup JSONL
///   seeding that never gets a follow-up event. 3 min is aggressive
///   but the false-positive cost is low (just a desk slot freed).
pub const STALE_ACTIVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const STALE_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const STALE_WAITING_TIMEOUT: Duration = Duration::from_secs(60 * 60);
pub const STALE_UNKNOWN_CWD_TIMEOUT: Duration = Duration::from_secs(3 * 60);

#[derive(Debug, Default)]
pub struct Reducer {
    /// Track recent hook-derived events so JSONL duplicates can be dropped.
    recent_hook_tool_uses: HashMap<(AgentId, String), SystemTime>,
    /// Per-agent set of Task tool_use_ids currently in flight. CC's hook
    /// payload sets `transcript_path` to the PARENT'S transcript even when a
    /// subagent is the actor, so subagent hook events hash to the parent's
    /// AgentId. While the parent has any Task in flight, hook
    /// ActivityStart/End events for that AgentId are dropped — JSONL has
    /// correct attribution to the subagent's own AgentId.
    active_tasks: HashMap<AgentId, HashSet<String>>,
    /// Monotonic counter for human-readable labels.
    next_label_n: u32,
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run the GC + exit-sweep + Active→Idle debounce expiry without
    /// applying an event. Must be called periodically (e.g. on each
    /// render tick) so exiting slots are reclaimed and pending-idle
    /// timers actually fire even when no new event arrives to drive
    /// `apply`.
    pub fn tick(&mut self, scene: &mut SceneState, now: SystemTime) {
        self.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);
        self.sweep_stale(scene, now);
        // Clean up active_tasks entries for agents that never got a
        // SessionStart (Task event arrived before JSONL created the slot).
        self.active_tasks
            .retain(|id, _| scene.agents.contains_key(id));
    }

    pub fn apply(
        &mut self,
        scene: &mut SceneState,
        event: AgentEvent,
        now: SystemTime,
        from: Transport,
    ) {
        self.gc(now);
        self.sweep_exited(scene, now);
        self.expire_pending_idles(scene, now);
        let id = event.agent_id();

        // Subagent-leak suppression: if this AgentId currently has any Task
        // tool in flight, hook ActivityStart/End events for it are almost
        // certainly subagent work misattributed to the parent. Drop them and
        // defer to JSONL, which targets the subagent's own AgentId. The
        // Task's own PostToolUse is exempt — its tool_use_id matches one we
        // are tracking, so it passes through and clears the slot.
        if from == Transport::Hook {
            let in_task = self.active_tasks.get(&id).is_some_and(|s| !s.is_empty());
            let suppress = match &event {
                AgentEvent::ActivityStart { .. } => in_task,
                AgentEvent::ActivityEnd { tool_use_id, .. } => {
                    let is_task_self_end = tool_use_id
                        .as_ref()
                        .is_some_and(|t| self.active_tasks.get(&id).is_some_and(|s| s.contains(t)));
                    in_task && !is_task_self_end
                }
                _ => false,
            };
            if suppress {
                return;
            }
        }

        // Dedup: drop JSONL events that match a recent Hook event by tool_use_id.
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

        // Track active Task tool_use_ids from either transport. HashSet is
        // idempotent so duplicate inserts from both hook+jsonl are harmless.
        //
        // Side effect: when the parent gains a Task, also mark it as
        // Active("Delegating") so it doesn't look idle/asleep while its
        // subagents do the visible work. When the last Task drains, the
        // next normal hook/JSONL event will reset its state.
        //
        // `handled_by_task_tracking`: when an ActivityEnd drains
        // active_tasks, the general ActivityEnd arm below must be
        // skipped — otherwise it would redundantly re-arm
        // pending_idle_at or arm it while tasks are still in flight.
        let mut handled_by_task_tracking = false;
        let mut handled_by_task_start = false;
        match &event {
            AgentEvent::ActivityStart {
                agent_id,
                tool_use_id: Some(tuid),
                detail: Some(d),
                ..
            } if d.is_task() => {
                handled_by_task_start = true;
                self.active_tasks
                    .entry(*agent_id)
                    .or_default()
                    .insert(tuid.clone());
                if let Some(slot) = scene.agents.get_mut(agent_id) {
                    if matches!(slot.state, ActivityState::Active { .. }) {
                        let elapsed = now
                            .duration_since(slot.state_started_at)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        slot.active_ms += elapsed;
                    }
                    slot.state = ActivityState::Active {
                        activity: crate::source::Activity::Typing,
                        tool_use_id: Some(Arc::<str>::from(tuid.as_str())),
                        detail: Some(Arc::<str>::from("Delegating")),
                    };
                    slot.state_started_at = now;
                    slot.pending_idle_at = None;
                }
            }
            AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id: Some(tuid),
            } => {
                if let Some(set) = self.active_tasks.get_mut(agent_id) {
                    if set.remove(tuid) {
                        handled_by_task_tracking = true;
                        if let Some(slot) = scene.agents.get_mut(agent_id) {
                            slot.last_event_at = now;
                            if set.is_empty() {
                                // Debounce: stay visually Active for
                                // ACTIVE_GRACE_WINDOW; expire_pending_idles
                                // flips to Idle if no new tool starts
                                // inside the window.
                                slot.pending_idle_at = Some(now);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        match event {
            AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            } => {
                if scene.agents.contains_key(&agent_id) {
                    return;
                }
                let Some(desk_index) = scene.next_free_desk() else {
                    tracing::warn!(
                        ?agent_id,
                        cwd = %cwd.display(),
                        session_id = %session_id,
                        total_capacity = scene.total_capacity(),
                        "dropped SessionStart — all desks occupied; bump --max-desks"
                    );
                    return;
                };
                let floor_idx = scene.floor_of(desk_index);
                self.next_label_n += 1;
                let has_cwd = cwd
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|s| !s.is_empty())
                    .is_some();
                let label: Arc<str> = if has_cwd {
                    Arc::<str>::from(cwd.file_name().and_then(|n| n.to_str()).unwrap_or(&source))
                } else {
                    let prefix: String = source.chars().take(2).collect();
                    Arc::<str>::from(format!("{prefix}#{}", self.next_label_n).as_str())
                };
                // Disambiguation for multiple sessions sharing a cwd happens
                // at render time, not here — we don't want to suffix unique
                // sessions with a noisy `·xxxx` they don't need.
                scene.agents.insert(
                    agent_id,
                    AgentSlot {
                        agent_id,
                        source: Arc::<str>::from(source.as_str()),
                        session_id: Arc::<str>::from(session_id.as_str()),
                        cwd: Arc::<std::path::Path>::from(cwd.as_path()),
                        label,
                        state: ActivityState::Idle,
                        state_started_at: now,
                        last_event_at: now,
                        created_at: now,
                        exiting_at: None,
                        pending_idle_at: None,
                        desk_index,
                        floor_idx,
                        tool_call_count: 0,
                        active_ms: 0,
                        unknown_cwd: !has_cwd,
                        parent_id,
                    },
                );
            }
            AgentEvent::ActivityStart {
                agent_id,
                activity,
                tool_use_id,
                detail,
            } => {
                if !handled_by_task_start {
                    if let Some(slot) = scene.agents.get_mut(&agent_id) {
                        if !detail.as_ref().is_some_and(|d| d.is_task()) {
                            slot.tool_call_count += 1;
                        }
                        if matches!(slot.state, ActivityState::Active { .. }) {
                            let elapsed = now
                                .duration_since(slot.state_started_at)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            slot.active_ms += elapsed;
                        }
                        slot.state = ActivityState::Active {
                            activity,
                            tool_use_id: tool_use_id.map(|s| Arc::<str>::from(s.as_str())),
                            detail: detail.map(|d| Arc::<str>::from(d.display())),
                        };
                        slot.state_started_at = now;
                        slot.last_event_at = now;
                        slot.pending_idle_at = None;
                    }
                }
            }
            AgentEvent::ActivityEnd { agent_id, .. } => {
                // Skip if this end was already processed by task tracking above.
                if !handled_by_task_tracking {
                    if let Some(slot) = scene.agents.get_mut(&agent_id) {
                        // Only arm the idle debounce when actually Active — an
                        // ActivityEnd arriving while Idle or Waiting is a stale
                        // duplicate and should not re-arm the timer.
                        if matches!(slot.state, ActivityState::Active { .. }) {
                            slot.pending_idle_at = Some(now);
                        }
                        slot.last_event_at = now;
                    }
                }
            }
            AgentEvent::Waiting { agent_id, reason } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    if matches!(slot.state, ActivityState::Active { .. }) {
                        let elapsed = now
                            .duration_since(slot.state_started_at)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        slot.active_ms += elapsed;
                    }
                    slot.state = ActivityState::Waiting {
                        reason: Arc::<str>::from(reason.as_str()),
                    };
                    slot.state_started_at = now;
                    slot.last_event_at = now;
                    slot.pending_idle_at = None;
                }
            }
            AgentEvent::Rename { agent_id, label } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    if &*slot.label != label.as_str() {
                        slot.label = Arc::<str>::from(label.as_str());
                    }
                    slot.last_event_at = now;
                }
            }
            AgentEvent::SessionEnd { agent_id } => {
                if let Some(slot) = scene.agents.get_mut(&agent_id) {
                    if slot.exiting_at.is_none() {
                        slot.exiting_at = Some(now);
                    }
                }
                let mut visited = HashSet::new();
                visited.insert(agent_id);
                let mut frontier = vec![agent_id];
                while let Some(parent) = frontier.pop() {
                    let children: Vec<AgentId> = scene
                        .agents
                        .values()
                        .filter(|s| s.parent_id == Some(parent) && s.exiting_at.is_none())
                        .map(|s| s.agent_id)
                        .collect();
                    for cid in children {
                        if visited.insert(cid) {
                            if let Some(slot) = scene.agents.get_mut(&cid) {
                                slot.exiting_at = Some(now);
                            }
                            frontier.push(cid);
                        }
                    }
                }
            }
        }
    }

    fn gc(&mut self, now: SystemTime) {
        // SystemTime::duration_since returns Err when `ts` is in the future
        // (clock went backwards). Drop those — stale entries either way.
        self.recent_hook_tool_uses
            .retain(|_, ts| now.duration_since(*ts).is_ok_and(|d| d < HOOK_WINS_WINDOW));
    }

    /// Walk through agents with `pending_idle_at` set and flip their
    /// state to Idle if the debounce window has elapsed. Resets
    /// `state_started_at` to `now` so the Idle wander state machine
    /// starts fresh from the visible transition, not from the
    /// (now-stale) original ActivityEnd time. Slots already in a
    /// non-Active state (e.g. Waiting from a parallel permission
    /// prompt) are left alone — only the originating Active slot
    /// gets flipped.
    fn expire_pending_idles(&mut self, scene: &mut SceneState, now: SystemTime) {
        for slot in scene.agents.values_mut() {
            let Some(pending) = slot.pending_idle_at else {
                continue;
            };
            if now
                .duration_since(pending)
                .is_ok_and(|d| d >= ACTIVE_GRACE_WINDOW)
            {
                if matches!(slot.state, ActivityState::Active { .. }) {
                    let elapsed = pending
                        .duration_since(slot.state_started_at)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    slot.active_ms += elapsed;
                    slot.state = ActivityState::Idle;
                    slot.state_started_at = now;
                }
                slot.pending_idle_at = None;
            }
        }
    }

    /// Mark agents as exiting when they haven't emitted any event for
    /// longer than their state-adaptive threshold. Uses `last_event_at`
    /// (updated on every reducer event) as the liveness signal, NOT
    /// `state_started_at` (which only tracks the current state's age).
    ///
    /// Unknown-cwd agents (label starts with "cc#") get a much shorter
    /// timeout — they're almost always ghosts from JSONL startup seeding.
    fn sweep_stale(&mut self, scene: &mut SceneState, now: SystemTime) {
        for slot in scene.agents.values_mut() {
            if slot.exiting_at.is_some() {
                continue;
            }
            let age = now
                .duration_since(slot.last_event_at)
                .unwrap_or(Duration::ZERO);
            let unknown_cwd = slot.unknown_cwd;
            let threshold = if unknown_cwd {
                STALE_UNKNOWN_CWD_TIMEOUT
            } else {
                match &slot.state {
                    ActivityState::Active { .. } => STALE_ACTIVE_TIMEOUT,
                    ActivityState::Idle => STALE_IDLE_TIMEOUT,
                    ActivityState::Waiting { .. } => STALE_WAITING_TIMEOUT,
                }
            };
            if age > threshold {
                tracing::info!(
                    agent_id = ?slot.agent_id,
                    label = %slot.label,
                    age_secs = age.as_secs(),
                    threshold_secs = threshold.as_secs(),
                    "stale agent — marking exiting"
                );
                slot.exiting_at = Some(now);
            }
        }
    }

    /// Remove agents whose exit animation has finished. Called at the top
    /// of every event apply, so any subsequent event naturally triggers
    /// the cleanup of expired slots.
    fn sweep_exited(&mut self, scene: &mut SceneState, now: SystemTime) {
        let expired: Vec<AgentId> = scene
            .agents
            .iter()
            .filter_map(|(id, slot)| {
                slot.exiting_at
                    .filter(|t| now.duration_since(*t).is_ok_and(|d| d > EXIT_GRACE_WINDOW))
                    .map(|_| *id)
            })
            .collect();
        for id in expired {
            scene.agents.remove(&id);
            self.active_tasks.remove(&id);
        }
    }
}

fn event_tool_use_id(ev: &AgentEvent) -> Option<&str> {
    match ev {
        AgentEvent::ActivityStart { tool_use_id, .. }
        | AgentEvent::ActivityEnd { tool_use_id, .. } => tool_use_id.as_deref(),
        _ => None,
    }
}
