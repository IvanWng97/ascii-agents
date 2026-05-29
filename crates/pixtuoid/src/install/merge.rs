use serde_json::{json, Map, Value};
use std::path::Path;
use toml::value::Table;

pub const SENTINEL_KEY: &str = "_pixtuoid";

/// Legacy sentinel keys from previous tool names. Entries tagged with any of
/// these are stripped on install/uninstall so a v0.3.x → v0.4.x upgrade does
/// not leave orphan hooks pointing at missing binaries (or worse, a live
/// legacy hook racing the new shim).
pub const LEGACY_SENTINEL_KEYS: &[&str] = &["_ascii_agents"];

pub const EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "SessionEnd",
];

pub const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];

fn is_managed_entry(entry: &Value) -> bool {
    if entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    LEGACY_SENTINEL_KEYS
        .iter()
        .any(|k| entry.get(*k).and_then(|v| v.as_bool()) == Some(true))
}

/// Merge pixtuoid hook entries into a CC settings.json document.
/// Idempotent: re-running replaces existing pixtuoid entries.
pub fn merge_install(doc: Value, hook_command: &str) -> Value {
    let mut root: Map<String, Value> = doc.as_object().cloned().unwrap_or_default();
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks_obj = match hooks.as_object_mut() {
        Some(o) => o,
        None => {
            *hooks = Value::Object(Map::new());
            hooks.as_object_mut().expect("just stored Value::Object")
        }
    };

    for ev in EVENTS {
        let list = hooks_obj
            .entry((*ev).to_string())
            .or_insert_with(|| Value::Array(vec![]));
        let arr = match list.as_array_mut() {
            Some(a) => a,
            None => {
                *list = Value::Array(vec![]);
                list.as_array_mut().expect("just stored Value::Array")
            }
        };
        arr.retain(|entry| !is_managed_entry(entry));
        arr.push(json!({
            SENTINEL_KEY: true,
            "matcher": ".*",
            "hooks": [
                { "type": "command", "command": hook_command }
            ]
        }));
    }

    Value::Object(root)
}

/// Remove pixtuoid hook entries. Idempotent.
pub fn merge_uninstall(mut doc: Value) -> Value {
    let Some(root) = doc.as_object_mut() else {
        return doc;
    };
    let Some(Value::Object(hooks_obj)) = root.get_mut("hooks") else {
        return doc;
    };
    for (_ev, list) in hooks_obj.iter_mut() {
        if let Some(arr) = list.as_array_mut() {
            arr.retain(|entry| !is_managed_entry(entry));
        }
    }
    let to_remove: Vec<String> = hooks_obj
        .iter()
        .filter_map(|(k, v)| match v.as_array() {
            Some(a) if a.is_empty() => Some(k.clone()),
            _ => None,
        })
        .collect();
    for k in to_remove {
        hooks_obj.remove(&k);
    }
    if hooks_obj.is_empty() {
        root.remove("hooks");
    }
    doc
}

fn codex_managed_command(command: &str) -> bool {
    Path::new(command).file_name().and_then(|s| s.to_str()) == Some("pixtuoid-hook")
}

fn codex_hook_is_managed(hook: &toml::Value) -> bool {
    hook.get("type").and_then(|v| v.as_str()) == Some("command")
        && hook
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(codex_managed_command)
}

fn prune_codex_managed_hooks(group: &mut toml::Value) {
    let Some(hooks) = group
        .get_mut("hooks")
        .and_then(|hooks| hooks.as_array_mut())
    else {
        return;
    };
    hooks.retain(|hook| !codex_hook_is_managed(hook));
}

fn codex_group_has_no_hooks(group: &toml::Value) -> bool {
    group
        .get("hooks")
        .and_then(|hooks| hooks.as_array())
        .is_some_and(|hooks| hooks.is_empty())
}

fn codex_group(event: &str, hook_command: &str) -> toml::Value {
    let mut hook = Table::new();
    hook.insert(
        "type".to_string(),
        toml::Value::String("command".to_string()),
    );
    hook.insert(
        "command".to_string(),
        toml::Value::String(hook_command.to_string()),
    );
    hook.insert("timeout".to_string(), toml::Value::Integer(1));
    hook.insert(
        "statusMessage".to_string(),
        toml::Value::String("Updating pixtuoid".to_string()),
    );

    let mut group = Table::new();
    if matches!(
        event,
        "PreToolUse" | "PostToolUse" | "SubagentStart" | "SubagentStop"
    ) {
        group.insert("matcher".to_string(), toml::Value::String("*".to_string()));
    } else if event == "SessionStart" {
        group.insert(
            "matcher".to_string(),
            toml::Value::String("startup|resume|clear|compact".to_string()),
        );
    }
    group.insert(
        "hooks".to_string(),
        toml::Value::Array(vec![toml::Value::Table(hook)]),
    );
    toml::Value::Table(group)
}

/// Merge pixtuoid hook entries into a Codex config.toml document.
/// Idempotent: re-running replaces existing pixtuoid entries.
pub fn merge_codex_install(doc: toml::Value, hook_command: &str) -> toml::Value {
    let mut root = doc.as_table().cloned().unwrap_or_default();
    let features = root
        .entry("features".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    if !features.is_table() {
        *features = toml::Value::Table(Table::new());
    }
    if let Some(table) = features.as_table_mut() {
        table.insert("hooks".to_string(), toml::Value::Boolean(true));
    }

    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    if !hooks.is_table() {
        *hooks = toml::Value::Table(Table::new());
    }
    if let Some(hooks) = hooks.as_table_mut() {
        for ev in CODEX_EVENTS {
            let entry = hooks
                .entry((*ev).to_string())
                .or_insert_with(|| toml::Value::Array(vec![]));
            if !entry.is_array() {
                *entry = toml::Value::Array(vec![]);
            }
            if let Some(arr) = entry.as_array_mut() {
                for group in arr.iter_mut() {
                    prune_codex_managed_hooks(group);
                }
                arr.retain(|group| !codex_group_has_no_hooks(group));
                arr.push(codex_group(ev, hook_command));
            }
        }
    }

    toml::Value::Table(root)
}

/// Remove pixtuoid hook entries from a Codex config.toml document.
pub fn merge_codex_uninstall(mut doc: toml::Value) -> toml::Value {
    let Some(root) = doc.as_table_mut() else {
        return doc;
    };
    let Some(toml::Value::Table(hooks)) = root.get_mut("hooks") else {
        return doc;
    };

    for (_ev, list) in hooks.iter_mut() {
        if let Some(arr) = list.as_array_mut() {
            for group in arr.iter_mut() {
                prune_codex_managed_hooks(group);
            }
            arr.retain(|group| !codex_group_has_no_hooks(group));
        }
    }
    let empty_events: Vec<String> = hooks
        .iter()
        .filter_map(|(k, v)| match v.as_array() {
            Some(a) if a.is_empty() => Some(k.clone()),
            _ => None,
        })
        .collect();
    for key in empty_events {
        hooks.remove(&key);
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_creates_entries_for_all_events() {
        let doc = merge_install(json!({}), "/usr/local/bin/pixtuoid-hook");
        let hooks = doc.get("hooks").and_then(|v| v.as_object()).unwrap();
        for ev in EVENTS {
            let arr = hooks.get(*ev).and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            assert_eq!(arr[0][SENTINEL_KEY], json!(true));
            assert_eq!(
                arr[0]["hooks"][0]["command"],
                json!("/usr/local/bin/pixtuoid-hook")
            );
        }
    }

    #[test]
    fn install_is_idempotent() {
        let d1 = merge_install(json!({}), "/x");
        let d2 = merge_install(d1.clone(), "/x");
        assert_eq!(d1, d2);
    }

    #[test]
    fn install_preserves_unrelated_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/other"}] }
                ]
            },
            "theme": "dark"
        });
        let merged = merge_install(initial, "/x");
        let arr = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(merged["theme"], json!("dark"));
    }

    #[test]
    fn uninstall_removes_sentinel_entries_only() {
        let installed = merge_install(
            json!({
                "hooks": { "PreToolUse": [
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/other"}] }
                ]}
            }),
            "/x",
        );
        let cleaned = merge_uninstall(installed);
        let arr = cleaned["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0][SENTINEL_KEY], json!(null));
    }

    #[test]
    fn uninstall_drops_empty_hooks_map() {
        let installed = merge_install(json!({}), "/x");
        let cleaned = merge_uninstall(installed);
        assert!(cleaned.get("hooks").is_none(), "got {cleaned}");
    }

    // Regression for the v0.3.x → v0.4.x upgrade path: legacy entries tagged
    // `_ascii_agents` must be stripped on install and uninstall. The previous
    // PR #40 dropped the dual-sentinel cleanup, leaving stale hooks that
    // point at a missing `ascii-agents-hook` binary.
    #[test]
    fn install_strips_legacy_ascii_agents_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "_ascii_agents": true, "matcher": ".*", "hooks": [{"type":"command","command":"/old"}] },
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/keep"}] }
                ]
            }
        });
        let merged = merge_install(initial, "/new");
        let arr = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(
            arr.len(),
            2,
            "legacy stripped, user entry kept, pixtuoid added"
        );
        let commands: Vec<&str> = arr
            .iter()
            .map(|e| e["hooks"][0]["command"].as_str().unwrap())
            .collect();
        assert!(commands.contains(&"/keep"));
        assert!(commands.contains(&"/new"));
        assert!(!commands.contains(&"/old"));
    }

    #[test]
    fn uninstall_strips_legacy_ascii_agents_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "_ascii_agents": true, "matcher": ".*", "hooks": [{"type":"command","command":"/old"}] }
                ]
            }
        });
        let cleaned = merge_uninstall(initial);
        assert!(
            cleaned.get("hooks").is_none(),
            "legacy entry should be removed and empty hooks map dropped: {cleaned}"
        );
    }

    #[test]
    fn uninstall_strips_legacy_keeps_user_entries() {
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    { "_ascii_agents": true, "matcher": ".*", "hooks": [{"type":"command","command":"/old"}] },
                    { "matcher": "Write", "hooks": [{"type":"command","command":"/keep"}] }
                ]
            }
        });
        let cleaned = merge_uninstall(initial);
        let arr = cleaned["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], json!("/keep"));
    }

    #[test]
    fn uninstall_non_array_hook_value_does_not_panic() {
        let doc = json!({
            "hooks": {
                "PreToolUse": "not-an-array",
                "PostToolUse": 42
            }
        });
        let cleaned = merge_uninstall(doc);
        let hooks = cleaned["hooks"].as_object().unwrap();
        assert_eq!(
            hooks["PreToolUse"],
            json!("not-an-array"),
            "non-array values should pass through unchanged"
        );
        assert_eq!(hooks["PostToolUse"], json!(42));
    }

    #[test]
    fn codex_install_creates_entries_for_all_events() {
        let doc = merge_codex_install(toml::Value::Table(Table::new()), "/opt/bin/pixtuoid-hook");
        let hooks = doc.get("hooks").and_then(|v| v.as_table()).unwrap();
        for ev in CODEX_EVENTS {
            let arr = hooks.get(*ev).and_then(|v| v.as_array()).unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            assert_eq!(
                arr[0]["hooks"][0]["command"].as_str().unwrap(),
                "/opt/bin/pixtuoid-hook"
            );
        }
        assert_eq!(
            doc["features"]["hooks"].as_bool(),
            Some(true),
            "Codex hook feature must be enabled"
        );
    }

    #[test]
    fn codex_install_registers_user_prompt_submit_without_matcher() {
        let doc = merge_codex_install(toml::Value::Table(Table::new()), "/opt/bin/pixtuoid-hook");
        let group = &doc["hooks"]["UserPromptSubmit"][0];

        assert_eq!(
            group["hooks"][0]["command"].as_str().unwrap(),
            "/opt/bin/pixtuoid-hook"
        );
        assert!(
            group.get("matcher").is_none(),
            "UserPromptSubmit should not have a matcher"
        );
    }

    #[test]
    fn codex_install_is_idempotent_and_replaces_old_pixtuoid_command() {
        let initial = r#"
[features]
hooks = true

[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "pixtuoid-hook"
"#;
        let parsed = toml::from_str::<toml::Value>(initial).unwrap();
        let merged = merge_codex_install(parsed, "/new/pixtuoid-hook");
        let merged_again = merge_codex_install(merged.clone(), "/new/pixtuoid-hook");
        assert_eq!(merged, merged_again);

        let pre = merged["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(
            pre[0]["hooks"][0]["command"].as_str().unwrap(),
            "/new/pixtuoid-hook"
        );
    }

    #[test]
    fn codex_install_preserves_sibling_hooks_in_mixed_group() {
        let initial = r#"
[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/keep"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/old/pixtuoid-hook"
"#;
        let merged = merge_codex_install(
            toml::from_str::<toml::Value>(initial).unwrap(),
            "/new/pixtuoid-hook",
        );
        let pre = merged["hooks"]["PreToolUse"].as_array().unwrap();

        assert_eq!(
            pre.len(),
            2,
            "mixed group should be kept and new managed group added"
        );
        assert_eq!(pre[0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(pre[0]["hooks"][0]["command"].as_str().unwrap(), "/keep");
        assert_eq!(
            pre[1]["hooks"][0]["command"].as_str().unwrap(),
            "/new/pixtuoid-hook"
        );
    }

    #[test]
    fn codex_uninstall_removes_only_pixtuoid_entries() {
        let initial = r#"
[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/keep"

[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/opt/bin/pixtuoid-hook"
"#;
        let cleaned = merge_codex_uninstall(toml::from_str::<toml::Value>(initial).unwrap());
        let pre = cleaned["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["hooks"][0]["command"].as_str().unwrap(), "/keep");
    }

    #[test]
    fn codex_uninstall_preserves_sibling_hooks_in_mixed_group() {
        let initial = r#"
[[hooks.PreToolUse]]
matcher = "*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/keep"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "/opt/bin/pixtuoid-hook"
"#;
        let cleaned = merge_codex_uninstall(toml::from_str::<toml::Value>(initial).unwrap());
        let pre = cleaned["hooks"]["PreToolUse"].as_array().unwrap();

        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(pre[0]["hooks"][0]["command"].as_str().unwrap(), "/keep");
    }
}
