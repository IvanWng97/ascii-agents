use serde_json::{json, Map, Value};

pub const SENTINEL_KEY: &str = "_pixtuoid";
pub const LEGACY_SENTINEL_KEY: &str = "_ascii_agents";
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "SessionEnd",
];

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
        arr.retain(|entry| {
            entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) != Some(true)
                && entry.get(LEGACY_SENTINEL_KEY).and_then(|v| v.as_bool()) != Some(true)
        });
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
            arr.retain(|entry| {
                entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) != Some(true)
                    && entry.get(LEGACY_SENTINEL_KEY).and_then(|v| v.as_bool()) != Some(true)
            });
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
}
