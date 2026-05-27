use pixtuoid_core::source::antigravity;
use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{Activity, AgentEvent};
use pixtuoid_core::AgentId;

fn load(name: &str) -> serde_json::Value {
    let s = std::fs::read_to_string(format!("tests/fixtures/hooks/{name}.json")).unwrap();
    serde_json::from_str(&s).unwrap()
}

fn load_jsonl(name: &str) -> serde_json::Value {
    let s = std::fs::read_to_string(format!("tests/fixtures/jsonl/{name}.json")).unwrap();
    serde_json::from_str(&s).unwrap()
}

#[test]
fn decode_session_start() {
    let ev = decode_hook_payload(load("session_start")).unwrap();
    let expected_id = AgentId::from_transcript_path("/Users/me/.claude/projects/x/ses-abc.jsonl");
    match ev {
        AgentEvent::SessionStart {
            agent_id,
            session_id,
            source,
            ..
        } => {
            assert_eq!(agent_id, expected_id);
            assert_eq!(session_id, "ses-abc");
            assert_eq!(source, "claude-code");
        }
        other => panic!("expected SessionStart, got {other:?}"),
    }
}

#[test]
fn decode_session_start_with_custom_source() {
    let mut payload = load("session_start");
    payload["source"] = serde_json::Value::String("antigravity".into());
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::SessionStart { source, .. } => {
            assert_eq!(source, "antigravity");
        }
        other => panic!("expected SessionStart, got {other:?}"),
    }
}

#[test]
fn decode_pre_tool_use_write_maps_to_typing() {
    let ev = decode_hook_payload(load("pre_tool_use_write")).unwrap();
    match ev {
        AgentEvent::ActivityStart {
            activity, detail, ..
        } => {
            assert_eq!(activity, Activity::Typing);
            assert!(detail.unwrap().display().contains("Write"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_post_tool_use_is_activity_end() {
    let ev = decode_hook_payload(load("post_tool_use_write")).unwrap();
    assert!(matches!(ev, AgentEvent::ActivityEnd { .. }));
}

#[test]
fn decode_notification_is_waiting() {
    let ev = decode_hook_payload(load("notification")).unwrap();
    match ev {
        AgentEvent::Waiting { reason, .. } => assert!(reason.contains("permission")),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_session_end() {
    let ev = decode_hook_payload(load("session_end")).unwrap();
    assert!(matches!(ev, AgentEvent::SessionEnd { .. }));
}

#[test]
fn decode_unknown_event_returns_err() {
    let mut bad = load("session_start");
    bad["hook_event_name"] = serde_json::Value::String("UnknownThing".into());
    assert!(decode_hook_payload(bad).is_err());
}

#[test]
fn cc_jsonl_assistant_tool_use_is_activity_start() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let events =
        decode_cc_line(transcript, "claude-code", load_jsonl("assistant_tool_use")).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityStart {
            activity,
            tool_use_id,
            detail,
            ..
        } => {
            assert_eq!(*activity, Activity::Typing);
            assert_eq!(tool_use_id.as_deref(), Some("tu_123"));
            assert!(detail.as_ref().unwrap().display().contains("Write"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn cc_jsonl_tool_result_is_activity_end() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let events = decode_cc_line(transcript, "claude-code", load_jsonl("tool_result")).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityEnd { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("tu_123"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_hook_payload_with_multibyte_tool_input_does_not_panic() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-zh",
        "transcript_path": "/tmp/zh.jsonl",
        "cwd": "/tmp",
        "tool_name": "Bash",
        "tool_input": {
            "command": "echo 这是一个非常长的中文命令需要被截断这是一个非常长的中文命令需要被截断"
        }
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityStart { detail, .. } => {
            let d = detail.expect("detail set");
            assert!(d.display().contains("Bash"), "got: {}", d.display());
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}

#[test]
fn decode_pre_tool_use_carries_tool_use_id_from_payload() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/Users/me/.claude/projects/x/ses-abc.jsonl",
        "cwd": "/repo",
        "tool_name": "Task",
        "tool_use_id": "toolu_01ABC",
        "tool_input": { "description": "go" }
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityStart {
            tool_use_id,
            detail,
            ..
        } => {
            assert_eq!(tool_use_id.as_deref(), Some("toolu_01ABC"));
            assert!(detail.expect("detail set").is_task());
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn decode_post_tool_use_carries_tool_use_id_from_payload() {
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/Users/me/.claude/projects/x/ses-abc.jsonl",
        "cwd": "/repo",
        "tool_name": "Task",
        "tool_use_id": "toolu_01ABC"
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityEnd { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("toolu_01ABC"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn cc_jsonl_subagent_line_with_attribution_emits_rename() {
    let transcript = "/Users/me/.claude/projects/x/sess/subagents/agent-abc.jsonl";
    let v = serde_json::json!({
        "type": "assistant",
        "sessionId": "sess",
        "cwd": "/repo",
        "attributionAgent": "feature-dev:code-explorer",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Read",
                  "input": { "file_path": "/repo/src/a.rs" } }
            ]
        }
    });
    let events = decode_cc_line(transcript, "claude-code", v).unwrap();
    let has_rename = events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::Rename { label, .. } if label == "code-explorer"
        )
    });
    assert!(has_rename, "expected Rename event, got {events:?}");
}

#[test]
fn cc_jsonl_plain_user_message_yields_no_events() {
    let transcript = "/Users/me/.claude/projects/x/ses-abc.jsonl";
    let events = decode_cc_line(transcript, "claude-code", load_jsonl("user_message")).unwrap();
    assert!(events.is_empty());
}

#[test]
fn ag_planner_response_emits_activity_start_with_indexed_tool_use_id() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = serde_json::json!({
        "step_index": 2,
        "source": "MODEL",
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            { "name": "list_dir", "args": { "DirectoryPath": "\"/repo/src\"" } },
            { "name": "read_file", "args": { "AbsolutePath": "\"/repo/README.md\"" } }
        ]
    });
    let events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    assert_eq!(events.len(), 2);
    match &events[0] {
        AgentEvent::ActivityStart { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-2-0"));
        }
        other => panic!("got {other:?}"),
    }
    match &events[1] {
        AgentEvent::ActivityStart { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-2-1"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn ag_tool_result_emits_activity_end() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";
    let v = serde_json::json!({
        "step_index": 3,
        "type": "LIST_DIRECTORY",
        "content": "output"
    });
    let events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::ActivityEnd { tool_use_id, .. } => {
            assert_eq!(tool_use_id.as_deref(), Some("ag-2-0"));
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn ag_uses_source_namespaced_agent_id() {
    let transcript = "/shared/path.jsonl";
    let v = serde_json::json!({ "step_index": 1, "type": "PLANNER_RESPONSE", "tool_calls": [] });
    let _events = antigravity::decode_ag_line(transcript, "antigravity", v).unwrap();
    let ag_id = AgentId::from_parts("antigravity", transcript);
    let cc_id = AgentId::from_parts("claude-code", transcript);
    assert_ne!(
        ag_id, cc_id,
        "different sources must produce different AgentIds"
    );
}

#[test]
fn ag_ask_permission_and_question_emits_waiting() {
    let transcript = "/Users/me/.gemini/antigravity-cli/brain/sess/transcript.jsonl";

    // ask_permission tool call
    let v_perm = serde_json::json!({
        "step_index": 4,
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            { "name": "ask_permission", "args": { "Reason": "read a file" } }
        ]
    });
    let events_perm = antigravity::decode_ag_line(transcript, "antigravity", v_perm).unwrap();
    assert_eq!(events_perm.len(), 1);
    match &events_perm[0] {
        AgentEvent::Waiting { reason, .. } => {
            assert_eq!(reason, "asking permission");
        }
        other => panic!("expected Waiting, got {other:?}"),
    }

    // ask_question tool call
    let v_quest = serde_json::json!({
        "step_index": 5,
        "type": "PLANNER_RESPONSE",
        "tool_calls": [
            { "name": "ask_question", "args": { "questions": [] } }
        ]
    });
    let events_quest = antigravity::decode_ag_line(transcript, "antigravity", v_quest).unwrap();
    assert_eq!(events_quest.len(), 1);
    match &events_quest[0] {
        AgentEvent::Waiting { reason, .. } => {
            assert_eq!(reason, "asking permission");
        }
        other => panic!("expected Waiting, got {other:?}"),
    }
}

#[test]
fn cc_session_ended_detects_session_end_subtype() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"assistant","message":{"role":"assistant","content":[]}}
{"type":"system","subtype":"session_end","sessionId":"s1"}
"#;
    assert!(cc_session_ended(tail));
}

#[test]
fn cc_session_ended_returns_false_for_active_session() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"assistant","message":{"role":"assistant","content":[]}}
"#;
    assert!(!cc_session_ended(tail));
}

#[test]
fn cc_session_ended_ignores_string_content_containing_session_end() {
    use pixtuoid_core::source::claude_code::cc_session_ended;
    let tail = br#"{"type":"system","subtype":"session_start","sessionId":"s1"}
{"type":"user","message":{"content":[{"type":"tool_result","output":"cat session_end.sh"}]}}
"#;
    assert!(
        !cc_session_ended(tail),
        "should not false-positive on session_end inside tool output"
    );
}

#[test]
fn detect_parent_id_for_subagent_path() {
    use pixtuoid_core::AgentId;
    use std::path::Path;

    // Simulate what jsonl.rs::detect_parent_id does
    let path = Path::new("/Users/me/.claude/projects/x/abc123/subagents/agent-1.jsonl");
    let path_str = path.to_string_lossy();
    let idx = path_str.find("/subagents/");
    assert!(idx.is_some(), "should find /subagents/ in path");
    let parent_dir = &path_str[..idx.unwrap()];
    let parent_jsonl = format!("{parent_dir}.jsonl");
    let parent_id = AgentId::from_parts("claude-code", &parent_jsonl);
    let expected = AgentId::from_parts("claude-code", "/Users/me/.claude/projects/x/abc123.jsonl");
    assert_eq!(parent_id, expected);
}

#[test]
fn detect_parent_id_returns_none_for_regular_path() {
    let path = std::path::Path::new("/Users/me/.claude/projects/x/ses-abc.jsonl");
    let path_str = path.to_string_lossy();
    assert!(
        path_str.find("/subagents/").is_none(),
        "regular path should not match subagent pattern"
    );
}

#[test]
fn decode_hook_payload_missing_session_id_returns_err() {
    let payload = serde_json::json!({
        "hook_event_name": "SessionStart",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/repo"
    });
    assert!(
        decode_hook_payload(payload).is_err(),
        "missing session_id must return Err"
    );
}

#[test]
fn decode_hook_payload_missing_transcript_path_returns_err() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "cwd": "/repo",
        "tool_name": "Bash",
        "tool_input": { "command": "ls" }
    });
    assert!(
        decode_hook_payload(payload).is_err(),
        "missing transcript_path must return Err"
    );
}

#[test]
fn decode_hook_payload_missing_tool_name_still_succeeds() {
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "ses-abc",
        "transcript_path": "/tmp/t.jsonl"
    });
    let ev = decode_hook_payload(payload).unwrap();
    match ev {
        AgentEvent::ActivityStart { detail, .. } => {
            let d = detail.expect("detail set");
            assert!(
                d.display().contains("?"),
                "missing tool_name should fall back to '?'"
            );
        }
        other => panic!("expected ActivityStart, got {other:?}"),
    }
}
