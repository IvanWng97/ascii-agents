use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::{Activity, AgentEvent, Transport};
use pixtuoid_core::state::reducer::Reducer;
use pixtuoid_core::state::{ActivityState, SceneState};
use pixtuoid_core::AgentId;

fn start(reducer: &mut Reducer, scene: &mut SceneState, id: AgentId) {
    reducer.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
}

#[test]
fn session_start_creates_idle_slot_at_first_free_desk() {
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");

    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).expect("agent inserted");
    assert_eq!(slot.desk_index, 0);
    assert_eq!(&*slot.label, "repo", "label derived from cwd basename");
    assert_eq!(slot.state, ActivityState::Idle);
}

#[test]
fn activity_start_sets_state_active() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: Some("Edit: foo.rs".into()),
        },
        SystemTime::now(),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(
        slot.state,
        ActivityState::Active {
            activity: Activity::Typing,
            ..
        }
    ));
}

#[test]
fn activity_end_arms_debounce_then_tick_flips_to_idle() {
    // After ActivityEnd the slot stays VISUALLY Active for
    // ACTIVE_GRACE_WINDOW (1500ms) — this hides per-tool-call flicker
    // from rapid CC tool chains. `pending_idle_at` is the debounce
    // armed-flag; `reducer.tick` (or another event past the window)
    // realizes the transition.
    use std::time::Duration;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t0 + Duration::from_millis(100),
        Transport::Hook,
    );

    // Immediately after ActivityEnd — still Active, debounce armed.
    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(slot.pending_idle_at.is_some());

    // Tick before window expires — still Active.
    r.tick(&mut scene, t0 + Duration::from_millis(900));
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));

    // Tick past the window — flips to Idle.
    r.tick(&mut scene, t0 + Duration::from_millis(2000));
    assert_eq!(scene.agents.get(&id).unwrap().state, ActivityState::Idle);
    assert!(scene.agents.get(&id).unwrap().pending_idle_at.is_none());
}

#[test]
fn activity_start_inside_grace_window_cancels_debounce() {
    // A new tool starting before the debounce window expires must
    // cancel the pending-idle so the slot reads as continuously
    // Active for chained tool work (Read → Glob → Edit etc.).
    use std::time::Duration;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t0 + Duration::from_millis(100),
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().pending_idle_at.is_some());
    // Second tool starts 200ms later — well inside the grace window.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t2".into()),
            detail: None,
        },
        t0 + Duration::from_millis(300),
        Transport::Hook,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(
        slot.pending_idle_at.is_none(),
        "ActivityStart inside grace must cancel pending idle"
    );
    // Tick well past the original ActivityEnd's grace — must still be Active.
    r.tick(&mut scene, t0 + Duration::from_millis(2500));
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));
}

#[test]
fn waiting_sets_state_with_reason() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "Bash: rm -rf?".into(),
        },
        SystemTime::now(),
        Transport::Hook,
    );

    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Waiting { reason } => assert_eq!(&**reason, "Bash: rm -rf?"),
        other => panic!("unexpected state: {other:?}"),
    }
}

#[test]
fn session_end_marks_slot_exiting_then_tick_removes_it_after_grace() {
    use pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW;
    let mut scene = SceneState::uniform(2);
    let mut r = Reducer::new();
    let a = AgentId::from_transcript_path("/p/a.jsonl");
    let b = AgentId::from_transcript_path("/p/b.jsonl");
    start(&mut r, &mut scene, a);
    start(&mut r, &mut scene, b);

    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: a },
        t0,
        Transport::Hook,
    );

    let slot = scene
        .agents
        .get(&a)
        .expect("slot still present during exit walk-out");
    assert!(
        slot.exiting_at.is_some(),
        "SessionEnd should mark exiting_at"
    );

    r.tick(
        &mut scene,
        t0 + EXIT_GRACE_WINDOW + std::time::Duration::from_millis(100),
    );
    assert!(
        !scene.agents.contains_key(&a),
        "tick should sweep expired exit"
    );
    assert_eq!(scene.next_free_desk(), Some(0));
}

#[test]
fn jsonl_duplicate_of_recent_hook_is_dropped() {
    let mut scene = SceneState::uniform(2);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t-1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Reading,
            tool_use_id: Some("t-1".into()),
            detail: Some("FROM_JSONL".into()),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    match &slot.state {
        ActivityState::Active {
            activity, detail, ..
        } => {
            assert_eq!(*activity, Activity::Typing, "hook event must win");
            assert_ne!(
                detail.as_deref(),
                Some("FROM_JSONL"),
                "jsonl detail must not overwrite"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// Bug 2: CC's hook payloads set transcript_path to the PARENT'S transcript
/// even for actions originating in a subagent. Those leak hook events onto
/// the parent's slot, making the parent sprite blink while the actual work
/// is in a subagent. Once the parent has a Task tool in flight, hook
/// ActivityStart/End events for that AgentId should be suppressed — the
/// JSONL stream is authoritative for the subagent (separate AgentId).
#[test]
fn hook_activity_during_active_task_is_suppressed() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    start(&mut r, &mut scene, parent);

    let t0 = SystemTime::now();

    // Parent enters Task tool — hook fires first, carrying the tool_use_id.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0,
        Transport::Hook,
    );

    // Subagent fires a Read hook. CC reports it on parent's transcript_path,
    // so it lands on parent's AgentId — we must drop it.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("subagent-R".into()),
            detail: Some("Read: /foo".into()),
        },
        t0 + Duration::from_millis(50),
        Transport::Hook,
    );

    // Parent slot should still reflect Task (now rendered as "Delegating"
    // per ToolDetail::display), not the leaked Read.
    let slot = scene.agents.get(&parent).unwrap();
    match &slot.state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Delegating"));
        }
        other => panic!("expected Active(Delegating), got {other:?}"),
    }

    // Subagent's PostToolUse hook for Read also lands on parent — drop it.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("subagent-R".into()),
        },
        t0 + Duration::from_millis(60),
        Transport::Hook,
    );
    let slot = scene.agents.get(&parent).unwrap();
    assert!(
        matches!(slot.state, ActivityState::Active { .. }),
        "parent must remain Active(Task) while task in flight"
    );

    // Task's own PostToolUse: tool_use_id matches the in-flight Task, so the
    // hook IS allowed through. With the Active-grace debounce, the
    // transition to Idle is deferred — `pending_idle_at` arms now,
    // `reducer.tick` past ACTIVE_GRACE_WINDOW (1500ms) realizes it.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: parent,
            tool_use_id: Some("task-T".into()),
        },
        t0 + Duration::from_millis(200),
        Transport::Hook,
    );
    let slot = scene.agents.get(&parent).unwrap();
    assert!(matches!(slot.state, ActivityState::Active { .. }));
    assert!(slot.pending_idle_at.is_some());
    r.tick(&mut scene, t0 + Duration::from_millis(2000));
    assert_eq!(
        scene.agents.get(&parent).unwrap().state,
        ActivityState::Idle
    );
}

/// JSONL is the authoritative attribution for subagent work — its events
/// go to the subagent's own AgentId (different file path) and must NOT be
/// affected by the parent's active Task suppression.
#[test]
fn subagent_jsonl_activity_is_unaffected_by_parent_task_suppression() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    let subagent = AgentId::from_transcript_path("/p/parent/subagents/agent-x.jsonl");
    start(&mut r, &mut scene, parent);
    start(&mut r, &mut scene, subagent);

    let t0 = SystemTime::now();
    // Parent enters a Task.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: parent,
            activity: Activity::Typing,
            tool_use_id: Some("task-T".into()),
            detail: Some("Task".into()),
        },
        t0,
        Transport::Hook,
    );
    // Subagent's JSONL activity targets ITS OWN AgentId — must apply normally.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: subagent,
            activity: Activity::Typing,
            tool_use_id: Some("sub-R".into()),
            detail: Some("Read: /bar".into()),
        },
        t0 + Duration::from_millis(120),
        Transport::Jsonl,
    );
    match &scene.agents.get(&subagent).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Read: /bar"));
        }
        other => panic!("subagent slot should be Active, got {other:?}"),
    }
}

/// Pre-existing behavior: with no active Task, hook events apply normally.
#[test]
fn hook_activity_without_active_task_applies_normally() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t".into()),
            detail: Some("Bash: ls".into()),
        },
        SystemTime::now(),
        Transport::Hook,
    );
    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Bash: ls"));
        }
        other => panic!("expected Active, got {other:?}"),
    }
}

#[test]
fn session_start_with_cwd_derives_label_from_basename() {
    // No more "cc#1" when the cwd tells us what project this is.
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/Users/me/Desktop/pixtuoid"),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&id).unwrap().label, "pixtuoid");
}

#[test]
fn session_start_without_cwd_falls_back_to_cc_label() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from(""),
            parent_id: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    assert_eq!(&*scene.agents.get(&id).unwrap().label, "cl#1");
}

#[test]
fn rename_updates_slot_label() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::Rename {
            agent_id: id,
            label: "feature-dev:code-explorer".into(),
        },
        SystemTime::now(),
        Transport::Jsonl,
    );
    assert_eq!(
        &*scene.agents.get(&id).unwrap().label,
        "feature-dev:code-explorer"
    );
}

#[test]
fn rename_for_unknown_agent_is_noop() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/missing.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::Rename {
            agent_id: id,
            label: "x".into(),
        },
        SystemTime::now(),
        Transport::Jsonl,
    );
    assert!(!scene.agents.contains_key(&id));
}

/// Regression guard: if a Hook PostToolUse arrives for a Task before its
/// JSONL ActivityStart (startup race where Pre was missed), the matching
/// JSONL ActivityEnd that always follows in the same transcript still drains
/// active_tasks. After the drain, normal hook events are no longer suppressed.
#[test]
fn active_tasks_drained_by_jsonl_end_even_if_hook_end_arrived_first() {
    use pixtuoid_core::source::ToolDetail;

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();

    // Hook PostToolUse arrives first (active_tasks empty — Pre was missed).
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("task-X".into()),
        },
        t0,
        Transport::Hook,
    );

    // JSONL ActivityStart for the same Task arrives after the hook dedup
    // window has expired — passes through and populates active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("task-X".into()),
            detail: Some(ToolDetail::Task),
        },
        t0 + Duration::from_millis(700),
        Transport::Jsonl,
    );

    // JSONL ActivityEnd from the same transcript drains active_tasks.
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("task-X".into()),
        },
        t0 + Duration::from_millis(800),
        Transport::Jsonl,
    );

    // Subsequent hook activity must apply normally — proves active_tasks drained.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("other".into()),
            detail: Some("Bash: ls".into()),
        },
        t0 + Duration::from_millis(900),
        Transport::Hook,
    );

    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(
                detail.as_deref(),
                Some("Bash: ls"),
                "active_tasks must drain so subsequent hook events apply"
            );
        }
        other => panic!("expected Active(Bash: ls), got {other:?}"),
    }
}

#[test]
fn jsonl_event_after_dedup_window_is_applied() {
    let mut scene = SceneState::uniform(2);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t-1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Reading,
            tool_use_id: Some("t-1".into()),
            detail: None,
        },
        t0 + Duration::from_millis(600),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(matches!(
        slot.state,
        ActivityState::Active {
            activity: Activity::Reading,
            ..
        }
    ));
}

// --- stale-agent sweep ---------------------------------------------------

#[test]
fn stale_idle_agent_is_marked_exiting_after_timeout() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stale.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_none());

    // Tick just before the threshold — should NOT mark exiting.
    reducer.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT - Duration::from_secs(1));
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "should not mark exiting before timeout"
    );

    // Tick past the threshold — should mark exiting.
    reducer.tick(&mut scene, t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(1));
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "should mark exiting after timeout"
    );
}

#[test]
fn stale_active_agent_uses_shorter_timeout_than_idle() {
    use pixtuoid_core::state::reducer::{STALE_ACTIVE_TIMEOUT, STALE_IDLE_TIMEOUT};
    assert!(
        STALE_ACTIVE_TIMEOUT < STALE_IDLE_TIMEOUT,
        "active timeout should be shorter than idle"
    );

    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    reducer.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    // Active timeout is 10 min — should mark exiting after that.
    reducer.tick(
        &mut scene,
        t0 + STALE_ACTIVE_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "active agent should be reaped after STALE_ACTIVE_TIMEOUT"
    );
}

#[test]
fn fresh_event_resets_stale_timer() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/fresh.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );

    // At 29 min (just before 30 min idle threshold), send a new event.
    let almost = t0 + STALE_IDLE_TIMEOUT - Duration::from_secs(60);
    reducer.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "perm".into(),
        },
        almost,
        Transport::Hook,
    );

    // Now tick at original t0 + 31 min — should NOT reap because
    // last_event_at was reset to `almost` (29 min mark).
    reducer.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_none(),
        "fresh event should have reset the stale timer"
    );
}

#[test]
fn unknown_cwd_agent_reaps_faster() {
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/ghost.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // SessionStart with empty cwd → label falls back to "cc#N".
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "s".into(),
            cwd: PathBuf::new(),
            parent_id: None,
        },
        t0,
        Transport::Jsonl,
    );
    let label = scene.agents.get(&id).unwrap().label.clone();
    assert!(
        label.contains('#'),
        "empty cwd should produce source#N label, got {label}"
    );

    // 3 min + 1s → should be reaped (STALE_UNKNOWN_CWD_TIMEOUT = 3 min).
    reducer.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "unknown-cwd agent should reap after STALE_UNKNOWN_CWD_TIMEOUT"
    );
}

#[test]
fn tool_call_count_increments_on_activity_start() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stats.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 0);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 1);

    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t2".into()),
            detail: None,
        },
        t0 + Duration::from_millis(600),
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 2);
}

#[test]
fn active_ms_accumulates_on_state_transitions() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().active_ms, 0);

    // End after 1 second, then tick past grace window to flush to Idle
    let t1 = t0 + Duration::from_secs(1);
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t1,
        Transport::Hook,
    );
    // active_ms not yet accumulated (happens on next ActivityStart or expire)
    r.tick(&mut scene, t1 + Duration::from_secs(3));
    let slot = scene.agents.get(&id).unwrap();
    assert!(
        slot.active_ms >= 1000,
        "expected >= 1000ms active, got {}",
        slot.active_ms
    );
}

#[test]
fn active_ms_does_not_double_count_on_duplicate_activity_end() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/dedup.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    let t1 = t0 + Duration::from_secs(2);
    // First ActivityEnd (hook)
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t1,
        Transport::Hook,
    );
    // Second ActivityEnd (late JSONL, past dedup window)
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t1 + Duration::from_millis(600),
        Transport::Jsonl,
    );

    // Flush to idle
    r.tick(&mut scene, t1 + Duration::from_secs(3));
    let slot = scene.agents.get(&id).unwrap();
    // Should be ~2-3s, not ~4-6s (double-counted)
    assert!(
        slot.active_ms < 5000,
        "active_ms looks double-counted: {}",
        slot.active_ms
    );
}

#[test]
fn active_ms_preserved_when_task_arrives_during_active_tool() {
    use pixtuoid_core::source::ToolDetail;

    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/task-active.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    // Tool starts
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    // 2 seconds later, Task arrives while still Active (within grace window)
    let t1 = t0 + Duration::from_secs(2);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("task-1".into()),
            detail: Some(ToolDetail::Task),
        },
        t1,
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(
        slot.active_ms >= 2000,
        "expected >= 2000ms active from pre-Task tool span, got {}",
        slot.active_ms
    );
}

#[test]
fn active_ms_preserved_when_waiting_interrupts_active() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/waiting.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    let t1 = t0 + Duration::from_secs(3);
    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t1,
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(
        slot.active_ms >= 3000,
        "expected >= 3000ms active before Waiting, got {}",
        slot.active_ms
    );
}

#[test]
fn session_end_cascades_to_children() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/parent.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/parent/subagents/agent-1.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "parent".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "child".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    assert!(scene.agents.get(&child).unwrap().exiting_at.is_none());

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: parent },
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "parent should be exiting"
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "child should cascade to exiting when parent ends"
    );
}

#[test]
fn session_end_cascades_to_grandchildren() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let grandparent = AgentId::from_transcript_path("/p/gp.jsonl");
    let parent = AgentId::from_parts("claude-code", "/p/gp/subagents/agent-p.jsonl");
    let child = AgentId::from_parts("claude-code", "/p/gp/subagents/agent-c.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: grandparent,
            source: "claude-code".into(),
            session_id: "gp".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(grandparent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child,
            source: "claude-code".into(),
            session_id: "c".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd {
            agent_id: grandparent,
        },
        t0 + Duration::from_secs(10),
        Transport::Hook,
    );
    assert!(
        scene.agents.get(&child).unwrap().exiting_at.is_some(),
        "grandchild should cascade to exiting via BFS"
    );
}

#[test]
fn unknown_cwd_agent_uses_faster_stale_timeout() {
    use pixtuoid_core::state::reducer::STALE_UNKNOWN_CWD_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/unknown.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "u".into(),
            cwd: PathBuf::new(),
            parent_id: None,
        },
        t0,
        Transport::Jsonl,
    );
    let slot = scene.agents.get(&id).unwrap();
    assert!(slot.unknown_cwd, "empty cwd should set unknown_cwd");

    r.tick(
        &mut scene,
        t0 + STALE_UNKNOWN_CWD_TIMEOUT + Duration::from_secs(1),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "unknown_cwd agent should reap after STALE_UNKNOWN_CWD_TIMEOUT"
    );
}

// --- parent-child cascade --------------------------------------------------

#[test]
fn session_end_cascade_marks_all_descendants_exiting() {
    let mut scene = SceneState::uniform(8);
    let mut r = Reducer::new();
    let parent = AgentId::from_transcript_path("/p/cascade-parent.jsonl");
    let child_a = AgentId::from_parts("claude-code", "/p/cascade-parent/subagents/agent-a.jsonl");
    let child_b = AgentId::from_parts("claude-code", "/p/cascade-parent/subagents/agent-b.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: parent,
            source: "claude-code".into(),
            session_id: "p".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child_a,
            source: "claude-code".into(),
            session_id: "ca".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(100),
        Transport::Jsonl,
    );
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: child_b,
            source: "claude-code".into(),
            session_id: "cb".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: Some(parent),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    assert!(scene.agents.get(&child_a).unwrap().exiting_at.is_none());
    assert!(scene.agents.get(&child_b).unwrap().exiting_at.is_none());

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: parent },
        t0 + Duration::from_secs(5),
        Transport::Hook,
    );

    assert!(
        scene.agents.get(&parent).unwrap().exiting_at.is_some(),
        "parent must be marked exiting"
    );
    assert!(
        scene.agents.get(&child_a).unwrap().exiting_at.is_some(),
        "child_a must cascade to exiting when parent ends"
    );
    assert!(
        scene.agents.get(&child_b).unwrap().exiting_at.is_some(),
        "child_b must cascade to exiting when parent ends"
    );
}

// --- hook-wins dedup -------------------------------------------------------

#[test]
fn hook_wins_dedup_drops_jsonl_duplicate_within_window() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/dedup-hw.jsonl");
    start(&mut r, &mut scene, id);

    let t0 = SystemTime::now();

    // Hook event first — establishes the tool_use_id in the dedup map.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("dedup-1".into()),
            detail: Some("Edit: hook.rs".into()),
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(scene.agents.get(&id).unwrap().tool_call_count, 1);

    // JSONL event with same tool_use_id within 500ms — must be dropped.
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Reading,
            tool_use_id: Some("dedup-1".into()),
            detail: Some("Edit: jsonl.rs".into()),
        },
        t0 + Duration::from_millis(200),
        Transport::Jsonl,
    );

    // tool_call_count should still be 1 — JSONL duplicate was dropped.
    assert_eq!(
        scene.agents.get(&id).unwrap().tool_call_count,
        1,
        "JSONL duplicate inside hook-wins window must be dropped"
    );
    // State should still reflect the hook event.
    match &scene.agents.get(&id).unwrap().state {
        ActivityState::Active { detail, .. } => {
            assert_eq!(detail.as_deref(), Some("Edit: hook.rs"));
        }
        other => panic!("expected Active from hook, got {other:?}"),
    }
}

// --- sweep_stale -----------------------------------------------------------

#[test]
fn sweep_stale_marks_old_agent_exiting_on_tick() {
    use pixtuoid_core::state::reducer::STALE_IDLE_TIMEOUT;
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/stale-sweep.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);

    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "sw".into(),
            cwd: PathBuf::from("/old-project"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert!(scene.agents.get(&id).unwrap().exiting_at.is_none());

    // Tick well past the idle stale timeout with no intervening events.
    r.tick(
        &mut scene,
        t0 + STALE_IDLE_TIMEOUT + Duration::from_secs(60),
    );
    assert!(
        scene.agents.get(&id).unwrap().exiting_at.is_some(),
        "tick past STALE_IDLE_TIMEOUT should mark agent exiting"
    );
}

/// With heterogeneous per-floor capacities, the third session should
/// overflow from floor 0 (cap=2) to floor 1's first desk (global index 2).
#[test]
fn session_start_overflows_to_floor1_with_heterogeneous_capacity() {
    let mut r = Reducer::new();
    let mut scene = SceneState::new([2, 4, 0, 0, 0]);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    for i in 0..3 {
        let id = AgentId::from_transcript_path(&format!("/proj/{i}.jsonl"));
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "cc".into(),
                session_id: format!("s{i}"),
                cwd: std::path::PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Jsonl,
        );
    }
    assert_eq!(scene.agents.len(), 3);
    let desks: Vec<usize> = scene.agents.values().map(|a| a.desk_index).collect();
    assert!(desks.contains(&0));
    assert!(desks.contains(&1));
    assert!(
        desks.contains(&2),
        "third agent should get desk 2 (floor 1)"
    );
    assert_eq!(scene.floor_of(2), 1);
}

#[test]
fn session_start_dropped_when_all_desks_occupied() {
    let mut r = Reducer::new();
    let mut scene = SceneState::new([2, 0, 0, 0, 0]);
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    for i in 0..2 {
        let id = AgentId::from_transcript_path(&format!("/proj/{i}.jsonl"));
        r.apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "cc".into(),
                session_id: format!("s{i}"),
                cwd: PathBuf::from("/repo"),
                parent_id: None,
            },
            t0,
            Transport::Hook,
        );
    }
    assert_eq!(scene.agents.len(), 2);
    assert!(scene.next_free_desk().is_none());

    let overflow_id = AgentId::from_transcript_path("/proj/overflow.jsonl");
    r.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: overflow_id,
            source: "cc".into(),
            session_id: "s-overflow".into(),
            cwd: PathBuf::from("/repo"),
            parent_id: None,
        },
        t0,
        Transport::Hook,
    );
    assert_eq!(
        scene.agents.len(),
        2,
        "third SessionStart must be silently dropped when desks are full"
    );
    assert!(
        !scene.agents.contains_key(&overflow_id),
        "overflow agent should not exist"
    );
}

#[test]
fn activity_end_while_waiting_does_not_arm_pending_idle() {
    let mut scene = SceneState::uniform(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/wait.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);

    start(&mut r, &mut scene, id);

    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        t0,
        Transport::Hook,
    );

    r.apply(
        &mut scene,
        AgentEvent::Waiting {
            agent_id: id,
            reason: "permission".into(),
        },
        t0 + Duration::from_millis(500),
        Transport::Hook,
    );
    assert!(matches!(
        scene.agents.get(&id).unwrap().state,
        ActivityState::Waiting { .. }
    ));

    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        t0 + Duration::from_millis(1000),
        Transport::Jsonl,
    );

    let slot = scene.agents.get(&id).unwrap();
    assert!(
        matches!(slot.state, ActivityState::Waiting { .. }),
        "state should remain Waiting, got {:?}",
        slot.state
    );
    assert!(
        slot.pending_idle_at.is_none(),
        "pending_idle_at must not be armed when state is Waiting"
    );
}
