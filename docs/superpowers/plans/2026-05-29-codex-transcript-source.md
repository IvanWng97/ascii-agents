# Codex Transcript Source — Implementation Plan

> **For agentic workers:** Implement task-by-task. Each task is TDD: write the failing test, run it (see it fail), implement minimally, run it (see it pass), commit. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add a Codex transcript (JSONL) source so `cx·` sprites resume "working" after a permission approval (the transcript's `function_call_output` → `ActivityStart` clears `Waiting`). Hook + JSONL coalesce on the session UUID.

**Architecture:** Keep the existing `Source` / `JsonlWatcher` seam (no per-CLI registry — reviewed). New `CodexSource` (JSONL-only; hooks already arrive via the shared socket). Add an `IdDeriver` fn-pointer to `JsonlWatcher` so the generic `SessionStart` keys on the trailing UUID of the rollout filename (matching the hook's `session_id`); default stays path-string so CC/Antigravity are byte-identical.

**Tech Stack:** Rust (workspace: `pixtuoid-core`, `pixtuoid`), tokio, `notify`, serde_json, `async_trait`, anyhow.

**Spec:** `docs/superpowers/specs/2026-05-29-codex-transcript-source.md`

**All work happens in the worktree `/Users/navepnow/Desktop/pixtuoid-codex-src` (branch `feat/codex-transcript-source`).** Use absolute paths. Run cargo with `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo …`. Do NOT switch git branches; do NOT `git push`. Commit per task with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## File Structure

- **Create** `crates/pixtuoid-core/src/source/codex.rs` — `CodexSource` (Source impl), `SOURCE_NAME`, `codex_id_from_path`, `decode_codex_line`, `function_call_needs_approval`, `codex_tool_start`, `derive_codex_label`, `codex_session_ended`, `#[cfg(test)] mod tests`.
- **Modify** `crates/pixtuoid-core/src/source/jsonl.rs` — add `IdDeriver` fn-pointer (field + `.with_id_deriver()` builder + default `default_id_from_path`), thread it into `walk_jsonl`/seed/scan, use it for the generic `SessionStart` `AgentId`; extend `extract_cwd` to read nested `payload.cwd`.
- **Modify** `crates/pixtuoid-core/src/source/mod.rs` — `pub mod codex;`.
- **Modify** `crates/pixtuoid-core/src/state/reducer.rs` — `source_label_prefix` arm references `crate::source::codex::SOURCE_NAME`.
- **Modify** `crates/pixtuoid/src/runtime.rs` — wire `CodexSource::default_paths()` into `SourceManager`.
- **Test** `crates/pixtuoid-core/tests/reducer.rs` — permission-resume regression.
- **Test** `crates/pixtuoid-core/tests/jsonl_watcher.rs` — Codex UUID-keying + IdDeriver default path-keying pin.

Run all tests with: `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo test --workspace --features pixtuoid-core/test-renderer`.

---

## Task 1: `IdDeriver` fn-pointer + `extract_cwd` payload.cwd (`jsonl.rs`)

**Files:**
- Modify: `crates/pixtuoid-core/src/source/jsonl.rs`

- [ ] **Step 1: Write the failing unit test.** Append to a `#[cfg(test)] mod tests` at the bottom of `jsonl.rs` (create the module if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_id_from_path_returns_full_path_string() {
        let p = Path::new("/Users/me/.claude/projects/x/abc.jsonl");
        assert_eq!(default_id_from_path(p), "/Users/me/.claude/projects/x/abc.jsonl");
    }

    #[test]
    fn extract_cwd_reads_top_level_and_nested_payload() {
        // CC/AG shape: top-level cwd.
        let top = br#"{"cwd":"/repo/a"}"#;
        assert_eq!(extract_cwd(top), Some(PathBuf::from("/repo/a")));
        // Codex shape: cwd nested under payload (session_meta).
        let nested = br#"{"type":"session_meta","payload":{"cwd":"/repo/b","id":"u"}}"#;
        assert_eq!(extract_cwd(nested), Some(PathBuf::from("/repo/b")));
    }
}
```

- [ ] **Step 2: Run, see it fail.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo test -p pixtuoid-core --lib source::jsonl::tests 2>&1 | tail -20`
  Expected: fails to compile (`default_id_from_path` undefined) and/or `extract_cwd` nested assertion fails.

- [ ] **Step 3: Add the `IdDeriver` type + default, near the other fn-pointer type aliases (after `SessionEndChecker` at line ~17):**

```rust
/// Derives the opaque session-id string used to build the generic
/// `SessionStart`'s `AgentId`. Default returns the transcript file path
/// (CC/Antigravity coalesce hook↔JSONL on the path). Codex overrides it to
/// the rollout filename's trailing UUID so it matches the hook `session_id`.
pub type IdDeriver = fn(&Path) -> String;

fn default_id_from_path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
```

- [ ] **Step 4: Add the field + builder + new()-default.** In `struct JsonlWatcher`, add field `id_derive: IdDeriver,`. In `JsonlWatcher::new(...)`, set `id_derive: default_id_from_path,` in the struct literal. Add the builder after `with_initial_window`:

```rust
    pub fn with_id_deriver(mut self, id_derive: IdDeriver) -> Self {
        self.id_derive = id_derive;
        self
    }
```

- [ ] **Step 5: Thread `id_derive` through and use it.** In `run()`, after `let derive_label = self.derive_label;` add `let id_derive = self.id_derive;`. Pass `id_derive` as a new trailing argument to `initial_seed_root`, `scan_root`, and `walk_jsonl` at every call site (in `run`, `initial_seed_root`, `initial_seed_walk`, `scan_root`). Add `id_derive: IdDeriver,` as the last parameter of `initial_seed_root`, `initial_seed_walk`, `scan_root`, and `walk_jsonl` (all already `#[allow(clippy::too_many_arguments)]` or add it). In `walk_jsonl`, change the id construction (currently `let id = AgentId::from_parts(source, &transcript_path_str);`) to:

```rust
            let id = AgentId::from_parts(source, &id_derive(path));
```

Leave `transcript_path_str` as-is for the `decode_line` call (decoders still receive the file path).

- [ ] **Step 6: Extend `extract_cwd`.** Replace its inner check so it reads top-level `cwd` then nested `payload.cwd`:

```rust
        if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
            return Some(PathBuf::from(cwd));
        }
        if let Some(cwd) = v
            .get("payload")
            .and_then(|p| p.get("cwd"))
            .and_then(|c| c.as_str())
        {
            return Some(PathBuf::from(cwd));
        }
```

- [ ] **Step 7: Run, see it pass.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo test -p pixtuoid-core --lib source::jsonl 2>&1 | tail -20` → PASS. Then `cargo build -p pixtuoid-core` to confirm CC/AG call sites still compile.

- [ ] **Step 8: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid-core/src/source/jsonl.rs
git commit -m "feat(source): IdDeriver fn-pointer + nested payload.cwd in JsonlWatcher

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `codex.rs` skeleton — `SOURCE_NAME` + `codex_id_from_path`

**Files:**
- Create: `crates/pixtuoid-core/src/source/codex.rs`
- Modify: `crates/pixtuoid-core/src/source/mod.rs`

- [ ] **Step 1: Register the module.** In `mod.rs`, add `pub mod codex;` (alphabetical, before `pub mod decoder;`).

- [ ] **Step 2: Create `codex.rs` with the header, `SOURCE_NAME`, `codex_id_from_path`, `is_uuid`, and a failing test:**

```rust
//! Codex CLI source. Watches the Codex session transcript
//! (`~/.codex/sessions/**/rollout-<ts>-<UUID>.jsonl`) via `JsonlWatcher`.
//! Codex hooks already arrive through the shared hook socket (the shim stamps
//! `source=codex`); this source adds the JSONL lifecycle signals hooks lack —
//! most importantly the post-approval resume (`function_call_output`).
//!
//! Coalescing: hook events key `AgentId` on the hook `session_id`; this source
//! keys on the trailing UUID of the rollout filename. Verified equal
//! (hook.session_id == session_meta.id == filename UUID), so both transports
//! merge onto one sprite.

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::source::decoder::make_tool_detail;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "codex";

/// Trailing canonical UUID (`8-4-4-4-12`) of a `rollout-<ts>-<UUID>.jsonl`
/// filename. Equals the hook payload's `session_id`, so hook and JSONL events
/// coalesce. Falls back to the full stem if no trailing UUID is present.
pub fn codex_id_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let tail = &stem[stem.len().saturating_sub(36)..];
    if is_uuid(tail) {
        tail.to_string()
    } else {
        stem.to_string()
    }
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_from_rollout_path_is_trailing_uuid() {
        let p = Path::new(
            "/Users/me/.codex/sessions/2026/05/29/rollout-2026-05-29T22-36-52-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl",
        );
        // Must equal the hook session_id for coalescing.
        assert_eq!(codex_id_from_path(p), "019e7762-9ded-7e33-be41-946ecf105bf4");
    }

    #[test]
    fn id_falls_back_to_stem_without_uuid() {
        let p = Path::new("/tmp/notarollout.jsonl");
        assert_eq!(codex_id_from_path(p), "notarollout");
    }
}
```

- [ ] **Step 3: Run, see it fail then pass.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo test -p pixtuoid-core --lib source::codex 2>&1 | tail -20`. The unused imports (`Map`, `Value`, `make_tool_detail`, etc.) will warn but the two id tests should PASS. (Warnings are fine here; they're resolved in Task 3–4. If `-D warnings` blocks the test build, add a temporary `#![allow(unused_imports)]` at the top of the module and remove it in Task 4.)

- [ ] **Step 4: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid-core/src/source/codex.rs crates/pixtuoid-core/src/source/mod.rs
git commit -m "feat(codex): codex source module + codex_id_from_path (UUID coalescing key)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `decode_codex_line` (transcript → AgentEvents)

**Files:**
- Modify: `crates/pixtuoid-core/src/source/codex.rs`

- [ ] **Step 1: Add failing tests** to the `tests` module:

```rust
    use serde_json::json;

    fn ev(line: Value) -> Vec<AgentEvent> {
        decode_codex_line("/x/rollout-1-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl", SOURCE_NAME, line).unwrap()
    }

    #[test]
    fn task_started_is_activity_start() {
        let out = ev(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}));
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn function_call_output_resumes_work() {
        // THE fix: resume signal must be an ActivityStart (clears Waiting in the reducer).
        let out = ev(json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"ok"}}));
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn escalated_function_call_is_waiting() {
        let args = r#"{"cmd":"date","sandbox_permissions":"require_escalated","justification":"allow?"}"#;
        let out = ev(json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}));
        assert!(matches!(out.as_slice(), [AgentEvent::Waiting { .. }]));
    }

    #[test]
    fn plain_function_call_is_activity_start() {
        let args = r#"{"cmd":"ls"}"#;
        let out = ev(json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":args}}));
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn malformed_arguments_does_not_panic_and_starts_work() {
        let out = ev(json!({"type":"response_item","payload":{"type":"function_call","name":"x","arguments":"{not json"}}));
        assert!(matches!(out.as_slice(), [AgentEvent::ActivityStart { .. }]));
    }

    #[test]
    fn task_complete_and_abort_end_activity() {
        for t in ["task_complete", "turn_aborted"] {
            let out = ev(json!({"type":"event_msg","payload":{"type":t,"turn_id":"t"}}));
            assert!(matches!(out.as_slice(), [AgentEvent::ActivityEnd { .. }]), "{t}");
        }
    }

    #[test]
    fn session_meta_and_unknown_emit_nothing() {
        assert!(ev(json!({"type":"session_meta","payload":{"id":"u","cwd":"/r"}})).is_empty());
        assert!(ev(json!({"type":"event_msg","payload":{"type":"token_count"}})).is_empty());
    }
```

- [ ] **Step 2: Run, see it fail.** `cargo test -p pixtuoid-core --lib source::codex` → `decode_codex_line` undefined.

- [ ] **Step 3: Implement `decode_codex_line` + helpers** (after `is_uuid`):

```rust
/// Decode one transcript line. `tool_use_id` is always `None` so these events
/// are never suppressed by the hook-wins dedup (which keys on `tool_use_id`).
pub fn decode_codex_line(transcript_path: &str, source: &str, v: Value) -> Result<Vec<AgentEvent>> {
    let agent_id = AgentId::from_parts(source, &codex_id_from_path(Path::new(transcript_path)));
    let Some(obj) = v.as_object() else {
        return Ok(vec![]);
    };
    let outer = obj.get("type").and_then(|s| s.as_str()).unwrap_or("");
    let payload = obj.get("payload").and_then(|p| p.as_object());
    let inner = payload
        .and_then(|p| p.get("type"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    let start = |activity| AgentEvent::ActivityStart {
        agent_id,
        activity,
        tool_use_id: None,
        detail: None,
    };
    let end = || AgentEvent::ActivityEnd {
        agent_id,
        tool_use_id: None,
    };

    let out = match (outer, inner) {
        ("event_msg", "task_started") => vec![start(Activity::Thinking)],
        ("response_item", "function_call") => {
            if function_call_needs_approval(payload) {
                vec![AgentEvent::Waiting {
                    agent_id,
                    reason: "permission".to_string(),
                }]
            } else {
                vec![codex_tool_start(agent_id, payload)]
            }
        }
        ("response_item", "function_call_output") | ("event_msg", "exec_command_end") => {
            vec![start(Activity::Typing)]
        }
        ("event_msg", "task_complete") | ("event_msg", "turn_aborted") => vec![end()],
        _ => vec![],
    };
    Ok(out)
}

/// A Codex `function_call` whose `arguments` (a JSON string) requests escalated
/// sandbox permissions or carries a justification is an approval gate → Waiting.
fn function_call_needs_approval(payload: Option<&Map<String, Value>>) -> bool {
    let Some(args_str) = payload
        .and_then(|p| p.get("arguments"))
        .and_then(|a| a.as_str())
    else {
        return false;
    };
    let Ok(args) = serde_json::from_str::<Value>(args_str) else {
        return false;
    };
    args.get("sandbox_permissions").and_then(|s| s.as_str()) == Some("require_escalated")
        || args.get("justification").is_some()
}

fn codex_tool_start(agent_id: AgentId, payload: Option<&Map<String, Value>>) -> AgentEvent {
    let name = payload
        .and_then(|p| p.get("name"))
        .and_then(|s| s.as_str())
        .unwrap_or("tool");
    AgentEvent::ActivityStart {
        agent_id,
        activity: Activity::Typing,
        tool_use_id: None,
        detail: Some(make_tool_detail(name, String::new())),
    }
}
```

- [ ] **Step 4: Run, see it pass.** `cargo test -p pixtuoid-core --lib source::codex` → all PASS.

- [ ] **Step 5: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid-core/src/source/codex.rs
git commit -m "feat(codex): decode_codex_line — transcript records to AgentEvents

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `CodexSource` + label + session-end

**Files:**
- Modify: `crates/pixtuoid-core/src/source/codex.rs`

- [ ] **Step 1: Add failing tests** to the `tests` module:

```rust
    #[test]
    fn label_is_cx_basename() {
        assert_eq!(derive_codex_label(Path::new("/x.jsonl"), SOURCE_NAME, Path::new("/Users/me/dotfiles")), "cx·dotfiles");
        assert_eq!(derive_codex_label(Path::new("/x.jsonl"), SOURCE_NAME, Path::new("")), "cx");
    }
```

- [ ] **Step 2: Run, see it fail.** `cargo test -p pixtuoid-core --lib source::codex` → `derive_codex_label` undefined.

- [ ] **Step 3: Implement label, session-end, and the `Source` impl** (and remove any temporary `#![allow(unused_imports)]` from Task 2):

```rust
fn derive_codex_label(_path: &Path, _source: &str, cwd: &Path) -> String {
    if cwd != Path::new("") && cwd != Path::new("/") {
        if let Some(name) = cwd.file_name().and_then(|n| n.to_str()) {
            return format!("cx·{name}");
        }
    }
    "cx".to_string()
}

/// Codex writes no session-end marker; the reducer's stale-sweep reaps dead
/// sessions. Always false (defer to mtime window + stale-sweep).
fn codex_session_ended(_tail: &[u8]) -> bool {
    false
}

/// Source that watches the Codex session transcript directory.
pub struct CodexSource {
    pub sessions_root: PathBuf,
}

impl CodexSource {
    pub fn default_paths() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Self {
            sessions_root: PathBuf::from(format!("{home}/.codex/sessions")),
        }
    }
}

#[async_trait]
impl Source for CodexSource {
    fn name(&self) -> &str {
        SOURCE_NAME
    }

    async fn run(self: Box<Self>, tx: TaggedSender) -> Result<()> {
        let watcher = JsonlWatcher::new(
            self.sessions_root.clone(),
            SOURCE_NAME.to_string(),
            decode_codex_line,
            derive_codex_label,
            codex_session_ended,
        )
        .with_id_deriver(codex_id_from_path);
        watcher.run(tx).await
    }
}
```

- [ ] **Step 4: Run, see it pass + no warnings.** `cargo test -p pixtuoid-core --lib source::codex` → PASS. `cargo build -p pixtuoid-core 2>&1 | grep -i warning` → none.

- [ ] **Step 5: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid-core/src/source/codex.rs
git commit -m "feat(codex): CodexSource (JSONL watcher) + cx label + session-end

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: reducer `source_label_prefix` references `codex::SOURCE_NAME`

**Files:**
- Modify: `crates/pixtuoid-core/src/state/reducer.rs`

- [ ] **Step 1: Make the edit** (consistency tidy; behavior identical). Change the `"codex" => "cx",` arm to:

```rust
        crate::source::codex::SOURCE_NAME => "cx",
```

- [ ] **Step 2: Run the existing reducer tests, confirm no regression.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo test -p pixtuoid-core --test reducer 2>&1 | tail -15` → PASS (including the existing `session_start_codex_source_gets_cx_label`).

- [ ] **Step 3: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid-core/src/state/reducer.rs
git commit -m "refactor(reducer): reference codex::SOURCE_NAME in label prefix

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Wire `CodexSource` into the runtime

**Files:**
- Modify: `crates/pixtuoid/src/runtime.rs`

- [ ] **Step 1: Add the import** near the other source imports (lines 9-11):

```rust
use pixtuoid_core::source::codex::CodexSource;
```

- [ ] **Step 2: Wire it into `SourceManager`** (the `.with_source` chain at ~line 101-104):

```rust
    let _source_handles = SourceManager::new()
        .with_source(Box::new(cc_src))
        .with_source(Box::new(ag_src))
        .with_source(Box::new(CodexSource::default_paths()))
        .spawn(tx);
```

- [ ] **Step 3: Build.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo build -p pixtuoid 2>&1 | tail -15` → compiles.

- [ ] **Step 4: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid/src/runtime.rs
git commit -m "feat(codex): wire CodexSource into the runtime SourceManager

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Permission-resume regression test (`tests/reducer.rs`)

**Files:**
- Modify: `crates/pixtuoid-core/tests/reducer.rs`

- [ ] **Step 1: Write the failing test.** Append (match the file's existing imports/helpers — it already uses `Reducer`, `SceneState`, `AgentEvent`, `Transport`, `AgentId`; reuse its patterns for building a scene and a `now`):

```rust
#[test]
fn codex_permission_then_jsonl_output_resumes_to_active() {
    // Regression: a cx· agent stuck Waiting on a permission prompt must return
    // to Active once the transcript's function_call_output (an ActivityStart)
    // arrives. Hook and JSONL coalesce on the session UUID.
    use pixtuoid_core::source::{Activity, ToolDetail};
    let mut reducer = Reducer::new();
    let mut scene = SceneState::new([16; pixtuoid_core::state::MAX_FLOORS]);
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let id = AgentId::from_parts("codex", uuid);

    // Hook: UserPromptSubmit → SessionStart (agent creation).
    reducer.apply(
        &mut scene,
        AgentEvent::SessionStart {
            agent_id: id,
            source: "codex".to_string(),
            session_id: uuid.to_string(),
            cwd: std::path::PathBuf::from("/Users/me/dotfiles"),
            parent_id: None,
        },
        now,
        Transport::Hook,
    );

    // Hook: PermissionRequest → Waiting.
    reducer.apply(
        &mut scene,
        AgentEvent::Waiting { agent_id: id, reason: "permission".to_string() },
        now,
        Transport::Hook,
    );
    assert!(matches!(scene.agents[&id].state, ActivityState::Waiting { .. }), "should be Waiting on permission");

    // JSONL: function_call_output → ActivityStart → must clear Waiting → Active.
    reducer.apply(
        &mut scene,
        AgentEvent::ActivityStart {
            agent_id: id,
            activity: Activity::Typing,
            tool_use_id: None,
            detail: Some(ToolDetail::from("exec_command")),
        },
        now,
        Transport::Jsonl,
    );
    assert!(matches!(scene.agents[&id].state, ActivityState::Active { .. }), "resume must return to Active");
}
```

> If `ActivityState` / `MAX_FLOORS` aren't already imported in the test file, add `use pixtuoid_core::state::{ActivityState, MAX_FLOORS};` (verify exact public paths — `MAX_FLOORS` may live at `pixtuoid_core::state::MAX_FLOORS` or `pixtuoid_core::MAX_FLOORS`; grep before writing). Match how the other tests in this file construct `SceneState` and `now`.

- [ ] **Step 2: Run, see it pass.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo test -p pixtuoid-core --test reducer codex_permission_then 2>&1 | tail -15`. It should PASS immediately (the reducer already clears Waiting on ActivityStart) — this test **locks in** the bugfix behavior end-to-end. If it does not pass, STOP: the coalescing/clear assumption is wrong and the design needs revisiting.

- [ ] **Step 3: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid-core/tests/reducer.rs
git commit -m "test(reducer): codex permission→resume regression (Waiting cleared by JSONL ActivityStart)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Codex UUID-keying + IdDeriver-default integration tests (`tests/jsonl_watcher.rs`)

**Files:**
- Modify: `crates/pixtuoid-core/tests/jsonl_watcher.rs`

- [ ] **Step 1: Inspect the existing file's harness.** `grep -nE "JsonlWatcher|tempfile|TempDir|tokio::|mpsc|recv|spawn" crates/pixtuoid-core/tests/jsonl_watcher.rs | head -30`. Reuse its exact pattern for: creating a temp dir, constructing the channel, spawning the watcher, writing a `.jsonl` file, and draining events with a timeout. Match its style (don't invent a new harness).

- [ ] **Step 2: Write the failing test** — a Codex rollout file yields a UUID-keyed `SessionStart`, and a non-Codex (default-deriver) watcher stays path-keyed:

```rust
#[tokio::test]
async fn codex_rollout_yields_uuid_keyed_session_start() {
    use pixtuoid_core::source::codex::{codex_id_from_path, decode_codex_line, derive_codex_label};
    use pixtuoid_core::source::jsonl::JsonlWatcher;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use pixtuoid_core::AgentId;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let uuid = "019e7762-9ded-7e33-be41-946ecf105bf4";
    let file = root.join(format!("rollout-2026-05-29T22-36-52-{uuid}.jsonl"));
    std::fs::write(
        &file,
        format!(
            "{}\n{}\n",
            r#"{"type":"session_meta","payload":{"id":"019e7762-9ded-7e33-be41-946ecf105bf4","cwd":"/Users/me/dotfiles"}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t"}}"#,
        ),
    )
    .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let watcher = JsonlWatcher::new(
        root.clone(),
        "codex".to_string(),
        decode_codex_line,
        derive_codex_label,
        |_t| false,
    )
    .with_id_deriver(codex_id_from_path);
    tokio::spawn(async move { let _ = watcher.run(tx).await; });

    let expected = AgentId::from_parts("codex", uuid);
    let mut saw_session_start = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await {
            Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) => {
                assert_eq!(agent_id, expected, "Codex SessionStart must be UUID-keyed");
                saw_session_start = true;
                break;
            }
            Ok(Some(_)) => continue,
            _ => continue,
        }
    }
    assert!(saw_session_start, "expected a SessionStart event");
}

#[tokio::test]
async fn default_id_deriver_stays_path_keyed() {
    // Pin the IdDeriver default: a non-Codex watcher must key on the file path
    // (so CC/Antigravity hook↔JSONL coalescing is unchanged).
    use pixtuoid_core::source::jsonl::JsonlWatcher;
    use pixtuoid_core::source::{AgentEvent, Transport};
    use pixtuoid_core::AgentId;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let file = root.join("abc.jsonl");
    std::fs::write(&file, "{\"cwd\":\"/repo\"}\n").unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let watcher = JsonlWatcher::new(
        root.clone(),
        "claude-code".to_string(),
        |_p, _s, _v| Ok(vec![]),
        |_p, _s, _c| "x".to_string(),
        |_t| false,
    );
    tokio::spawn(async move { let _ = watcher.run(tx).await; });

    let expected = AgentId::from_parts("claude-code", &file.to_string_lossy());
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut ok = false;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_t, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv()).await
        {
            assert_eq!(agent_id, expected, "default deriver must be path-keyed");
            ok = true;
            break;
        }
    }
    assert!(ok, "expected a path-keyed SessionStart");
}
```

> Verify `tempfile` is a dev-dependency of `pixtuoid-core` (the existing `jsonl_watcher.rs` likely already uses it). If a different temp mechanism is used in that file, follow it. Remove the unused `Transport` import if clippy flags it.

- [ ] **Step 3: Run, see it pass.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo test -p pixtuoid-core --test jsonl_watcher 2>&1 | tail -20` → PASS.

- [ ] **Step 4: Commit.**

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add crates/pixtuoid-core/tests/jsonl_watcher.rs
git commit -m "test(jsonl): codex UUID-keying + IdDeriver default path-keying pins

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Full preflight

- [ ] **Step 1: Run the CI mirror.** `cd /Users/navepnow/Desktop/pixtuoid-codex-src && cargo fmt --all && ./scripts/preflight.sh 2>&1 | tail -30` (fmt + cargo-machete + cargo-deny + clippy `-D warnings` + workspace tests). Fix anything red. If `preflight.sh` is unavailable, run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --features pixtuoid-core/test-renderer`.

- [ ] **Step 2: Commit any fmt/clippy fixes** (if `cargo fmt` changed files):

```bash
cd /Users/navepnow/Desktop/pixtuoid-codex-src
git add -A && git commit -m "style: fmt/clippy for codex transcript source

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review checklist (run after all tasks)

- **Spec coverage:** every spec component has a task (jsonl IdDeriver+cwd → T1; codex.rs → T2-4; mod reg → T2; reducer tidy → T5; runtime wiring → T6; regression → T7; integration → T8). ✓
- **Type consistency:** `codex_id_from_path: fn(&Path)->String` matches `IdDeriver`; `decode_codex_line` matches `LineDecoder = fn(&str,&str,Value)->Result<Vec<AgentEvent>>`; `derive_codex_label` matches `LabelDeriver = fn(&Path,&str,&Path)->String`; `codex_session_ended` matches `SessionEndChecker = fn(&[u8])->bool`. ✓
- **No placeholders:** all code blocks are complete. ✓
- **Coalescing:** hook `session_id` (UUID) == `codex_id_from_path(rollout)` (UUID) — verified live, pinned by T2 + T7 tests. ✓
