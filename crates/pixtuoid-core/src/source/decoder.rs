//! Shared decoder utilities used by per-source decoders (CC, Antigravity).
//! Hook payload decoding lives here because the hook socket is shared.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::source::{Activity, AgentEvent, ToolDetail};
use crate::AgentId;

pub fn decode_hook_payload(v: Value) -> Result<AgentEvent> {
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("hook payload must be an object"))?;
    let raw_event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing hook_event_name"))?;
    let event = normalize_hook_event(raw_event);

    let session_id = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing session_id"))?
        .to_string();
    let transcript_path = obj
        .get("agent_transcript_path")
        .or_else(|| obj.get("transcript_path"))
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing transcript_path"))?;
    let source = obj
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or_else(|| infer_hook_source(transcript_path, raw_event));
    let agent_id = AgentId::from_parts(source, transcript_path);

    match event {
        "SessionStart" | "SubagentStart" => {
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            let source = source.to_string();
            let parent_id = if event == "SubagentStart" {
                obj.get("transcript_path")
                    .and_then(|s| s.as_str())
                    .map(|path| AgentId::from_parts(&source, path))
            } else {
                None
            };
            Ok(AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id,
            })
        }
        "PreToolUse" => {
            let tool_name = obj.get("tool_name").and_then(|s| s.as_str()).unwrap_or("?");
            let target = describe_tool_target(tool_name, obj.get("tool_input"));
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            Ok(AgentEvent::ActivityStart {
                agent_id,
                activity: Activity::Typing,
                tool_use_id,
                detail: Some(make_tool_detail(tool_name, target)),
            })
        }
        "PostToolUse" => {
            let tool_use_id = obj
                .get("tool_use_id")
                .and_then(|s| s.as_str())
                .map(String::from);
            Ok(AgentEvent::ActivityEnd {
                agent_id,
                tool_use_id,
            })
        }
        "Notification" => {
            let msg = obj
                .get("message")
                .and_then(|s| s.as_str())
                .unwrap_or("waiting");
            Ok(AgentEvent::Waiting {
                agent_id,
                reason: msg.into(),
            })
        }
        "SessionEnd" | "Stop" | "SubagentStop" => Ok(AgentEvent::SessionEnd { agent_id }),
        "UserPromptSubmit" => Ok(AgentEvent::Waiting {
            agent_id,
            reason: "prompt submitted".into(),
        }),
        other => bail!("unsupported hook_event_name: {other}"),
    }
}

fn normalize_hook_event(event: &str) -> &str {
    match event {
        "session_start" => "SessionStart",
        "pre_tool_use" => "PreToolUse",
        "post_tool_use" => "PostToolUse",
        "notification" => "Notification",
        "session_end" => "SessionEnd",
        "stop" => "Stop",
        "user_prompt_submit" => "UserPromptSubmit",
        "subagent_start" => "SubagentStart",
        "subagent_stop" => "SubagentStop",
        other => other,
    }
}

fn infer_hook_source(transcript_path: &str, event: &str) -> &'static str {
    if transcript_path.contains("/.codex/")
        || matches!(
            normalize_hook_event(event),
            "Stop" | "UserPromptSubmit" | "SubagentStart" | "SubagentStop"
        )
    {
        "codex"
    } else {
        crate::source::claude_code::SOURCE_NAME
    }
}

pub(crate) fn make_tool_detail(tool_name: &str, target: String) -> ToolDetail {
    if tool_name == "Task" {
        ToolDetail::Task
    } else {
        ToolDetail::Generic {
            display: format!("{tool_name}{target}"),
        }
    }
}

pub(crate) fn describe_tool_target(tool: &str, input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let key = match tool {
        "Write" | "Edit" | "MultiEdit" | "Read" => "file_path",
        "Bash" => "command",
        "Grep" | "Glob" => "pattern",
        _ => "",
    };
    if key.is_empty() {
        return String::new();
    }
    let Some(s) = input.get(key).and_then(|v| v.as_str()) else {
        return String::new();
    };
    let total_chars = s.chars().count();
    let mut s: String = s.chars().take(40).collect();
    if total_chars > 40 {
        s.push('…');
    }
    format!(": {s}")
}
