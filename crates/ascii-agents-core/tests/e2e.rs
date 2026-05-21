#![cfg(feature = "test-renderer")]

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ascii_agents_core::render::test_renderer::TestRenderer;
use ascii_agents_core::source::Activity;
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentEvent, AgentId, Reducer, Renderer, SceneState, Transport};

#[test]
fn scripted_timeline_drives_scene_through_states() {
    let mut scene = SceneState::new(4);
    let mut reducer = Reducer::new();
    let mut renderer = TestRenderer::new();
    let id = AgentId::from_transcript_path("/p/a.jsonl");

    let mut now = SystemTime::now();
    let mut step =
        |events: Vec<AgentEvent>,
         dt_ms: u64,
         r: &mut Reducer,
         s: &mut SceneState,
         render: &mut TestRenderer| {
            for ev in events {
                r.apply(s, ev, now, Transport::Hook);
            }
            render.render(s).unwrap();
            now += Duration::from_millis(dt_ms);
        };

    step(
        vec![AgentEvent::SessionStart {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/repo"),
        }],
        10,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    step(
        vec![AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: None,
            detail: Some("Bash: ls".into()),
        }],
        200,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    step(
        vec![AgentEvent::ActivityEnd {
            agent_id: id,
            tool_use_id: None,
        }],
        50,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    step(
        vec![AgentEvent::Waiting {
            agent_id: id,
            reason: "permission?".into(),
        }],
        50,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    step(
        vec![AgentEvent::SessionEnd { agent_id: id }],
        10,
        &mut reducer,
        &mut scene,
        &mut renderer,
    );

    let snaps = renderer.snapshots.lock().unwrap();
    assert_eq!(snaps.len(), 5);
    assert_eq!(snaps[0].agents.get(&id).unwrap().state, ActivityState::Idle);
    assert!(matches!(
        snaps[1].agents.get(&id).unwrap().state,
        ActivityState::Active { .. }
    ));
    assert_eq!(snaps[2].agents.get(&id).unwrap().state, ActivityState::Idle);
    assert!(matches!(
        snaps[3].agents.get(&id).unwrap().state,
        ActivityState::Waiting { .. }
    ));
    assert!(!snaps[4].agents.contains_key(&id));
}
