# Multi-CLI Hook Install Design

**Date:** 2026-05-29
**Status:** Draft
**Author:** Ivan + Claude (seeds from PR #59 by @trjh)

## Overview

Refactor the Claude-only `install-hooks` / `uninstall-hooks` commands into an extensible **multi-CLI** install layer so a single machine running both Claude Code and Codex (and, later, Cursor/Copilot) can install pixtuoid hooks into each one. The driving constraint: adding the *next* hook-supported CLI should be roughly **one new module + one registry line**, with the file-format, config-path, and merge logic encapsulated per-CLI and the safety machinery (atomic write, advisory lock, symlink resolution, backup) shared so it can never be skipped.

Codex is the second target that proves the abstraction. Its hook support has been verified against OpenAI's official docs and the shipped `openai/codex` source (see [Background: Codex ground truth](#background-codex-ground-truth)).

This spec supersedes the design in PR #59 (`trjh`, branch `codex-hook-support`), whose Codex TOML merge logic is reused as the basis for the Codex target. PR #59 is **absorbed** into this work (author credited), not merged separately.

## Goals

1. A single `install-hooks` / `uninstall-hooks` that targets multiple agent CLIs from one shared, safe code path.
2. Add a new single-file-config CLI in ~one module + one registry entry; per-CLI format knowledge lives in that module only.
3. Claude behavior is byte-for-byte preserved (pure refactor in phase 1).
4. Codex install/uninstall is **surgical** (sentinel-based, like Claude) and idempotent regardless of the resolved binary path.
5. Auto-detect installed CLIs with an interactive confirm; never silently mutate a high-value config (`~/.codex/config.toml`) in a non-interactive context. (The non-TTY single-Claude path is an intentional **backward-compatibility exception**: Claude is the legacy default and pre-multi-CLI scripts rely on non-interactive `install-hooks` working. Codex — the high-value config — is never written silently.)
6. Structurally prevent the four review findings from PR #59 (backup omission, backup-name corruption, lossy hook path, source misrouting).
7. Codex sessions render as sprites end-to-end, attributed to the `codex` source via an explicit tag (not path-sniffing).

## Non-Goals

- Directory-of-files hook targets (e.g. a `hooks.d/`-style CLI). The abstraction is intentionally a **single config file, single content blob**. YAML single-file targets fit; directory targets are a future extension, not pre-built.
- A second hook socket or per-CLI socket isolation. The shim stays single-socket, single-binary (invariant #5).
- Wiring Codex's `notify` program (single `agent-turn-complete`, argv payload) — unsuitable for live tool-call visualization.
- Codex's `PreCompact` / `PostCompact` events (documented as intentionally ignored for now).
- Windows support (`commandWindows` field is left unset).

## Decisions (resolved)

| # | Decision | Choice |
|---|----------|--------|
| 1 | PR #59 relationship | **Absorb** its TOML merge logic into `codex.rs`; credit `trjh` as co-author; close #59 with thanks. |
| 2 | Abstraction shape | **No trait / no `dyn`.** A `const`-data `Target` struct (fn-pointers for format-specific bits) + a `const TARGETS` slice + one shared `run_install`/`run_uninstall` orchestrator. (The architecture review rejected a `trait HookTarget` as over-abstraction for a fixed, synchronous, ~2-target set.) |
| 3 | Source attribution | **Explicit tag.** Codex's hook command sets `PIXTUOID_SOURCE=codex`; the shim stamps `payload["source"]`; the decoder reads the explicit field. Shared single socket. No `transcript_path` sniffing. |
| 4 | Non-TTY + multiple CLIs present | **Error** + "pass `--target`". Never silently write Codex config in a non-interactive run. |
| 5 | Codex event set | #59's 7 events **+ `PermissionRequest`** (maps to the `Waiting` state). `PreCompact`/`PostCompact` documented as ignored. |
| 6 | Codex sentinel `_pixtuoid = true` | **Use it.** Verified `codex-rs/config/src/hook_config.rs` has no `deny_unknown_fields`; serde ignores the extra key, and pixtuoid round-trips the file as an untyped `toml::Value`, so the sentinel survives. Enables surgical uninstall. |
| 7 | TTY detection | `std::io::IsTerminal` (`std::io::stdin().is_terminal()`). No `libc`, no `unsafe`. MSRV 1.78 ≥ 1.70. |

## Background: current Claude-only architecture

(`crates/pixtuoid/src/install/`)

- `mod.rs` — `install(hook_path, settings)` / `uninstall(settings)`. Hardcoded to `~/.claude/settings.json`. Flow: `read_settings` → `merge::merge_install` → if changed `backup_once` + `write_settings_atomic`; on uninstall, `merge_uninstall` + `remove_backup`. Prints "Claude Code session".
- `io.rs` — `read_settings() -> Value` / `write_settings_atomic(&Value)` (advisory lock + tmp + atomic rename + `resolve_symlink`), `backup_once`/`remove_backup` (hardcoded `with_extension("json.pixtuoid.bak")`), `hook_on_path`, `default_hook_binary`, `resolve_symlink`. These last three are already format-neutral.
- `merge.rs` — JSON merge keyed on the `_pixtuoid` sentinel + an `EVENTS` list. `merge_install` injects managed hook entries (sentinel-marked); `merge_uninstall` removes only sentinel-marked entries (surgical).

Load-bearing invariants (CLAUDE.md): all settings writes go through `write_settings_atomic` (lock + atomic rename + symlink resolution); **the backup is the user's only recovery path** — never destroyed on a no-op uninstall; surgical uninstall via sentinel.

## Background: Codex ground truth

Verified against `developers.openai.com/codex/hooks`, `/config-reference`, `/config-advanced`, and `openai/codex` source (`codex-rs/hooks/src/schema.rs`, `codex-rs/config/src/hook_config.rs`) on 2026-05-29.

- Codex CLI **does** support lifecycle hooks in `~/.codex/config.toml`, a near-clone of Claude Code's: `[hooks]` table keyed by PascalCase event name → array of *groups* → each group has optional `matcher` (regex; `*`/`""`/omitted = match-all) + a `hooks` array of `{ type = "command", command, timeout, statusMessage }` handlers.
- **Every command hook receives one JSON object on stdin** (`session_id`, `transcript_path`, `cwd`, `hook_event_name`, `model`, and `tool_name`/`tool_use_id`/`tool_input` on `PreToolUse`) — same contract our shim + decoder already rely on.
- **Hooks are enabled by default.** `[features] hooks` is an *opt-out* (`= false`). Writing `hooks = true` is a redundant no-op → **we will not write it.**
- Full event set is **10**: `PreToolUse, PermissionRequest, PostToolUse, PreCompact, PostCompact, SessionStart, UserPromptSubmit, SubagentStart, SubagentStop, Stop`.
- `command` is a single **string** (shell command line; the docs example uses `$(...)`), not argv. Writing an **absolute** path is robust; bare-name PATH resolution is *likely* (shell) but not contractually documented.
- `hook_config.rs` has **no `deny_unknown_fields`** → our `_pixtuoid` sentinel key is tolerated.

**Version sensitivity (for live verification):** `PreToolUse`/`PostToolUse` landed *after* the original experimental hooks (Codex `v0.114.0` shipped only `SessionStart` + `Stop`, PR #13276); present on main/docs as of May 2026. `apply_patch`/MCP-tool hook coverage was patched ~Apr–May 2026. There is a `[hooks.state]` table with `trusted_hash` → Codex appears to have a hook trust/hash-pinning model, so a freshly-installed hook **may require approval before it fires**. The user's `codex --version` should be post-~April 2026.

## Architecture

### The `Target` data struct (no trait, no `dyn`)

`crates/pixtuoid/src/install/target.rs`:

```rust
use std::path::{Path, PathBuf};
use anyhow::Result;

/// A single install destination (one CLI's config file). Fixed set, resolved at
/// compile time as `const` data — no dyn dispatch (install runs once, synchronously).
pub struct Target {
    /// Stable lowercase id: "claude" | "codex". Matches the source-side name prefix.
    pub name: &'static str,
    /// Human-readable name for CLI output.
    pub display_name: &'static str,
    /// Restart noun for the "→ start a new <noun> session" hint.
    pub restart_noun: &'static str,
    /// Default config path (reads $HOME, hence a fn not a const).
    pub default_config_path: fn() -> PathBuf,
    /// Build the command string written into config from the resolved binary path.
    /// Claude returns bare "pixtuoid-hook"; Codex returns the full path with a
    /// PIXTUOID_SOURCE prefix, or Err on non-UTF-8 (kills the to_string_lossy bug).
    pub hook_command: fn(resolved: &Path) -> Result<String>,
    /// Parse `content`, inject managed hook entries, reserialize. MUST treat
    /// empty/whitespace-only content as the empty document — never error on empty.
    pub merge_install: fn(content: &str, hook_cmd: &str) -> Result<String>,
    /// Parse `content`, remove only sentinel-marked entries, reserialize. Same empty rule.
    pub merge_uninstall: fn(content: &str) -> Result<String>,
    /// True if the bare hook name must resolve on PATH (Claude writes the bare name).
    pub needs_path_warning: bool,
}

pub const BACKUP_SUFFIX: &str = "pixtuoid.bak"; // same const for every target

pub const CLAUDE: Target = Target { name: "claude", display_name: "Claude Code",
    restart_noun: "Claude Code", default_config_path: crate::install::claude::default_config_path,
    hook_command: crate::install::claude::hook_command,
    merge_install: crate::install::claude::merge_install,
    merge_uninstall: crate::install::claude::merge_uninstall, needs_path_warning: true };

pub const CODEX: Target = Target { name: "codex", display_name: "Codex",
    restart_noun: "Codex", default_config_path: crate::install::codex::default_config_path,
    hook_command: crate::install::codex::hook_command,
    merge_install: crate::install::codex::merge_install,
    merge_uninstall: crate::install::codex::merge_uninstall, needs_path_warning: false };

pub const TARGETS: &[&Target] = &[&CLAUDE, &CODEX];

pub fn by_name(name: &str) -> Option<&'static Target> {
    TARGETS.iter().copied().find(|t| t.name == name)
}

/// Detection = the config FILE exists (not merely its parent dir): an empty
/// ~/.codex must NOT count as present.
pub fn config_present(path: &Path) -> bool {
    crate::install::io::resolve_symlink(path).exists()
}
pub fn is_present(t: &Target) -> bool { config_present((t.default_config_path)().as_path()) }
```

**Why a string-blob merge interface (`content: &str -> Result<String>`):** it makes `io.rs` truly format-neutral — read a `String`, hand to the target, write a `String`. No `serde_json::Value` / `toml::Value` leaks into the generic layer; each target owns its full parse/serialize cycle. Trade-off: Claude re-parses the JSON it just wrote — fine at human-speed install.

### Module layout

```
crates/pixtuoid/src/install/
├── mod.rs      run_install/run_uninstall orchestrator + plan_targets (pure) + dispatch (detect/confirm/--target)
├── io.rs       format-neutral fs primitives: read_config/write_config_atomic (String), backup/remove (suffix-param)
├── target.rs   Target struct + CLAUDE/CODEX consts + TARGETS + by_name + is_present/config_present
├── claude.rs   JSON merge fns (moved from merge.rs) + parse_or_empty + hook_command + default_config_path
└── codex.rs    TOML merge fns (#59 logic + sentinel + fixes) + hook_command + default_config_path
```

**Delete** `install/merge.rs` (JSON logic → `claude.rs`).

> **`const` vs `static`:** the `Target` consts + `const TARGETS: &[&Target] = &[&CLAUDE, &CODEX]` pattern compiles as written — `&CONST` in a `const` context is legal via rvalue static promotion (since Rust 1.21, well under MSRV 1.78), verified by compiling the exact pattern on editions 2021 and 2024. No `static` conversion needed.

### `claude.rs` notes (load-bearing — easy to drop on the move)

- **Port `LEGACY_SENTINEL_KEYS = &["_ascii_agents"]`** and the legacy-key branch of `is_managed_entry` **verbatim** from `merge.rs`, along with its two inline regression tests (`install_strips_legacy_ascii_agents_entries`, `uninstall_strips_legacy_ascii_agents_entries`). Dropping this silently regresses v0.3.x→v0.4.x upgraders, leaving orphan `_ascii_agents` hooks pointing at the long-gone `ascii-agents-hook` binary (the PR #40 regression).
- `merge.rs` uses two `.expect("just stored Value::Object/Array")` calls. The "no `unwrap`" rule means **no NEW `unwrap`/`expect`**; these pre-existing calls move verbatim (out of scope), or may optionally be converted to `?`/`match` propagation during the move.
- `io::hook_on_path()` stays Claude-only (hardcoded `"pixtuoid-hook"` name). It is **not** generalized: Codex writes an absolute path (`needs_path_warning: false`), so no PATH check applies to it. The PATH warning fires only when `t.needs_path_warning` is true (Claude).

### `io.rs` — format-neutral

```rust
// UNCHANGED (already generic): resolve_symlink, default_hook_binary, hook_on_path
pub fn read_config(path: &Path) -> Result<String>;          // raw content; "" for missing/empty
pub fn write_config_atomic(path: &Path, contents: &str) -> Result<()>; // lock + tmp + atomic rename + resolve_symlink
pub fn backup_once(path: &Path, suffix: &str) -> Result<Option<PathBuf>>;
pub fn remove_backup(path: &Path, suffix: &str) -> Result<Option<PathBuf>>;
```

All sibling paths use **string-append, never `with_extension`** — this is the structural fix for the backup-name bug:

```rust
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}", target.display(), suffix))
}
// backup: config.toml.pixtuoid.bak / settings.json.pixtuoid.bak   (with_extension gave config.json.pixtuoid.bak ✗)
// lock:   config.toml.lock                                        (with_extension gave config.lock ✗)
// tmp:    config.toml.tmp
```

The Claude backup name is **byte-identical** to today's (`settings.json.pixtuoid.bak`), so **no backup migration is needed** — do not add migration code.

`read_config` returns `""` for missing/empty; the empty-doc default lives in each target's `parse_or_empty` (`"" → json!({})` / `"" → empty toml Table`) so a fresh machine with no config still installs cleanly.

### `mod.rs` — one orchestrator + pure planner

```rust
pub struct InstallArgs   { pub hook_path: Option<PathBuf>, pub config: Option<PathBuf>,
                           pub target: Option<TargetName>, pub yes: bool }
pub struct UninstallArgs { pub config: Option<PathBuf>, pub target: Option<TargetName>, pub yes: bool }

// Single orchestrator — backup_once lives HERE, so no target can skip it.
fn run_install(t: &Target, config: Option<PathBuf>, hook_path: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let binary = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    let hook_cmd = (t.hook_command)(&binary)?;                  // Err on non-UTF-8 aborts loudly
    let content = io::read_config(&path)?;
    let merged = (t.merge_install)(&content, &hook_cmd)?;
    if merged == content { /* already up to date */ return Ok(()); }
    io::backup_once(&path, target::BACKUP_SUFFIX)?;             // always before write
    io::write_config_atomic(&path, &merged)?;
    /* report ok + backup path + restart_noun; PATH warning gated on t.needs_path_warning */
    Ok(())
}
fn run_uninstall(t: &Target, config: Option<PathBuf>) -> Result<()> {
    // read_config -> merge_uninstall; if merged == content, print "no pixtuoid hooks
    // found ... nothing to remove" and return Ok WITHOUT removing the backup (covers
    // both file-absent — read_config returns "" — and no-match). Else write_config_atomic
    // + remove_backup + report. The old separate "no settings.json ... nothing to do"
    // message is intentionally collapsed into this no-change branch.
}

// PURE — no filesystem, no stdin — fully unit-testable.
pub enum Plan { Targets(Vec<&'static Target>), NothingDetected, Conflict(String) }
pub fn plan_targets(requested: Option<TargetName>, explicit_config: bool,
                    present: &[(&'static Target, bool)], yes: bool, is_tty: bool) -> Plan;
```

**`TargetName` (clap enum) → `Target` resolution.** `Claude`/`Codex` resolve via `by_name` to a single `Target`. `All` is a meta-value (not a `Target`; `by_name("all")` is `None`) meaning "all present targets" — filter `present` to `bool == true`, and **warn** (not error) on a requested-but-absent target. The enum→`Target` resolution happens *inside* `plan_targets`, which keeps it pure (it receives the already-resolved `present` slice).

### `plan_targets` policy table

| `--target` | TTY? | Behavior |
|---|---|---|
| `claude` | any | Claude only |
| `codex` | any | Codex only |
| `all` | any | both present; **warn** (not error) for absent; **`--config` with `all` → `Conflict`** |
| *(none)* | TTY | detect present (by config-file existence); if ≥1, prompt `install hooks into <names>? [Y/n]` (default yes); `--yes` skips |
| *(none)* | non-TTY | exactly Claude present → install Claude (backward-compatible); Codex (or both) present → print detected + **exit non-zero** ("pass `--target …`"); never silently write Codex |
| *(none)* | any | none present → "no supported CLIs detected; pass `--target …`", exit 0 |

Detection is injected into `plan_targets` as `present: &[(&Target, bool)]`, so the entire policy is unit-testable without touching the real `$HOME` or stdin.

**Confirm-prompt contract** (TTY, no `--yes`): one `stdin().read_line()`, trim whitespace, ASCII-lowercase. Empty/Enter or `y`/`yes` → proceed. `n`/`no` → abort cleanly with a one-line message and exit 0. Any other input → treat as no (abort), no re-prompt. `--yes`/`-y` skips the read entirely. The answer parsing is a pure helper with its own unit test (separate from the `stdin` read).

### CLI surface (`cli.rs`)

```rust
InstallHooks {
    #[arg(long)] hook_path: Option<PathBuf>,
    #[arg(long, alias = "settings")] config: Option<PathBuf>,   // single field, hidden back-compat alias
    #[arg(long, value_enum)] target: Option<TargetName>,
    #[arg(long, short = 'y')] yes: bool,
},
UninstallHooks {
    #[arg(long, alias = "settings")] config: Option<PathBuf>,
    #[arg(long, value_enum)] target: Option<TargetName>,
    #[arg(long, short = 'y')] yes: bool,
},
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TargetName { Claude, Codex, All }
```

- `--config` is a single field with `--settings` as a clap **alias** (back-compat). Drops #59's separate `--codex-config`.
- `--config` + (`--target all` or auto-detect resolving to >1) → hard error via `Plan::Conflict` (clap's `conflicts_with` can't express "only when >1 target"); never silently drop a user-supplied path.
- Renames #59's `enum HookTarget` → `TargetName` (the name `HookTarget` is freed; no trait reuses it). Called out for the #59 author.

### Source attribution (decision 3)

The shim and socket stay single-instance. The install layer makes the source **explicit**:

- `codex::hook_command(resolved)` returns `PIXTUOID_SOURCE=codex <abs-path>/pixtuoid-hook` (absolute path; `Err` on non-UTF-8). Claude's returns bare `pixtuoid-hook` (no prefix; CC payloads already carry their source).
- The shim (`crates/pixtuoid-hook/src/main.rs`) reads `PIXTUOID_SOURCE` from its env and stamps `payload["source"]` before forwarding — a one-field addition, alongside the existing `_shim_ts_ms`. **The exit-0 / 200ms-timeout contract is untouched.**
- `decode_hook_payload` (`decoder.rs:28-31`) **already** reads explicit `payload["source"]` first and falls back to the hardcoded `claude_code::SOURCE_NAME` constant. **No `infer_hook_source` function exists on main** (it is a PR #59 worktree artifact). So phase-3 decoder changes are *additive only*: stamp `payload["source"]` in the shim from `PIXTUOID_SOURCE`, and add decoder arms for the new Codex event names. The existing explicit-`source`-then-fallback logic is unchanged — this is what removes the `transcript_path`-sniffing fragility (#59's worktree heuristic; Codex's `transcript_path` can be `null`).

**Shell-execution dependency.** The `PIXTUOID_SOURCE=codex …` env-prefix form only works if Codex runs the `command` string under a shell (`sh -c`). The verification rates this "almost certainly" true (the official docs example uses `$(...)` command substitution, which is shell-only), but it is *inferred, not contractual*. The phase-2 live checkpoint must confirm `payload["source"] == "codex"` actually arrives. **Robust fallback if Codex exec's directly:** make the shim accept `--source codex` as an argv flag and have `hook_command` emit `<abs-path>/pixtuoid-hook --source codex` instead — argv survives both shell and direct-exec. Default to the env-prefix per the approved decision; switch to the flag only if the checkpoint shows the env var isn't propagating.

## Codex specifics (`codex.rs`)

- `default_config_path` → `~/.codex/config.toml`.
- Events (decision 5): `SessionStart, PreToolUse, PostToolUse, UserPromptSubmit, SubagentStart, SubagentStop, Stop, PermissionRequest`. `matcher = "*"` on tool/subagent/permission events; `"startup|resume|clear|compact"` on `SessionStart`; omitted on `UserPromptSubmit`/`Stop`.
- Each handler: `{ type = "command", command = "<PIXTUOID_SOURCE=codex abs-path>", timeout = 5, statusMessage = "pixtuoid visualizer", _pixtuoid = true }`.
  - **No `[features] hooks = true`** (redundant no-op).
  - `timeout = 5` (raised from #59's aggressive `1`; the shim's own 200ms write timeout already bounds blocking).
  - `statusMessage = "pixtuoid visualizer"` (replaces #59's misleading "Updating pixtuoid", which surfaces in Codex's UI).
  - `_pixtuoid = true` sentinel lives on the **handler** (inside each `[[hooks.Event.hooks]]` entry), since Codex's structure is two-level: `[hooks.Event]` → array of *groups* → each group has a `matcher` + a `hooks` array of handlers. **Uninstall semantics:** `merge_uninstall` removes handlers carrying `_pixtuoid == true` (primary) or `file_name == "pixtuoid-hook"` (legacy #59 fallback) from each group; **if a group's `hooks` array becomes empty, remove the group; if an event's group array becomes empty, remove the event key.** A group mixing a managed handler and a user-authored handler keeps the user handler. `merge_install` replaces sentinel-marked handlers (idempotent regardless of resolved path).
- `PermissionRequest` decodes to the `Waiting` state in phase 3. Its **payload shape** (which of `session_id` / `transcript_path` / `tool_name` are present) is a **live unknown** — confirm at the phase-2 socket-tail checkpoint before writing the decoder. Note `decode_hook_payload` currently *hard-requires* `transcript_path` (`decoder.rs:24-27`, `.ok_or_else(bail)`); since Codex's `transcript_path` can be null, phase 3 must relax that to tolerate a missing/null `transcript_path` (fall back to `session_id` for the `AgentId`), or `PermissionRequest`/`Stop` will `bail` before the event match.

## How the four PR #59 review findings are structurally prevented

- **(a) No backup on Codex install.** `backup_once` lives in the single `run_install` orchestrator, called before every `write_config_atomic`. A target physically cannot reach the write without backing up.
- **(b) `with_extension` corrupts the Codex backup name.** All sibling paths (backup/lock/tmp) use `format!("{}.{}", display, suffix)` append. Unit tests pin multi-dot cases (`config.local.toml` → `config.local.toml.pixtuoid.bak`/`.lock`/`.tmp`).
- **(c) `to_string_lossy` → silent dead hook.** `hook_command` returns `Err` on non-UTF-8; `run_install` propagates and aborts loudly. Claude ignores the path (bare literal); Codex needs the path — modeled per-target.
- **(d) #59's worktree `infer_hook_source` misroutes Claude events** (classifies `Stop`/`UserPromptSubmit`/`Subagent*` as codex when `source` is absent). We do **not** port that heuristic. Main's `decode_hook_payload` already reads explicit `payload["source"]` first; the explicit `PIXTUOID_SOURCE` tag (above) makes Codex events carry their own source, so no inference is needed. Lands in phase 3 (decoder), not via any socket-topology change.

## Phasing & test gates

### Phase 1 — `Target`-struct refactor (pure, Claude-behavior-preserving)
Create `target.rs`, `claude.rs` (move JSON merge from `merge.rs`); rewrite `io.rs` (`read_config`/`write_config_atomic`/suffix-param backup + append fix on lock/tmp); rewrite `mod.rs` (`run_install`/`run_uninstall` + `plan_targets` + dispatch); update `cli.rs` (`TargetName`, `--config` + alias) and `main.rs`; delete `merge.rs`.

Also: update **CLAUDE.md** in the same commit — rename the `write_settings_atomic` invariant reference to `write_config_atomic`, and note install is now multi-target (Claude + Codex) via the `Target` registry (per CLAUDE.md's own "update docs in the same commit" rule).

**Gates (all green before phase 2):**
- Ported `merge.rs` JSON tests pass under `claude.rs`, **including the two `_ascii_agents` legacy-strip regression tests** (`install_strips_legacy_ascii_agents_entries`, `uninstall_strips_legacy_ascii_agents_entries`).
- `read_config` returns `""` for missing/empty; `merge_install("")` yields a valid populated config (empty-doc regression guard).
- `plan_targets` pure unit tests for **every** policy-table row (TTY/non-TTY × {claude,codex,all,none} × present-sets × {yes}); includes non-TTY+Codex-present+no-target → `Conflict`/exit-nonzero, and `--target all` + `--config` → `Conflict`. Confirm-prompt answer parsing has its own pure unit test.
- backup/lock/tmp naming asserted against literal multi-dot paths (e.g. `config.local.toml` → `…​.pixtuoid.bak`/`.lock`/`.tmp`).
- Existing `io.rs` symlink/atomic tests **rewritten** (not merely adapted) to call `write_config_atomic(&path, "…")` with a `&str` instead of `write_settings_atomic(&path, &Value)`; all pass.
- The existing `tests/install.rs` (which invokes the binary with `--settings`) must pass **UNCHANGED** — it is the back-compat oracle for `alias = "settings"`. Add a **second** test using `--config` to pin the new primary name; do not convert the existing test.
- Round-trip: `run_install` then `run_uninstall` on a temp Claude config restores byte-identical content; `CARGO_BIN_EXE` end-to-end CLI test green.
- `scripts/preflight.sh` green (clippy `-D warnings`, no **new** `unwrap`/`expect`, no new dep, no `unsafe`).

### Phase 2 — Codex target + detect/confirm
Create `codex.rs` (port #59's TOML merge logic, but **DELETE the `[features] hooks = true` block** — a verbatim port would reintroduce that forbidden redundant no-op; add the `_pixtuoid` handler sentinel + group-emptying uninstall, `hook_command` with `PIXTUOID_SOURCE` prefix + Err-on-non-UTF-8, `timeout = 5`, `statusMessage = "pixtuoid visualizer"`); add `CODEX` to `TARGETS`; wire detect/confirm/`--yes`/non-TTY into the binary via `IsTerminal`.

**Gates:**
- Codex `run_install`/`run_uninstall` round-trip on temp TOML: idempotent, **idempotent with a different resolved path** (sentinel, not basename), backup created (`config.toml.pixtuoid.bak`), surgical uninstall.
- Surgical boundary, pinned with a **mixed group** (one managed handler + one user-authored handler in the *same* `[hooks.Event]` group): uninstall removes only the managed handler, keeps the user handler, and does not delete the group. A group that becomes empty is removed; an event key that becomes empty is removed.
- `codex::hook_command` returns `Err` for a non-UTF-8 path.
- `plan_targets` integration: Codex-only → Codex; both + TTY + `--yes` → both; both + non-TTY + no-target → error.

**Codex-session verification checkpoint (manual gate before phase 3).** On a machine with a recent Codex (`codex --version` post-~Apr 2026):
1. `pixtuoid install-hooks --target codex`; confirm `~/.codex/config.toml.pixtuoid.bak` created and the merge is surgical (diff the backup); confirm `[[hooks.PreToolUse]]` groups carry the absolute `pixtuoid-hook` path + `PIXTUOID_SOURCE=codex`.
2. If Codex prompts to **trust** the hook (`trusted_hash` model), approve it.
3. Start a Codex session, trigger a **Bash/exec** tool (most reliable coverage), watch a sprite go Active. (It shows CC-attributed until phase 3 ships the decoder — expected.)
4. Tail the socket side to confirm JSON with `hook_event_name` + `tool_use_id` arrives on stdin.
5. `pixtuoid uninstall-hooks --target codex`; confirm config restored and backup removed.

This human gate must pass before any decoder work.

### Phase 3 — Codex Source + decoder (separate PR)
`decode_hook_payload` already reads explicit `payload["source"]` first and falls back to `claude_code::SOURCE_NAME` — **no `infer_hook_source` to demote** (it doesn't exist on main). Changes are *additive*: (1) shim stamps `payload["source"]` from `PIXTUOID_SOURCE`; (2) **relax the hard `transcript_path` requirement** (`decoder.rs:24-27`) to tolerate missing/null for Codex (fall back to `session_id` for the `AgentId`); (3) add decoder arms for the new Codex event names + a `CodexSource` JSONL decoder; (4) `cx·` label prefix (consistent with `cc·`/`ag·`); register in `SourceManager`; `PermissionRequest` → `Waiting`. **No second socket, no shim-socket parameterization.** Update the four refactor-sensitive test files if `AgentEvent` changes (per CLAUDE.md "When refactoring").

**Gates:** existing `e2e.rs`/`hook_socket.rs`/`jsonl_watcher.rs`/`reducer.rs` green; new Codex decode tests in `crates/pixtuoid-core/tests/decoder.rs` (the file #59 already adds in the worktree); a test proving a Codex payload with null/missing `transcript_path` still decodes; explicit-`source` test proving `Stop`/`UserPromptSubmit`/`Subagent*` route to the correct CLI.

## Risks / live unknowns

- **Codex hook trust model** (`trusted_hash`) may gate first-fire on user approval — surfaced in the phase-2 checkpoint.
- **Codex version skew** — pre-Apr-2026 installs may lack `PreToolUse`/`PostToolUse` (only `SessionStart`/`Stop`). Document the minimum version once confirmed live.
- **Shell PATH resolution of bare names** is inferred, not contractual; we sidestep it by writing an absolute path for Codex.
- **Codex rewriting config.toml** would drop our `_pixtuoid` sentinel (it deserializes into typed structs without the key). Codex does not generally rewrite user config; if observed, uninstall falls back to basename match. Acceptable.
- **Comment/format loss on `~/.codex/config.toml`.** Round-tripping through untyped `toml::Value` (read → parse → reserialize) canonicalizes the file and strips comments and field ordering on the first install. The `.pixtuoid.bak` backup is the recovery path. Emit a user-visible install line: `note: comments and formatting in config.toml are not preserved`.

## Crediting PR #59

`codex.rs`'s TOML merge/unmerge is derived from `trjh`'s work in PR #59. Commits that carry it use `Co-Authored-By: trjh`. PR #59 is closed with a comment explaining the absorption and pointing here.
