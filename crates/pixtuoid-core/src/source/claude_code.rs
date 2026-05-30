use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::source::decoder::{describe_tool_target, make_tool_detail};
use crate::source::hook::HookSocketListener;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "claude-code";

pub struct ClaudeCodeSource {
    pub socket_path: PathBuf,
    pub projects_root: PathBuf,
}

impl ClaudeCodeSource {
    pub fn default_socket_path() -> PathBuf {
        if let Ok(p) = std::env::var("PIXTUOID_SOCKET") {
            return PathBuf::from(p);
        }
        if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
            return PathBuf::from(format!("{dir}/pixtuoid.sock"));
        }
        // SAFETY: getuid() is a trivial syscall with no pointer args; cannot fail.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/pixtuoid-{uid}.sock"))
    }

    pub fn default_paths() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Self {
            socket_path: Self::default_socket_path(),
            projects_root: PathBuf::from(format!("{home}/.claude/projects")),
        }
    }
}

#[async_trait]
impl Source for ClaudeCodeSource {
    fn name(&self) -> &str {
        "claude-code"
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let socket = HookSocketListener::bind(self.socket_path.clone()).await?;
        let watcher = JsonlWatcher::new(
            self.projects_root.clone(),
            SOURCE_NAME.to_string(),
            decode_cc_line,
            cc_derive_label,
            cc_session_ended,
        );

        let tx_hook = tx.clone();
        let tx_jsonl = tx.clone();
        let hook_task = tokio::spawn(async move { socket.run(tx_hook).await });
        let jsonl_task = tokio::spawn(async move { watcher.run(tx_jsonl).await });

        let hook_abort = hook_task.abort_handle();
        let jsonl_abort = jsonl_task.abort_handle();

        let inner: Result<()> = tokio::select! {
            r = hook_task => {
                tracing::warn!("hook listener exited first; aborting jsonl watcher");
                jsonl_abort.abort();
                r?
            }
            r = jsonl_task => {
                tracing::warn!("jsonl watcher exited first; aborting hook listener");
                hook_abort.abort();
                r?
            }
        };
        inner
    }
}

/// Decode one CC JSONL transcript line into 0..N AgentEvents.
pub fn decode_cc_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let agent_id = AgentId::from_parts(source, transcript_path);
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };

    let mut out = Vec::new();
    let ty = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");

    if let Some(name) = obj.get("attributionAgent").and_then(|v| v.as_str()) {
        let label = name.rsplit(':').next().unwrap_or(name).to_string();
        out.push(AgentEvent::Rename { agent_id, label });
    }

    let Some(message) = obj.get("message").and_then(|m| m.as_object()) else {
        return Ok(out);
    };
    let content = message.get("content");
    match (ty, content) {
        ("assistant", Some(Value::Array(blocks))) => {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                if btype != "tool_use" {
                    continue;
                }
                let id = bobj.get("id").and_then(|s| s.as_str()).map(String::from);
                let name = bobj.get("name").and_then(|s| s.as_str()).unwrap_or("?");
                let input = bobj.get("input");
                let target = describe_tool_target(name, input);
                out.push(AgentEvent::ActivityStart {
                    agent_id,
                    activity: Activity::Typing,
                    tool_use_id: id,
                    detail: Some(make_tool_detail(name, target)),
                });
            }
        }
        ("user", Some(Value::Array(blocks))) => {
            for block in blocks {
                let Some(bobj) = block.as_object() else {
                    continue;
                };
                let btype = bobj.get("type").and_then(|s| s.as_str()).unwrap_or("");
                if btype != "tool_result" {
                    continue;
                }
                let id = bobj
                    .get("tool_use_id")
                    .and_then(|s| s.as_str())
                    .map(String::from);
                out.push(AgentEvent::ActivityEnd {
                    agent_id,
                    tool_use_id: id,
                });
            }
        }
        // CC writes no `session_end` line on a clean `/exit`; it logs the
        // slash command as a string-valued user message. Treat `/exit`+`/quit`
        // as a durable SessionEnd so the JSONL transport reaps the session even
        // when the best-effort SessionEnd hook is dropped (the hook races CC's
        // own teardown and has no retry). See `is_exit_command`.
        ("user", Some(Value::String(s))) if is_exit_command(s) => {
            out.push(AgentEvent::SessionEnd { agent_id });
        }
        _ => {}
    }
    Ok(out)
}

/// True if a CC user-message content string is a session-terminating slash
/// command (`/exit` or `/quit`). CC logs slash commands as a `<command-name>`
/// wrapper. Only the two that actually end the session count — `/clear` and
/// `/compact` keep it alive, and prose merely mentioning `/exit` is not wrapped.
fn is_exit_command(content: &str) -> bool {
    content.contains("<command-name>/exit</command-name>")
        || content.contains("<command-name>/quit</command-name>")
}

/// CC session-end checker: parses lines as JSON and checks for
/// session lifecycle markers structurally (not byte scan).
pub fn cc_session_ended(tail: &[u8]) -> bool {
    let mut last_is_end = false;
    for line in tail.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
        let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
        let hook = v
            .get("hook_event_name")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if subtype == "session_start" {
            last_is_end = false;
        }
        if subtype == "session_end" || hook == "SessionEnd" {
            last_is_end = true;
        }
        // A `/exit` or `/quit` user event ends the session too (CC writes no
        // `session_end` line for it) — without this, a recently-exited session
        // re-ghosts on restart within the mtime window. Same matcher as the
        // live decode path so the two transports agree.
        if v.get("type").and_then(|s| s.as_str()) == Some("user") {
            if let Some(c) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                if is_exit_command(c) {
                    last_is_end = true;
                }
            }
        }
    }
    last_is_end
}

/// CC label: subagent paths → "subagent", otherwise "cc·" + cwd basename.
pub fn cc_derive_label(path: &Path, _source: &str, cwd: &Path) -> String {
    let is_subagent = path.to_string_lossy().contains("subagents");
    if is_subagent {
        "subagent".to_string()
    } else if cwd != Path::new("") && cwd != Path::new("/") {
        let base = cwd.file_name().and_then(|n| n.to_str()).unwrap_or("cc");
        format!("cc·{base}")
    } else {
        "cc".to_string()
    }
}
