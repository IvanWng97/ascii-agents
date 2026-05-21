use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ascii_agents_core::source::{Activity, AgentEvent};
use ascii_agents_core::state::reducer::{Reducer, Transport};
use ascii_agents_core::state::{ActivityState, SceneState};
use ascii_agents_core::AgentId;

fn start(reducer: &mut Reducer, scene: &mut SceneState, id: AgentId) {
    reducer.apply(
        scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/"),
        },
        SystemTime::now(),
        Transport::Hook,
    );
}

#[test]
fn session_start_creates_idle_slot_at_first_free_desk() {
    let mut scene = SceneState::new(4);
    let mut reducer = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");

    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/repo"),
        },
        SystemTime::now(),
        Transport::Hook,
    );

    let slot = scene.agents.get(&id).expect("agent inserted");
    assert_eq!(slot.desk_index, 0);
    assert_eq!(slot.label, "cc#1");
    assert_eq!(slot.state, ActivityState::Idle);
}

#[test]
fn activity_start_sets_state_active() {
    let mut scene = SceneState::new(4);
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
fn activity_end_returns_to_idle() {
    let mut scene = SceneState::new(4);
    let mut r = Reducer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");
    start(&mut r, &mut scene, id);
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: Some("t1".into()),
            detail: None,
        },
        SystemTime::now(),
        Transport::Hook,
    );
    r.apply(
        &mut scene,
        AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: Some("t1".into()),
        },
        SystemTime::now(),
        Transport::Hook,
    );

    assert_eq!(scene.agents.get(&id).unwrap().state, ActivityState::Idle);
}

#[test]
fn waiting_sets_state_with_reason() {
    let mut scene = SceneState::new(4);
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
        ActivityState::Waiting { reason } => assert_eq!(reason, "Bash: rm -rf?"),
        other => panic!("unexpected state: {other:?}"),
    }
}

#[test]
fn session_end_removes_slot_and_frees_desk() {
    let mut scene = SceneState::new(2);
    let mut r = Reducer::new();
    let a = AgentId::from_transcript_path("/p/a.jsonl");
    let b = AgentId::from_transcript_path("/p/b.jsonl");
    start(&mut r, &mut scene, a);
    start(&mut r, &mut scene, b);

    r.apply(
        &mut scene,
        AgentEvent::SessionEnd { agent_id: a },
        SystemTime::now(),
        Transport::Hook,
    );

    assert!(!scene.agents.contains_key(&a));
    assert_eq!(scene.next_free_desk(), Some(0));
}

#[test]
fn jsonl_duplicate_of_recent_hook_is_dropped() {
    let mut scene = SceneState::new(2);
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

    let detail_marker = Some("FROM_JSONL".to_string());
    r.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Reading,
            tool_use_id: Some("t-1".into()),
            detail: detail_marker.clone(),
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
            assert_ne!(*detail, detail_marker, "jsonl detail must not overwrite");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn jsonl_event_after_dedup_window_is_applied() {
    let mut scene = SceneState::new(2);
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
