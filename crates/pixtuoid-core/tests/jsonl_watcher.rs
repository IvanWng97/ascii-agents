use std::time::{Duration, SystemTime};

use filetime::{set_file_mtime, FileTime};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use pixtuoid_core::source::claude_code::{cc_derive_label, cc_session_ended, decode_cc_line};
use pixtuoid_core::source::jsonl::JsonlWatcher;
use pixtuoid_core::source::AgentEvent;
use pixtuoid_core::source::Transport;
use pixtuoid_core::AgentId;

fn cc_watcher(root: std::path::PathBuf) -> JsonlWatcher {
    JsonlWatcher::new(
        root,
        "claude-code".to_string(),
        decode_cc_line,
        cc_derive_label,
        cc_session_ended,
    )
}

#[tokio::test]
async fn watcher_emits_session_start_then_activity_for_tool_use() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-x");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-abc",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    let assistant_line = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-abc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    f.write_all(format!("{assistant_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_start = false;
    let mut got_activity = false;
    let mut start_transport = Transport::Hook;
    let mut activity_transport = Transport::Hook;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((t, AgentEvent::SessionStart { .. }))) => {
                got_start = true;
                start_transport = t;
            }
            Ok(Some((t, AgentEvent::ActivityStart { .. }))) => {
                got_activity = true;
                activity_transport = t;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
        if got_start && got_activity {
            break;
        }
    }
    assert!(got_start, "expected SessionStart from JSONL watcher");
    assert!(got_activity, "expected ActivityStart from JSONL watcher");
    assert_eq!(start_transport, Transport::Jsonl);
    assert_eq!(activity_transport, Transport::Jsonl);
    handle.abort();
}

#[tokio::test]
async fn watcher_does_not_consume_partial_trailing_line() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-x");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // First write: a complete line + a partial line (no trailing \n).
    let complete = serde_json::json!({
        "type": "assistant",
        "sessionId": "ses-abc",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "Bash", "input": { "command": "ls" } }
            ]
        }
    });
    let partial_head = r#"{"type":"assistant","sessionId":"ses-abc","cwd":"/repo","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_2","name":"Read","input":{"file_path":"/etc/host"#;
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(format!("{complete}\n{partial_head}").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    // We should see the SessionStart + ActivityStart for tu_1, but NOT for tu_2.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut seen_tool_use_ids: Vec<String> = Vec::new();
    while let Ok(Some((_t, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
    {
        if let AgentEvent::ActivityStart {
            tool_use_id: Some(id),
            ..
        } = ev
        {
            seen_tool_use_ids.push(id);
        }
    }
    assert!(
        seen_tool_use_ids.contains(&"tu_1".to_string()),
        "expected tu_1 from complete line, got {seen_tool_use_ids:?}"
    );
    assert!(
        !seen_tool_use_ids.contains(&"tu_2".to_string()),
        "tu_2 came from a partial line and must not be emitted yet"
    );

    // Now complete tu_2 by appending the rest of the line. tu_2 should appear.
    let partial_tail = "s\"}}]}}\n";
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    f.write_all(partial_tail.as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut got_tu_2 = false;
    while let Ok(Some((_t, ev))) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
    {
        if let AgentEvent::ActivityStart { tool_use_id, .. } = ev {
            if tool_use_id.as_deref() == Some("tu_2") {
                got_tu_2 = true;
            }
        }
    }
    assert!(
        got_tu_2,
        "tu_2 should appear after partial line is completed"
    );

    handle.abort();
}

/// On startup, the watcher must NOT emit SessionStart for every historical
/// .jsonl on disk. With small `max_desks` this would saturate desks with
/// long-dead sessions and starve the user's currently-active session.
/// Files older than the initial-window are seeded with cursor=file_len and
/// left out of the SessionStart stream until they next get written to.
#[tokio::test]
async fn watcher_skips_session_start_for_stale_files_on_startup() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-stale");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    // Pre-existing stale transcript (mtime backdated 1 hour).
    let stale = project_dir.join("old.jsonl");
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "old",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_old", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    tokio::fs::write(&stale, format!("{line}\n")).await.unwrap();
    let backdated = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600));
    set_file_mtime(&stale, backdated).unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(60));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    // Give the initial scan a moment to run.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut events = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(ev);
    }
    assert!(
        events.is_empty(),
        "stale file must not produce events on startup, got {events:?}"
    );
    handle.abort();
}

/// Conversely, a transcript whose mtime is *within* the initial-window is
/// treated as live: its SessionStart and any historical content replays so
/// in-flight Task / tool state survives a pixtuoid restart.
#[tokio::test]
async fn watcher_emits_session_start_for_recent_files_on_startup() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-fresh");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let fresh = project_dir.join("fresh.jsonl");
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "fresh",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_fresh", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    std::fs::write(&fresh, format!("{line}\n")).unwrap();
    // fsync the parent directory so the directory entry is guaranteed visible
    // to read_dir — without this, APFS metadata propagation can race with
    // the watcher's initial seed walk under heavy concurrent I/O.
    std::fs::File::open(&project_dir)
        .unwrap()
        .sync_all()
        .unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(3600));
    let fresh_path = fresh.clone();
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    // Give the watcher task a chance to complete initial_seed_walk, then
    // append a no-op newline to trigger FSEvents as a fallback path in case
    // the initial seed missed the file under heavy I/O contention.
    tokio::time::sleep(Duration::from_millis(500)).await;
    tokio::fs::OpenOptions::new()
        .append(true)
        .open(&fresh_path)
        .await
        .unwrap()
        .sync_all()
        .await
        .unwrap();

    let mut got_start = false;
    let mut got_activity = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some((_, AgentEvent::SessionStart { .. }))) => got_start = true,
            Ok(Some((_, AgentEvent::ActivityStart { .. }))) => got_activity = true,
            _ => {}
        }
        if got_start && got_activity {
            break;
        }
    }
    assert!(got_start, "fresh file should produce SessionStart");
    assert!(got_activity, "fresh file content should be replayed");
    handle.abort();
}

/// First-sight cwd extraction must scan past unparsable prefix lines.
/// `extract_cwd` previously short-circuited via `?` on the first non-JSON
/// (or non-UTF8) line, even if a later line carried the `cwd` field.
#[tokio::test]
async fn first_sight_extracts_cwd_past_non_json_prefix() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-cwd");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-cwd.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // First line: garbage. Second line: a system line carrying cwd. Watcher
    // should still derive cwd = /real-repo on the SessionStart for first-sight.
    //
    // The watcher emits SessionStart exactly ONCE per file, with cwd taken from
    // whatever bytes are present at first read. Writing the lines incrementally
    // (or even create-then-write) leaves a window where the 250ms poll observes
    // a partial/empty file, latches cwd="" permanently, and fails this test
    // (flaky under load / coverage instrumentation). Stage the complete content
    // in a sibling `.partial` file — excluded by the watcher's `.jsonl`
    // extension filter — then atomically rename it into place so first sight
    // always reads the full content.
    let sys_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-cwd",
        "cwd": "/real-repo"
    });
    let content = format!("not-json-prefix\n{sys_line}\n");
    let staging = project_dir.join("ses-cwd.jsonl.partial");
    tokio::fs::write(&staging, content.as_bytes())
        .await
        .unwrap();
    tokio::fs::rename(&staging, &transcript).await.unwrap();

    let mut found_cwd = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { cwd, .. }))) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            found_cwd = Some(cwd);
            break;
        }
    }
    assert_eq!(
        found_cwd,
        Some(std::path::PathBuf::from("/real-repo")),
        "extract_cwd must scan past non-JSON lines to find cwd"
    );
    handle.abort();
}

/// Stale files become live as soon as CC writes to them — the next notify
/// event must produce a SessionStart, since the file is now active.
#[tokio::test]
async fn stale_file_emits_session_start_when_written_to() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-revive");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let revived = project_dir.join("revive.jsonl");
    tokio::fs::write(&revived, "{}\n").await.unwrap();
    set_file_mtime(
        &revived,
        FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600)),
    )
    .unwrap();

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(60));
    let handle = tokio::spawn(async move { watcher.run(tx).await });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // No SessionStart yet (stale + skipped).
    while tokio::time::timeout(Duration::from_millis(20), rx.recv())
        .await
        .is_ok()
    {}

    // Append a real assistant tool_use line — file is now live.
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "revive",
        "cwd": "/repo",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "tu_new", "name": "Bash",
                  "input": { "command": "ls" } }
            ]
        }
    });
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&revived)
        .await
        .unwrap();
    f.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { .. }))) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            got_start = true;
            break;
        }
    }
    assert!(got_start, "appending to a stale file should bring it live");
    handle.abort();
}

/// A recent file (within the initial window) that has a session_end marker
/// at its tail must NOT produce a SessionStart on startup — the watcher
/// must detect the ended session and seed the cursor at EOF.
#[tokio::test]
async fn watcher_skips_recent_file_with_session_end_marker() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-ended");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let ended = project_dir.join("ended.jsonl");
    let content = r#"{"type":"system","subtype":"session_start","sessionId":"ended","cwd":"/repo"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}]}}
{"type":"system","subtype":"session_end","sessionId":"ended"}
"#;
    tokio::fs::write(&ended, content).await.unwrap();
    // mtime is "now" — well within the initial window.

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(projects_root.clone()).with_initial_window(Duration::from_secs(3600));
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut events = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
        events.push(ev);
    }
    let has_session_start = events
        .iter()
        .any(|(_, ev)| matches!(ev, AgentEvent::SessionStart { .. }));
    assert!(
        !has_session_start,
        "recent file with session_end marker must not produce SessionStart, got {events:?}"
    );
    handle.abort();
}

fn custom_label(_path: &std::path::Path, _source: &str, _cwd: &std::path::Path) -> String {
    "custom-label-ok".to_string()
}

#[tokio::test]
async fn watcher_custom_label_deriver() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-y");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-xyz.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        projects_root.clone(),
        "claude-code".to_string(),
        decode_cc_line,
        custom_label,
        cc_session_ended,
    );
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "ses-xyz",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let mut got_custom_rename = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_, AgentEvent::Rename { label, .. }))) => {
                if label == "custom-label-ok" {
                    got_custom_rename = true;
                    break;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(
        got_custom_rename,
        "expected Rename event with custom label from label deriver fn"
    );
    handle.abort();
}

#[tokio::test]
async fn codex_rollout_yields_uuid_keyed_session_start() {
    use pixtuoid_core::source::codex::{codex_id_from_path, decode_codex_line, derive_codex_label};

    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let transcript = root.join(format!("rollout-2026-05-29T22-36-52-{uuid}.jsonl"));

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(
        root.clone(),
        "codex".to_string(),
        decode_codex_line,
        derive_codex_label,
        |_t| false,
    )
    .with_id_deriver(codex_id_from_path);
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": uuid, "cwd": "/Users/me/dotfiles" }
    });
    f.write_all(format!("{meta}\n").as_bytes()).await.unwrap();
    let task_started = serde_json::json!({
        "type": "event_msg",
        "payload": { "type": "task_started", "turn_id": "t" }
    });
    f.write_all(format!("{task_started}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    let expected = AgentId::from_parts("codex", uuid);
    let mut saw_session_start = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) => {
                assert_eq!(agent_id, expected, "Codex SessionStart must be UUID-keyed");
                saw_session_start = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(saw_session_start, "expected a SessionStart event");
    handle.abort();
}

#[tokio::test]
async fn default_id_deriver_stays_path_keyed() {
    // Pin the IdDeriver default: a non-Codex watcher must key on the file path
    // (so CC/Antigravity hook↔JSONL coalescing is unchanged).
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let project_dir = root.join("proj-y");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = cc_watcher(root.clone());
    let handle = tokio::spawn(async move { watcher.run(tx).await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript)
        .await
        .unwrap();
    let start_line = serde_json::json!({
        "type": "system",
        "subtype": "session_start",
        "sessionId": "abc",
        "cwd": "/repo"
    });
    f.write_all(format!("{start_line}\n").as_bytes())
        .await
        .unwrap();
    f.flush().await.unwrap();
    drop(f);

    // The default deriver keys on the file path. The watcher may report the
    // raw TempDir path (rescan via read_dir) or the symlink-resolved path
    // (macOS FSEvents canonicalizes /var → /private/var), so accept either —
    // both are path-keyed. What must NOT match is a UUID/stem key.
    let raw = AgentId::from_parts("claude-code", &transcript.to_string_lossy());
    let canon = std::fs::canonicalize(&transcript)
        .map(|p| AgentId::from_parts("claude-code", &p.to_string_lossy()))
        .unwrap_or(raw);
    let stem_keyed = AgentId::from_parts("claude-code", "abc");
    let mut ok = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) => {
                assert_ne!(
                    agent_id, stem_keyed,
                    "default deriver must be path-keyed, not stem-keyed"
                );
                assert!(
                    agent_id == raw || agent_id == canon,
                    "default deriver must key on the file path (raw or canonical)"
                );
                ok = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    assert!(ok, "expected a path-keyed SessionStart");
    handle.abort();
}
