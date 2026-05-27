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
    let event = obj
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing hook_event_name"))?;

    let session_id = obj
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing session_id"))?
        .to_string();
    let transcript_path = obj
        .get("transcript_path")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing transcript_path"))?;
    let source = obj
        .get("source")
        .and_then(|s| s.as_str())
        .unwrap_or(crate::source::claude_code::SOURCE_NAME);
    let agent_id = AgentId::from_parts(source, transcript_path);

    match event {
        "SessionStart" => {
            let cwd = obj.get("cwd").and_then(|s| s.as_str()).unwrap_or("").into();
            let source = source.to_string();
            Ok(AgentEvent::SessionStart {
                agent_id,
                source,
                session_id,
                cwd,
                parent_id: None,
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
        "SessionEnd" => Ok(AgentEvent::SessionEnd { agent_id }),
        other => bail!("unsupported hook_event_name: {other}"),
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
