use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use ascii_agents_core::source::jsonl::JsonlWatcher;
use ascii_agents_core::source::AgentEvent;
use ascii_agents_core::state::reducer::Transport;

#[tokio::test]
async fn watcher_emits_session_start_then_activity_for_tool_use() {
    let dir = TempDir::new().unwrap();
    let projects_root = dir.path().to_path_buf();
    let project_dir = projects_root.join("proj-x");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let transcript = project_dir.join("ses-abc.jsonl");

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(32);
    let watcher = JsonlWatcher::new(projects_root.clone());
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
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
    let watcher = JsonlWatcher::new(projects_root.clone());
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
    assert!(got_tu_2, "tu_2 should appear after partial line is completed");

    handle.abort();
}
