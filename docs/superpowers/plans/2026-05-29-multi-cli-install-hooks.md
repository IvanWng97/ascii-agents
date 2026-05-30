# Multi-CLI Hook Install Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the Claude-only `install-hooks`/`uninstall-hooks` into an extensible multi-CLI install layer (`Target` data struct + shared orchestrator + format-neutral `io.rs`), then add Codex as the second target.

**Architecture:** A `const`-data `Target` struct holds per-CLI fn-pointers (config path, hook-command builder, format-specific merge/unmerge). A `TARGETS` registry + one shared `run_install`/`run_uninstall` orchestrator owns the atomic-write + backup machinery so no target can skip it. `io.rs` becomes format-neutral (`String` in/out; JSON-vs-TOML lives in each target module). A pure `plan_targets` function decides which targets to act on (auto-detect + confirm + non-TTY policy).

**Tech Stack:** Rust (workspace, MSRV 1.78), `serde_json` (Claude JSON), `toml` (Codex TOML — already a dep), `clap` (`ValueEnum`), `fs2` (advisory lock), `std::io::IsTerminal` (TTY detection). Spec: [`docs/superpowers/specs/2026-05-29-multi-cli-install-hooks-design.md`](../specs/2026-05-29-multi-cli-install-hooks-design.md).

**Scope:** Phases 1–2 only (buildable + testable here). Phase 3 (decoder/Source wiring, Codex sprites) is a **separate plan** written after the manual Codex-session checkpoint at the end of Phase 2.

---

## File Structure

```
crates/pixtuoid/src/install/
├── mod.rs      [REWRITE]  run_install/run_uninstall orchestrator + plan_targets (pure) + confirm + dispatch
├── io.rs       [REWRITE]  format-neutral fs: read_config/write_config_atomic (String), backup/remove (suffix-param)
├── target.rs   [CREATE]   Target struct + CLAUDE (P1) + CODEX (P2) + TARGETS + by_name + is_present/config_present
├── claude.rs   [CREATE]   JSON merge (moved from merge.rs) + parse_or_empty + hook_command + default_config_path + LEGACY keys
├── codex.rs    [CREATE]   (P2) TOML merge + sentinel + hook_command (PIXTUOID_SOURCE) + default_config_path
└── merge.rs    [DELETE]   (after claude.rs takes its logic + tests)
crates/pixtuoid/src/cli.rs    [MODIFY]  TargetName enum; InstallHooks/UninstallHooks gain target/config(alias settings)/yes
crates/pixtuoid/src/main.rs   [MODIFY]  dispatch to install::install(InstallArgs)/uninstall(UninstallArgs)
crates/pixtuoid/tests/install.rs [MODIFY]  keep --settings oracle UNCHANGED; add --config + --target tests
CLAUDE.md                     [MODIFY]  rename write_settings_atomic→write_config_atomic; note multi-target install
```

**Why this shape:** `io.rs` = bytes/paths/atomicity (format-agnostic); each target module = one CLI's format knowledge; `mod.rs` = policy + orchestration. Files that change together (a CLI's path + format + command) live together.

**Refactor note:** Phase 1 is a behavior-preserving refactor. The existing `merge.rs` and `io.rs` tests are characterization tests — they must stay green (adapted to new signatures). Each task below ends with a **green `cargo build` + `cargo test`**; do not leave the workspace uncompilable between commits.

**Build/test commands (run from repo root):**
- Build: `cargo build -p pixtuoid`
- Unit tests for a module: `cargo test -p pixtuoid install::`
- Crate tests incl. integration: `cargo test -p pixtuoid`
- Full preflight (CI mirror): `./scripts/preflight.sh`

---

# PHASE 1 — HookTarget refactor (Claude behavior preserved)

## Task 1: Format-neutral `io.rs`

**Files:**
- Modify: `crates/pixtuoid/src/install/io.rs`

Current `io.rs` exposes `read_settings(&Path) -> Result<Value>`, `write_settings_atomic(&Path, &Value)`, `backup_once(&Path)`, `remove_backup(&Path)`, plus `resolve_symlink`, `default_hook_binary`, `hook_on_path`, `default_settings_path`. We replace the JSON-typed and hardcoded-suffix pieces with `String`/suffix-parameterized ones, and switch sibling-path construction to string-append (fixes the TOML backup/lock corruption).

- [ ] **Step 1: Write the failing tests** (append to the existing `mod tests` in `io.rs`, and delete the old `write_settings_atomic_through_symlink_preserves_link` / `backup_once_*` / `remove_backup_*` tests that use the old signatures — they are rewritten here)

```rust
    #[test]
    fn read_config_missing_returns_empty_string() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read_config(&dir.path().join("nope.json")).unwrap(), "");
    }

    #[test]
    fn read_config_empty_file_returns_empty_string() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("empty.json");
        std::fs::write(&p, "").unwrap();
        assert_eq!(read_config(&p).unwrap(), "");
    }

    #[test]
    fn read_config_returns_raw_content() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("c.toml");
        std::fs::write(&p, "a = 1\n").unwrap();
        assert_eq!(read_config(&p).unwrap(), "a = 1\n");
    }

    #[test]
    fn write_config_atomic_through_symlink_preserves_link() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_config_atomic(&link, "{\"a\":1}").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn backup_and_lock_and_tmp_names_use_string_append() {
        // multi-dot filename must keep its full name + suffix (not with_extension truncation)
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("config.local.toml");
        std::fs::write(&p, "x = 1\n").unwrap();
        let bak = backup_once(&p, "pixtuoid.bak").unwrap().unwrap();
        assert_eq!(bak.file_name().unwrap(), "config.local.toml.pixtuoid.bak");
    }

    #[test]
    fn backup_once_idempotent_and_remove() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, "{}").unwrap();
        let b1 = backup_once(&p, "pixtuoid.bak").unwrap().unwrap();
        assert_eq!(b1.file_name().unwrap(), "settings.json.pixtuoid.bak");
        let b2 = backup_once(&p, "pixtuoid.bak").unwrap().unwrap();
        assert_eq!(b1, b2);
        assert_eq!(remove_backup(&p, "pixtuoid.bak").unwrap(), Some(b1.clone()));
        assert!(!b1.exists());
        assert_eq!(remove_backup(&p, "pixtuoid.bak").unwrap(), None);
    }
```

- [ ] **Step 2: Run tests, verify they fail to compile**

Run: `cargo test -p pixtuoid install::io 2>&1 | head -20`
Expected: compile errors — `read_config`/`write_config_atomic` not found, `backup_once`/`remove_backup` arity mismatch.

- [ ] **Step 3: Rewrite the `io.rs` functions**

Replace `read_settings`, `write_settings_atomic`, `backup_once`, `remove_backup` with these. Keep `resolve_symlink`, `default_hook_binary`, `hook_on_path`, `default_settings_path` exactly as they are (note: `default_settings_path` will be re-homed in Task 3; leave it here for now). Remove `use serde_json::Value;` (no longer needed) — keep `anyhow`, `fs2`, `std::fs`, `std::io`, `std::path` imports.

```rust
/// Build a sibling path by APPENDING `.suffix` to the full filename — never
/// `with_extension`, which truncates at the last dot (corrupting `config.toml`
/// into `config.json.pixtuoid.bak` / `config.lock`).
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}", target.display(), suffix))
}

/// Read raw config content, following symlinks. Returns "" for a missing or
/// empty file — the target's parser supplies the empty-document default.
pub fn read_config(path: &Path) -> Result<String> {
    let target = resolve_symlink(path);
    if !target.exists() {
        return Ok(String::new());
    }
    let mut s = String::new();
    File::open(&target)?.read_to_string(&mut s)?;
    Ok(s)
}

/// Atomic write that follows symlinks: write a temp file beside the resolved
/// target, fsync, then rename onto it. Advisory-locked. Format-agnostic (&str).
pub fn write_config_atomic(path: &Path, contents: &str) -> Result<()> {
    let target = resolve_symlink(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = sibling(&target, "lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|e| anyhow!("could not lock {}: {e}", lock_path.display()))?;

    let tmp = sibling(&target, "tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &target)?;
    fs2::FileExt::unlock(&lock).ok();
    Ok(())
}

pub fn backup_once(path: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    let target = resolve_symlink(path);
    if !target.exists() {
        return Ok(None);
    }
    let bak = sibling(&target, suffix);
    if bak.exists() {
        return Ok(Some(bak));
    }
    std::fs::copy(&target, &bak)?;
    Ok(Some(bak))
}

pub fn remove_backup(path: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    let target = resolve_symlink(path);
    let bak = sibling(&target, suffix);
    if !bak.exists() {
        return Ok(None);
    }
    std::fs::remove_file(&bak)?;
    Ok(Some(bak))
}
```

- [ ] **Step 4: Run the io tests**

Run: `cargo test -p pixtuoid install::io`
Expected: PASS (build will still fail at the crate level because `mod.rs`/`merge.rs` reference the removed `read_settings`/`write_settings_atomic` — that's fixed in Task 4. To verify *this module* compiles in isolation, the crate must build; so temporarily this task's verification is the unit-test compile of io.rs. If the crate won't build yet, proceed to Task 4 and run io tests after.)

> **Sequencing reality:** Tasks 1→4 form one coherent refactor; the crate may not build between them. Implement Tasks 1–5 as a unit, then run the full suite at Task 5. Commit at the end of Task 5 (the first green point). Tasks 1–4 are logical checkpoints, not commit points.

---

## Task 2: `target.rs` — the `Target` data struct + Claude registry

**Files:**
- Create: `crates/pixtuoid/src/install/target.rs`

- [ ] **Step 1: Create `target.rs` with the struct, the Claude const, the registry, and detection**

```rust
use std::path::{Path, PathBuf};

use anyhow::Result;

/// A single install destination (one CLI's config file). Fixed set, resolved
/// at compile time as `const` data — no dyn dispatch (install runs once,
/// synchronously). `&CONST` in `const TARGETS` is legal via rvalue static
/// promotion (Rust 1.21+, MSRV 1.78), so `const` is correct here.
pub struct Target {
    /// Stable lowercase id: "claude" | "codex".
    pub name: &'static str,
    /// Human-readable name for CLI output.
    pub display_name: &'static str,
    /// Restart noun for the "→ start a new <noun> session" hint.
    pub restart_noun: &'static str,
    /// Default config path (reads $HOME, hence a fn not a const).
    pub default_config_path: fn() -> PathBuf,
    /// Build the command string written into config from the resolved binary.
    /// Claude returns bare "pixtuoid-hook"; Codex returns the full path (Err on
    /// non-UTF-8). Takes the resolved binary so each target decides how to use it.
    pub hook_command: fn(resolved: &Path) -> Result<String>,
    /// Parse `content`, inject managed hook entries, reserialize. MUST treat
    /// empty/whitespace-only content as the empty document — never error on empty.
    pub merge_install: fn(content: &str, hook_cmd: &str) -> Result<String>,
    /// Parse `content`, remove only managed entries, reserialize. Same empty rule.
    pub merge_uninstall: fn(content: &str) -> Result<String>,
    /// True if the bare hook name must resolve on PATH (Claude writes the bare name).
    pub needs_path_warning: bool,
}

/// Backup suffix — the same constant for every target (not a per-target field).
pub const BACKUP_SUFFIX: &str = "pixtuoid.bak";

pub const CLAUDE: Target = Target {
    name: "claude",
    display_name: "Claude Code",
    restart_noun: "Claude Code",
    default_config_path: crate::install::claude::default_config_path,
    hook_command: crate::install::claude::hook_command,
    merge_install: crate::install::claude::merge_install,
    merge_uninstall: crate::install::claude::merge_uninstall,
    needs_path_warning: true,
};

pub const TARGETS: &[&Target] = &[&CLAUDE];

pub fn by_name(name: &str) -> Option<&'static Target> {
    TARGETS.iter().copied().find(|t| t.name == name)
}

/// Detection = the config FILE exists (not merely its parent dir): an empty
/// ~/.codex must NOT count as present.
pub fn config_present(path: &Path) -> bool {
    crate::install::io::resolve_symlink(path).exists()
}

pub fn is_present(t: &Target) -> bool {
    config_present((t.default_config_path)().as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_resolves_claude_and_rejects_unknown() {
        assert_eq!(by_name("claude").unwrap().name, "claude");
        assert!(by_name("nope").is_none());
        assert!(by_name("all").is_none()); // "all" is a meta-value, not a Target
    }

    #[test]
    fn config_present_checks_file_existence() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("x.json");
        assert!(!config_present(&p));
        std::fs::write(&p, "{}").unwrap();
        assert!(config_present(&p));
    }
}
```

- [ ] **Step 2:** (verification deferred to Task 5 — crate won't build until `claude.rs` exists; this task references `crate::install::claude::*`.)

---

## Task 3: `claude.rs` — JSON merge moved from `merge.rs`, string API

**Files:**
- Create: `crates/pixtuoid/src/install/claude.rs`

- [ ] **Step 1: Create `claude.rs`.** Move the JSON merge logic and **all** tests from `merge.rs` verbatim, then wrap them in the `&str → Result<String>` API. Keep `SENTINEL_KEY`, `LEGACY_SENTINEL_KEYS`, `EVENTS`, `is_managed_entry`, and the `Value`-based `merge_install`/`merge_uninstall` as **private** helpers renamed `json_merge_install`/`json_merge_uninstall`.

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

const SENTINEL_KEY: &str = "_pixtuoid";

/// Legacy sentinel keys from previous tool names. Entries tagged with any of
/// these are stripped on install/uninstall so a v0.3.x → v0.4.x upgrade does
/// not leave orphan hooks pointing at missing binaries.
const LEGACY_SENTINEL_KEYS: &[&str] = &["_ascii_agents"];

const EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "SessionEnd",
];

pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(format!("{home}/.claude/settings.json"))
}

/// Claude writes the bare name for portability (CC spawns hooks via PATH).
/// Ignores the resolved path entirely (existence is checked by the orchestrator).
pub fn hook_command(_resolved: &Path) -> Result<String> {
    Ok("pixtuoid-hook".to_string())
}

fn parse_or_empty(content: &str) -> Result<Value> {
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(content).context("settings.json is not valid JSON — refusing to overwrite")
}

pub fn merge_install(content: &str, hook_cmd: &str) -> Result<String> {
    let doc = parse_or_empty(content)?;
    let merged = json_merge_install(doc, hook_cmd);
    Ok(serde_json::to_string_pretty(&merged)?)
}

pub fn merge_uninstall(content: &str) -> Result<String> {
    let doc = parse_or_empty(content)?;
    let cleaned = json_merge_uninstall(doc);
    Ok(serde_json::to_string_pretty(&cleaned)?)
}

fn is_managed_entry(entry: &Value) -> bool {
    if entry.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    LEGACY_SENTINEL_KEYS
        .iter()
        .any(|k| entry.get(*k).and_then(|v| v.as_bool()) == Some(true))
}

fn json_merge_install(doc: Value, hook_command: &str) -> Value {
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
            "hooks": [ { "type": "command", "command": hook_command } ]
        }));
    }
    Value::Object(root)
}

fn json_merge_uninstall(mut doc: Value) -> Value {
    let Some(root) = doc.as_object_mut() else { return doc; };
    let Some(Value::Object(hooks_obj)) = root.get_mut("hooks") else { return doc; };
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
```

- [ ] **Step 2: Move the `merge.rs` tests into `claude.rs`** under a `#[cfg(test)] mod tests`. The existing tests call `merge_install(json!({}), "/x")` returning `Value`; they now must test the **private** `json_merge_install`/`json_merge_uninstall` (same `Value` API) so they transfer verbatim with only the function-name change. Copy ALL 9 tests from `merge.rs` (`install_creates_entries_for_all_events`, `install_is_idempotent`, `install_preserves_unrelated_entries`, `uninstall_removes_sentinel_entries_only`, `uninstall_drops_empty_hooks_map`, `install_strips_legacy_ascii_agents_entries`, `uninstall_strips_legacy_ascii_agents_entries`, `uninstall_strips_legacy_keeps_user_entries`, `uninstall_non_array_hook_value_does_not_panic`), renaming `merge_install`→`json_merge_install` and `merge_uninstall`→`json_merge_uninstall` and `SENTINEL_KEY` stays. Then ADD a string-API guard:

```rust
    #[test]
    fn merge_install_on_empty_string_produces_valid_populated_config() {
        let out = merge_install("", "pixtuoid-hook").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["hooks"]["PreToolUse"][0][SENTINEL_KEY].as_bool().unwrap());
    }

    #[test]
    fn merge_uninstall_on_empty_string_is_noop() {
        let out = merge_uninstall("").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("hooks").is_none());
    }

    #[test]
    fn merge_install_rejects_invalid_json() {
        assert!(merge_install("{not json", "pixtuoid-hook").is_err());
    }
```

- [ ] **Step 3:** (verification at Task 5.)

---

## Task 4: `mod.rs` — orchestrator + pure `plan_targets` + confirm + dispatch

**Files:**
- Modify: `crates/pixtuoid/src/install/mod.rs`

- [ ] **Step 1: Write the pure-`plan_targets` failing tests** (these are the policy oracle; they need no filesystem/stdin)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::target::{CLAUDE, Target};

    // A second fake target for "both present" rows (avoids depending on Phase 2's CODEX).
    static FAKE: Target = Target {
        name: "fake", display_name: "Fake", restart_noun: "Fake",
        default_config_path: || std::path::PathBuf::from("/nonexistent/fake"),
        hook_command: |_| Ok("x".into()),
        merge_install: |c, _| Ok(c.to_string()),
        merge_uninstall: |c| Ok(c.to_string()),
        needs_path_warning: false,
    };

    fn present(claude: bool, fake: bool) -> Vec<(&'static Target, bool)> {
        vec![(&CLAUDE, claude), (&FAKE, fake)]
    }

    #[test]
    fn explicit_target_claude_ignores_detection() {
        let p = plan_targets(Some("claude"), false, &present(false, false), false, false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn explicit_all_with_config_is_conflict() {
        let p = plan_targets(Some("all"), true, &present(true, true), false, true);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_tty_returns_detected() {
        let p = plan_targets(None, false, &present(true, true), false, true);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 2));
    }

    #[test]
    fn no_target_non_tty_single_claude_installs_claude() {
        let p = plan_targets(None, false, &present(true, false), false, false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn no_target_non_tty_multiple_present_is_conflict() {
        let p = plan_targets(None, false, &present(true, true), false, false);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_nothing_present_is_nothing_detected() {
        let p = plan_targets(None, false, &present(false, false), false, false);
        assert!(matches!(p, Plan::NothingDetected));
    }

    #[test]
    fn confirm_answer_parses_default_yes() {
        assert!(parse_confirm(""));
        assert!(parse_confirm("y"));
        assert!(parse_confirm("YES"));
        assert!(!parse_confirm("n"));
        assert!(!parse_confirm("no"));
        assert!(!parse_confirm("garbage")); // anything not yes/empty → no
    }
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p pixtuoid install::mod 2>&1 | head -20`
Expected: compile errors — `plan_targets`, `Plan`, `parse_confirm` undefined.

- [ ] **Step 3: Rewrite `mod.rs`** in full:

```rust
pub mod claude;
pub mod io;
pub mod target;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{bail, Result};

use target::{Target, BACKUP_SUFFIX};

pub struct InstallArgs {
    pub hook_path: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub target: Option<String>,
    pub yes: bool,
}

pub struct UninstallArgs {
    pub config: Option<PathBuf>,
    pub target: Option<String>,
    pub yes: bool,
}

pub enum Plan {
    Targets(Vec<&'static Target>),
    NothingDetected,
    Conflict(String),
}

/// Pure policy: decide which targets to act on. No filesystem, no stdin.
/// `present` is the injected detection result; `explicit_config` is whether
/// `--config` was passed (only valid for a single target).
pub fn plan_targets(
    requested: Option<&str>,
    explicit_config: bool,
    present: &[(&'static Target, bool)],
    _yes: bool,
    is_tty: bool,
) -> Plan {
    match requested {
        Some("all") => {
            if explicit_config {
                return Plan::Conflict(
                    "--config applies to a single target; use --target claude|codex".into(),
                );
            }
            let chosen: Vec<_> = present.iter().filter(|(_, p)| *p).map(|(t, _)| *t).collect();
            if chosen.is_empty() { Plan::NothingDetected } else { Plan::Targets(chosen) }
        }
        Some(name) => match target::by_name(name) {
            Some(t) => Plan::Targets(vec![t]),
            None => Plan::Conflict(format!("unknown target: {name}")),
        },
        None => {
            // `--config`/`--settings` without `--target` is the legacy Claude-only
            // contract (pre-multi-CLI scripts). The supplied path IS the target
            // selection signal — `$HOME` detection is meaningless here — so default
            // to Claude rather than coupling the explicit path to ambient detection.
            // (This is why the `--settings` back-compat oracle keeps passing once
            // CODEX joins TARGETS and detection could see 0 or 2 present CLIs.)
            if explicit_config {
                return match target::by_name("claude") {
                    Some(t) => Plan::Targets(vec![t]),
                    None => Plan::Conflict("claude target not registered".into()),
                };
            }
            let detected: Vec<_> = present.iter().filter(|(_, p)| *p).map(|(t, _)| *t).collect();
            match detected.len() {
                0 => Plan::NothingDetected,
                1 => Plan::Targets(detected), // TTY or not: a single detected target is safe
                _ if is_tty => Plan::Targets(detected), // caller confirms interactively
                _ => Plan::Conflict(
                    "multiple CLIs detected; pass --target claude|codex|all".into(),
                ),
            }
        }
    }
}

/// Parse a confirm answer: empty/Enter or y/yes → true; anything else → false.
fn parse_confirm(answer: &str) -> bool {
    let a = answer.trim().to_ascii_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    parse_confirm(&line)
}

fn detection() -> Vec<(&'static Target, bool)> {
    target::TARGETS.iter().map(|t| (*t, target::is_present(t))).collect()
}

pub fn install(args: InstallArgs) -> Result<()> {
    let present = detection();
    let is_tty = std::io::stdin().is_terminal();
    let plan = plan_targets(args.target.as_deref(), args.config.is_some(), &present, args.yes, is_tty);
    let targets = resolve_plan(plan, args.target.as_deref())?;
    // Confirm ONLY on bare auto-detect (no --target) resolving to >1 target.
    // An explicit --target (incl. `all`) is treated as intent and skips the prompt.
    if args.target.is_none() && targets.len() > 1 && !args.yes && is_tty {
        let names: Vec<_> = targets.iter().map(|t| t.display_name).collect();
        if !confirm(&format!("install pixtuoid hooks into {}?", names.join(" + "))) {
            println!("aborted");
            return Ok(());
        }
    }
    for t in targets {
        run_install(t, args.config.clone(), args.hook_path.clone())?;
    }
    Ok(())
}

pub fn uninstall(args: UninstallArgs) -> Result<()> {
    let present = detection();
    let is_tty = std::io::stdin().is_terminal();
    let plan = plan_targets(args.target.as_deref(), args.config.is_some(), &present, args.yes, is_tty);
    let targets = resolve_plan(plan, args.target.as_deref())?;
    for t in targets {
        run_uninstall(t, args.config.clone())?;
    }
    Ok(())
}

fn resolve_plan(plan: Plan, _requested: Option<&str>) -> Result<Vec<&'static Target>> {
    match plan {
        Plan::Targets(t) => Ok(t),
        Plan::NothingDetected => {
            println!("no supported CLIs detected; pass --target claude|codex|all");
            Ok(vec![])
        }
        Plan::Conflict(msg) => bail!(msg),
    }
}

fn run_install(t: &Target, config: Option<PathBuf>, hook_path: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let binary = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    let hook_cmd = (t.hook_command)(&binary)?;
    let content = io::read_config(&path)?;
    let merged = (t.merge_install)(&content, &hook_cmd)?;
    if merged == content {
        println!("[{}] already up to date — {}", t.name, path.display());
        return Ok(());
    }
    let backup = io::backup_once(&path, BACKUP_SUFFIX)?;
    io::write_config_atomic(&path, &merged)?;
    println!("ok: installed pixtuoid hooks into {} ({})", path.display(), t.display_name);
    if let Some(b) = backup {
        println!("backup: {} (removed automatically on uninstall-hooks)", b.display());
    }
    if t.needs_path_warning && !io::hook_on_path() {
        println!("warn: `pixtuoid-hook` not found on PATH (checked against this shell).");
        println!("      Install it on PATH, e.g. `cargo install --path crates/pixtuoid-hook`.");
    }
    println!("→ start a new {} session for this to take effect.", t.restart_noun);
    Ok(())
}

fn run_uninstall(t: &Target, config: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let content = io::read_config(&path)?;
    let cleaned = (t.merge_uninstall)(&content)?;
    if cleaned == content {
        // Covers both file-absent (content == "") and no-match. The backup is
        // the user's only recovery path — never destroyed on a no-op.
        println!("[{}] no pixtuoid hooks found in {} — nothing to remove", t.name, path.display());
        return Ok(());
    }
    io::write_config_atomic(&path, &cleaned)?;
    println!("ok: removed pixtuoid hooks from {} ({})", path.display(), t.display_name);
    if let Some(b) = io::remove_backup(&path, BACKUP_SUFFIX)? {
        println!("removed backup: {}", b.display());
    }
    println!("→ start a new {} session for this to take effect.", t.restart_noun);
    Ok(())
}
```

- [ ] **Step 4:** (verification at Task 5; `cli.rs`/`main.rs` still reference old signatures.)

> Note: `merge_install`/`merge_uninstall` round-trip through `to_string_pretty`, so a no-op re-run produces text byte-identical to a fresh read only if the file was already pretty-printed. For the "already up to date" short-circuit, compare `merged == content`; on a hand-formatted file the first install reformats (acceptable — backup is taken). This matches the spec's documented canonicalization behavior.

---

## Task 5: `cli.rs` + `main.rs` — new flags, dispatch, delete `merge.rs`; first green commit

**Files:**
- Modify: `crates/pixtuoid/src/cli.rs`
- Modify: `crates/pixtuoid/src/main.rs`
- Delete: `crates/pixtuoid/src/install/merge.rs`
- Modify: `crates/pixtuoid/tests/install.rs`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update `cli.rs`** — add `TargetName`, change the two variants, update the `mod tests` constructor:

Change `use clap::{Parser, Subcommand};` → `use clap::{Parser, Subcommand, ValueEnum};`. Replace the two variants:

```rust
    /// Install pixtuoid hooks into agent CLI config(s).
    InstallHooks {
        #[arg(long)]
        hook_path: Option<PathBuf>,
        /// Config file override (single target only; conflicts with --target all).
        #[arg(long, alias = "settings")]
        config: Option<PathBuf>,
        #[arg(long, value_enum)]
        target: Option<TargetName>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Remove pixtuoid hook entries from agent CLI config(s).
    UninstallHooks {
        #[arg(long, alias = "settings")]
        config: Option<PathBuf>,
        #[arg(long, value_enum)]
        target: Option<TargetName>,
        #[arg(long, short = 'y')]
        yes: bool,
    },
```

Add after the `Cmd` enum:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TargetName {
    Claude,
    Codex,
    All,
}

impl TargetName {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetName::Claude => "claude",
            TargetName::Codex => "codex",
            TargetName::All => "all",
        }
    }
}
```

Update the `cmd_or_default_preserves_explicit_subcommand` test constructor:

```rust
            cmd: Some(Cmd::UninstallHooks {
                config: None,
                target: None,
                yes: false,
            }),
```

- [ ] **Step 2: Update `main.rs` dispatch** — replace lines 72–76:

```rust
        Cmd::InstallHooks {
            hook_path,
            config,
            target,
            yes,
        } => install::install(install::InstallArgs {
            hook_path,
            config,
            target: target.map(|t| t.as_str().to_string()),
            yes,
        }),
        Cmd::UninstallHooks {
            config,
            target,
            yes,
        } => install::uninstall(install::UninstallArgs {
            config,
            target: target.map(|t| t.as_str().to_string()),
            yes,
        }),
```

- [ ] **Step 3: Delete `merge.rs`**

Run: `git rm crates/pixtuoid/src/install/merge.rs`

(Its logic + tests now live in `claude.rs`. `mod.rs` no longer declares `pub mod merge;`.)

- [ ] **Step 4: Update `tests/install.rs`** — keep the existing `install_then_uninstall_round_trip` (uses `--settings`) **UNCHANGED** as the back-compat oracle. Add a second test:

```rust
#[test]
fn install_with_config_and_target_flags() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target", "claude",
            "--config", settings.to_str().unwrap(),
            "--hook-path", "/fake/path",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert!(v["hooks"]["PreToolUse"][0]["_pixtuoid"].as_bool().unwrap());
}
```

- [ ] **Step 5: Update `CLAUDE.md`** — in the "Things NOT to do" and "Where to look" sections, rename `write_settings_atomic` → `write_config_atomic`, and update the `install/` layout line to "multi-target (Claude + Codex) hook install via the `Target` registry; format-neutral atomic write, advisory lock, stow-symlink safe". (Search: `grep -n write_settings_atomic CLAUDE.md` and replace each.)

- [ ] **Step 6: Build + full test + preflight**

Run: `cargo build -p pixtuoid && cargo test -p pixtuoid && ./scripts/preflight.sh`
Expected: PASS — all `install::io`, `install::claude`, `install::mod`, `install::target` unit tests; `tests/install.rs` both tests (`--settings` oracle + `--config`); clippy `-D warnings` clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(install): extensible Target abstraction + format-neutral io (phase 1)

Claude behavior preserved; merge.rs JSON logic + all tests moved to
claude.rs (incl. _ascii_agents legacy migration). io.rs is now String/
suffix-based with string-append sibling paths. Pure plan_targets policy.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

# PHASE 2 — Codex target

## Task 6: `codex.rs` — TOML merge with sentinel + group-emptying

**Files:**
- Create: `crates/pixtuoid/src/install/codex.rs`

Port #59's TOML logic with the spec-required changes: **drop `[features] hooks = true`**, `timeout = 5`, `statusMessage = "pixtuoid visualizer"`, add `_pixtuoid = true` handler sentinel, add `PermissionRequest`, and make uninstall sentinel-primary (basename fallback).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> toml::Value { toml::from_str(s).unwrap() }

    #[test]
    fn install_creates_groups_for_all_events_with_sentinel() {
        let out = merge_install("", "PIXTUOID_SOURCE=codex /opt/bin/pixtuoid-hook").unwrap();
        let v = parse(&out);
        for ev in CODEX_EVENTS {
            let arr = v["hooks"][*ev].as_array().unwrap();
            assert_eq!(arr.len(), 1, "event {ev}");
            let handler = &arr[0]["hooks"][0];
            assert_eq!(handler["command"].as_str().unwrap(), "PIXTUOID_SOURCE=codex /opt/bin/pixtuoid-hook");
            assert_eq!(handler["timeout"].as_integer().unwrap(), 5);
            assert_eq!(handler["statusMessage"].as_str().unwrap(), "pixtuoid visualizer");
            assert_eq!(handler["_pixtuoid"].as_bool().unwrap(), true);
        }
    }

    #[test]
    fn install_does_not_write_features_hooks() {
        let out = merge_install("", "/x").unwrap();
        let v = parse(&out);
        assert!(v.get("features").is_none(), "must not write [features] hooks = true");
    }

    #[test]
    fn install_is_idempotent_across_different_paths() {
        // Sentinel (not basename/path) drives replacement → re-install with a
        // different resolved path replaces, never duplicates.
        let a = merge_install("", "/opt/a/pixtuoid-hook").unwrap();
        let b = merge_install(&a, "/opt/b/pixtuoid-hook").unwrap();
        let v = parse(&b);
        for ev in CODEX_EVENTS {
            assert_eq!(v["hooks"][*ev].as_array().unwrap().len(), 1, "event {ev} duplicated");
        }
    }

    #[test]
    fn uninstall_keeps_user_handler_in_mixed_group() {
        // A group with one managed + one user handler: uninstall strips only ours.
        let installed = merge_install("", "/x/pixtuoid-hook").unwrap();
        let mut v = parse(&installed);
        // inject a user handler into the PreToolUse group
        let group = &mut v["hooks"]["PreToolUse"].as_array_mut().unwrap()[0];
        group["hooks"].as_array_mut().unwrap().push(toml::Value::Table({
            let mut t = toml::value::Table::new();
            t.insert("type".into(), "command".into());
            t.insert("command".into(), "/usr/bin/mytool".into());
            t
        }));
        let cleaned = merge_uninstall(&toml::to_string_pretty(&v).unwrap()).unwrap();
        let cv = parse(&cleaned);
        let arr = cv["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "group kept (user handler remains)");
        let hooks = arr[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"].as_str().unwrap(), "/usr/bin/mytool");
    }

    #[test]
    fn uninstall_removes_empty_groups_and_events() {
        let installed = merge_install("", "/x/pixtuoid-hook").unwrap();
        let cleaned = merge_uninstall(&installed).unwrap();
        let v = parse(&cleaned);
        assert!(v.get("hooks").is_none(), "all managed → hooks table dropped: {cleaned}");
    }

    #[test]
    fn uninstall_legacy_basename_fallback() {
        // A pre-sentinel #59 entry (no _pixtuoid, command basename pixtuoid-hook) is removed.
        let cfg = r#"
[[hooks.PreToolUse]]
matcher = "*"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/old/pixtuoid-hook"
"#;
        let cleaned = merge_uninstall(cfg).unwrap();
        let v = parse(&cleaned);
        assert!(v.get("hooks").is_none(), "legacy basename entry removed: {cleaned}");
    }
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test -p pixtuoid install::codex 2>&1 | head -20`
Expected: compile error — `codex` module / functions undefined.

- [ ] **Step 3: Implement `codex.rs`**

```rust
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use toml::value::Table;

const SENTINEL_KEY: &str = "_pixtuoid";

const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "PermissionRequest",
];

pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(format!("{home}/.codex/config.toml"))
}

/// Codex runs the `command` string under a shell; we write an ABSOLUTE path
/// (robust regardless of PATH) prefixed with PIXTUOID_SOURCE so the shim can
/// stamp the source. Err on non-UTF-8 (prevents the to_string_lossy dead-hook).
pub fn hook_command(resolved: &Path) -> Result<String> {
    let p = resolved
        .to_str()
        .ok_or_else(|| anyhow!("pixtuoid-hook path is non-UTF-8: {}", resolved.display()))?;
    Ok(format!("PIXTUOID_SOURCE=codex {p}"))
}

fn parse_or_empty(content: &str) -> Result<toml::Value> {
    if content.trim().is_empty() {
        return Ok(toml::Value::Table(Table::new()));
    }
    toml::from_str(content).context("config.toml is not valid TOML — refusing to overwrite")
}

pub fn merge_install(content: &str, hook_cmd: &str) -> Result<String> {
    let doc = parse_or_empty(content)?;
    let merged = toml_merge_install(doc, hook_cmd);
    Ok(toml::to_string_pretty(&merged)?)
}

pub fn merge_uninstall(content: &str) -> Result<String> {
    let doc = parse_or_empty(content)?;
    let cleaned = toml_merge_uninstall(doc);
    Ok(toml::to_string_pretty(&cleaned)?)
}

fn command_basename_is_hook(command: &str) -> bool {
    // The command string may be "PIXTUOID_SOURCE=codex /path/pixtuoid-hook";
    // take the last whitespace-separated token, then its file_name.
    let token = command.split_whitespace().last().unwrap_or(command);
    Path::new(token).file_name().and_then(|s| s.to_str()) == Some("pixtuoid-hook")
}

fn handler_is_managed(h: &toml::Value) -> bool {
    if h.get(SENTINEL_KEY).and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    // Legacy fallback for pre-sentinel (#59) entries.
    h.get("type").and_then(|v| v.as_str()) == Some("command")
        && h.get("command").and_then(|v| v.as_str()).is_some_and(command_basename_is_hook)
}

fn prune_managed_handlers(group: &mut toml::Value) {
    if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
        hooks.retain(|h| !handler_is_managed(h));
    }
}

fn group_has_no_hooks(group: &toml::Value) -> bool {
    group.get("hooks").and_then(|h| h.as_array()).is_some_and(|h| h.is_empty())
}

fn managed_group(event: &str, hook_command: &str) -> toml::Value {
    let mut handler = Table::new();
    handler.insert("type".into(), toml::Value::String("command".into()));
    handler.insert("command".into(), toml::Value::String(hook_command.into()));
    handler.insert("timeout".into(), toml::Value::Integer(5));
    handler.insert("statusMessage".into(), toml::Value::String("pixtuoid visualizer".into()));
    handler.insert(SENTINEL_KEY.into(), toml::Value::Boolean(true));

    let mut group = Table::new();
    if matches!(event, "PreToolUse" | "PostToolUse" | "SubagentStart" | "SubagentStop" | "PermissionRequest") {
        group.insert("matcher".into(), toml::Value::String("*".into()));
    } else if event == "SessionStart" {
        group.insert("matcher".into(), toml::Value::String("startup|resume|clear|compact".into()));
    }
    group.insert("hooks".into(), toml::Value::Array(vec![toml::Value::Table(handler)]));
    toml::Value::Table(group)
}

fn toml_merge_install(doc: toml::Value, hook_command: &str) -> toml::Value {
    let mut root = doc.as_table().cloned().unwrap_or_default();
    let hooks = root.entry("hooks".to_string()).or_insert_with(|| toml::Value::Table(Table::new()));
    if !hooks.is_table() {
        *hooks = toml::Value::Table(Table::new());
    }
    if let Some(hooks) = hooks.as_table_mut() {
        for ev in CODEX_EVENTS {
            let entry = hooks.entry((*ev).to_string()).or_insert_with(|| toml::Value::Array(vec![]));
            if !entry.is_array() {
                *entry = toml::Value::Array(vec![]);
            }
            if let Some(arr) = entry.as_array_mut() {
                for group in arr.iter_mut() {
                    prune_managed_handlers(group);
                }
                arr.retain(|group| !group_has_no_hooks(group));
                arr.push(managed_group(ev, hook_command));
            }
        }
    }
    toml::Value::Table(root)
}

fn toml_merge_uninstall(mut doc: toml::Value) -> toml::Value {
    let Some(root) = doc.as_table_mut() else { return doc; };
    let Some(toml::Value::Table(hooks)) = root.get_mut("hooks") else { return doc; };
    for (_ev, list) in hooks.iter_mut() {
        if let Some(arr) = list.as_array_mut() {
            for group in arr.iter_mut() {
                prune_managed_handlers(group);
            }
            arr.retain(|group| !group_has_no_hooks(group));
        }
    }
    let empty: Vec<String> = hooks
        .iter()
        .filter_map(|(k, v)| match v.as_array() {
            Some(a) if a.is_empty() => Some(k.clone()),
            _ => None,
        })
        .collect();
    for k in empty {
        hooks.remove(&k);
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    doc
}
```

- [ ] **Step 4: Run codex unit tests**

Run: `cargo test -p pixtuoid install::codex`
Expected: PASS (module compiles standalone; `codex` is declared in `mod.rs` in Task 7. To run now, add `pub mod codex;` to `mod.rs` first — do that as the first edit of Task 7, or temporarily here.)

---

## Task 7: Register `CODEX` in the registry + wire the module

**Files:**
- Modify: `crates/pixtuoid/src/install/mod.rs`
- Modify: `crates/pixtuoid/src/install/target.rs`

- [ ] **Step 1: Add `pub mod codex;`** to `mod.rs` (next to `pub mod claude;`).

- [ ] **Step 2: Add the `CODEX` const + register it** in `target.rs`:

```rust
pub const CODEX: Target = Target {
    name: "codex",
    display_name: "Codex",
    restart_noun: "Codex",
    default_config_path: crate::install::codex::default_config_path,
    hook_command: crate::install::codex::hook_command,
    merge_install: crate::install::codex::merge_install,
    merge_uninstall: crate::install::codex::merge_uninstall,
    needs_path_warning: false,
};

pub const TARGETS: &[&Target] = &[&CLAUDE, &CODEX];
```

(Replace the Phase-1 `TARGETS: &[&Target] = &[&CLAUDE];`.)

- [ ] **Step 3: Add a `codex::hook_command` non-UTF-8 test** to `codex.rs` tests (Unix-only):

```rust
    #[test]
    #[cfg(unix)]
    fn hook_command_errors_on_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;
        let bad = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/x/\xff/pixtuoid-hook"));
        assert!(hook_command(bad).is_err());
    }

    #[test]
    fn hook_command_prefixes_source_for_valid_path() {
        let cmd = hook_command(std::path::Path::new("/opt/bin/pixtuoid-hook")).unwrap();
        assert_eq!(cmd, "PIXTUOID_SOURCE=codex /opt/bin/pixtuoid-hook");
    }
```

- [ ] **Step 4: Build + test**

Run: `cargo test -p pixtuoid install::`
Expected: PASS — `install::codex::*`, `install::target::*` (now incl. `by_name("codex")`), all phase-1 tests still green.

- [ ] **Step 5: Add a `by_name("codex")` assertion** to `target.rs`'s `by_name_resolves_claude_and_rejects_unknown` test:

```rust
        assert_eq!(by_name("codex").unwrap().name, "codex");
```

Run: `cargo test -p pixtuoid install::target`
Expected: PASS.

---

## Task 8: End-to-end `--target codex` + non-TTY policy integration

**Files:**
- Modify: `crates/pixtuoid/tests/install.rs`

- [ ] **Step 1: Add integration tests** exercising the real binary against a temp Codex config and the non-TTY policy:

```rust
#[test]
fn install_codex_writes_toml_with_sentinel_and_backup() {
    let dir = TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "model = \"o1\"\n").unwrap(); // pre-existing user content
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target", "codex",
            "--config", cfg.to_str().unwrap(),
            "--hook-path", "/fake/pixtuoid-hook",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["model"].as_str().unwrap(), "o1", "user content preserved");
    assert!(v["hooks"]["PreToolUse"][0]["hooks"][0]["_pixtuoid"].as_bool().unwrap());
    assert!(v.get("features").is_none(), "no [features] hooks = true");
    // backup created with the correct multi-dot name
    assert!(dir.path().join("config.toml.pixtuoid.bak").exists());

    // uninstall restores + removes backup
    let status = std::process::Command::new(bin)
        .args(["uninstall-hooks", "--target", "codex", "--config", cfg.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(v.get("hooks").is_none());
    assert_eq!(v["model"].as_str().unwrap(), "o1");
    assert!(!dir.path().join("config.toml.pixtuoid.bak").exists());
}

#[test]
fn install_unknown_target_errors() {
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args(["install-hooks", "--target", "bogus"])
        .status()
        .unwrap();
    // clap rejects an invalid ValueEnum value → non-zero exit.
    assert!(!status.success());
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p pixtuoid --test install`
Expected: PASS — Codex round-trip, user content preserved, backup name correct.

- [ ] **Step 3: Full preflight**

Run: `./scripts/preflight.sh`
Expected: PASS — clippy `-D warnings` clean (watch for unused `_yes` param — it is intentionally unused in `plan_targets`; the leading underscore silences it).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(install): Codex target (config.toml) with sentinel + group-emptying

Ports #59's TOML merge with fixes: drop redundant [features] hooks=true,
timeout 5, honest statusMessage, _pixtuoid handler sentinel (surgical,
path-independent uninstall), PermissionRequest event, PIXTUOID_SOURCE
hook command (Err on non-UTF-8). Auto-detect + non-TTY policy via
std::io::IsTerminal.

Co-Authored-By: trjh
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Manual Codex-session verification checkpoint (human gate)

**Not automated.** This gates Phase 3 (the separate decoder plan). On a machine with a recent Codex (`codex --version` post-~Apr 2026):

- [ ] Build + install the hook binary: `cargo install --path crates/pixtuoid-hook`.
- [ ] `pixtuoid install-hooks --target codex` (or `--config` to a scratch copy of `~/.codex/config.toml`).
- [ ] Confirm `~/.codex/config.toml.pixtuoid.bak` exists; diff it against the live file to confirm the merge is surgical (only `[hooks.*]` added, user content untouched).
- [ ] Confirm `[[hooks.PreToolUse]]` carries `command = "PIXTUOID_SOURCE=codex /…/pixtuoid-hook"` and `_pixtuoid = true`.
- [ ] If Codex prompts to **trust** the hook (`trusted_hash` model), approve it.
- [ ] Start a Codex session; run `pixtuoid run` in another pane; trigger a **Bash/exec** tool in Codex; tail the socket and confirm a JSON object with `hook_event_name` + `tool_use_id` arrives, and `payload["source"] == "codex"` (verifies the shell-exec env-prefix works — if `source` is absent, switch `codex::hook_command` to the `--source codex` argv form per the spec).
- [ ] `pixtuoid uninstall-hooks --target codex`; confirm config restored + backup removed.
- [ ] Record findings (Codex version, whether source stamping worked, trust-model behavior) — these feed the Phase 3 plan.

---

## Self-review notes (for the implementer)

- **Do NOT** convert `const CLAUDE/CODEX/TARGETS` to `static` — `&CONST` in `const` compiles via static promotion (verified).
- **Do NOT** copy `[features] hooks = true` from #59 — it's a redundant no-op (Codex hooks default on).
- The `plan_targets` `_yes` param is intentionally unused (the confirm happens in `install()`); keep the leading underscore. If clippy still complains, drop the param and pass `yes` only to `install()`.
- `toml::to_string_pretty` canonicalizes `config.toml` (strips comments). The backup is the recovery path; this is documented in the spec's Risks.
