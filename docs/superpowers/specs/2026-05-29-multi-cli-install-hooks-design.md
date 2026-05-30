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
5. Auto-detect installed CLIs with an interactive confirm; never silently mutate a high-value config (`~/.codex/config.toml`) in a non-interactive context.
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
fn run_uninstall(t: &Target, config: Option<PathBuf>) -> Result<()> { /* symmetric; remove_backup only on change */ }

// PURE — no filesystem, no stdin — fully unit-testable.
pub enum Plan { Targets(Vec<&'static Target>), NothingDetected, Conflict(String) }
pub fn plan_targets(requested: Option<TargetName>, explicit_config: bool,
                    present: &[(&'static Target, bool)], yes: bool, is_tty: bool) -> Plan;
```

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
- `decode_hook_payload` already reads an explicit `source`; `infer_hook_source` becomes a last-resort fallback only. This removes the `transcript_path`-sniffing fragility (Codex's `transcript_path` can be `null`).

**Shell-execution dependency.** The `PIXTUOID_SOURCE=codex …` env-prefix form only works if Codex runs the `command` string under a shell (`sh -c`). The verification rates this "almost certainly" true (the official docs example uses `$(...)` command substitution, which is shell-only), but it is *inferred, not contractual*. The phase-2 live checkpoint must confirm `payload["source"] == "codex"` actually arrives. **Robust fallback if Codex exec's directly:** make the shim accept `--source codex` as an argv flag and have `hook_command` emit `<abs-path>/pixtuoid-hook --source codex` instead — argv survives both shell and direct-exec. Default to the env-prefix per the approved decision; switch to the flag only if the checkpoint shows the env var isn't propagating.

## Codex specifics (`codex.rs`)

- `default_config_path` → `~/.codex/config.toml`.
- Events (decision 5): `SessionStart, PreToolUse, PostToolUse, UserPromptSubmit, SubagentStart, SubagentStop, Stop, PermissionRequest`. `matcher = "*"` on tool/subagent/permission events; `"startup|resume|clear|compact"` on `SessionStart`; omitted on `UserPromptSubmit`/`Stop`.
- Each handler: `{ type = "command", command = "<PIXTUOID_SOURCE=codex abs-path>", timeout = 5, statusMessage = "pixtuoid visualizer", _pixtuoid = true }`.
  - **No `[features] hooks = true`** (redundant no-op).
  - `timeout = 5` (raised from #59's aggressive `1`; the shim's own 200ms write timeout already bounds blocking).
  - `statusMessage = "pixtuoid visualizer"` (replaces #59's misleading "Updating pixtuoid", which surfaces in Codex's UI).
  - `_pixtuoid = true` sentinel → `merge_uninstall` removes **only** sentinel-marked entries (surgical), and `merge_install` replaces sentinel-marked entries (idempotent regardless of resolved path). Basename match kept as a legacy fallback for entries written by an un-upgraded #59.
- `PermissionRequest` decodes to the `Waiting` state in phase 3.

## How the four PR #59 review findings are structurally prevented

- **(a) No backup on Codex install.** `backup_once` lives in the single `run_install` orchestrator, called before every `write_config_atomic`. A target physically cannot reach the write without backing up.
- **(b) `with_extension` corrupts the Codex backup name.** All sibling paths (backup/lock/tmp) use `format!("{}.{}", display, suffix)` append. Unit tests pin multi-dot cases (`config.local.toml` → `config.local.toml.pixtuoid.bak`/`.lock`/`.tmp`).
- **(c) `to_string_lossy` → silent dead hook.** `hook_command` returns `Err` on non-UTF-8; `run_install` propagates and aborts loudly. Claude ignores the path (bare literal); Codex needs the path — modeled per-target.
- **(d) `infer_hook_source` misroutes Claude events.** Fixed by the explicit `PIXTUOID_SOURCE` tag (above); inference demoted to fallback. Lands in phase 3 (decoder), not via any socket-topology change.

## Phasing & test gates

### Phase 1 — `Target`-struct refactor (pure, Claude-behavior-preserving)
Create `target.rs`, `claude.rs` (move JSON merge from `merge.rs`); rewrite `io.rs` (`read_config`/`write_config_atomic`/suffix-param backup + append fix on lock/tmp); rewrite `mod.rs` (`run_install`/`run_uninstall` + `plan_targets` + dispatch); update `cli.rs` (`TargetName`, `--config` + alias) and `main.rs`; delete `merge.rs`.

**Gates (all green before phase 2):**
- Ported `merge.rs` JSON tests pass under `claude.rs`.
- `read_config` returns `""` for missing/empty; `merge_install("")` yields a valid populated config (empty-doc regression guard).
- `plan_targets` pure unit tests for **every** policy-table row (TTY/non-TTY × {claude,codex,all,none} × present-sets × {yes}); includes non-TTY+Codex-present+no-target → `Conflict`/exit-nonzero, and `--target all` + `--config` → `Conflict`.
- backup/lock/tmp naming asserted against literal multi-dot paths.
- Existing `io.rs` symlink/atomic tests adapted + pass.
- Round-trip: `run_install` then `run_uninstall` on a temp Claude config restores byte-identical content; `CARGO_BIN_EXE` end-to-end CLI test green.
- `scripts/preflight.sh` green (clippy `-D warnings`, no `unwrap`, no new dep, no `unsafe`).

### Phase 2 — Codex target + detect/confirm
Create `codex.rs` (port #59 TOML merge, add `_pixtuoid` sentinel, `hook_command` with `PIXTUOID_SOURCE` prefix + Err-on-non-UTF-8, drop `[features]`, `timeout=5`, fixed `statusMessage`); add `CODEX` to `TARGETS`; wire detect/confirm/`--yes`/non-TTY into the binary via `IsTerminal`.

**Gates:**
- Codex `run_install`/`run_uninstall` round-trip on temp TOML: idempotent, **idempotent with a different resolved path** (sentinel, not basename), backup created (`config.toml.pixtuoid.bak`), surgical uninstall.
- User-authored Codex hook with a different command survives uninstall; a sentinel-marked `pixtuoid-hook` entry is removed (pins the surgical boundary).
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
`decoder.rs` reads explicit `source` (already does) → `infer_hook_source` becomes fallback only; shim stamps `payload["source"]` from `PIXTUOID_SOURCE`; new `CodexSource` JSONL decoder + `cx·` label prefix; register in `SourceManager`; `PermissionRequest` → `Waiting`. **No second socket, no shim-socket parameterization.** Update the four refactor-sensitive test files if `AgentEvent` changes (per CLAUDE.md "When refactoring").

**Gates:** existing `e2e.rs`/`hook_socket.rs`/`jsonl_watcher.rs`/`reducer.rs` green; new `codex_decoder` test; explicit-`source` test proving `Stop`/`UserPromptSubmit`/`Subagent*` route to the correct CLI.

## Risks / live unknowns

- **Codex hook trust model** (`trusted_hash`) may gate first-fire on user approval — surfaced in the phase-2 checkpoint.
- **Codex version skew** — pre-Apr-2026 installs may lack `PreToolUse`/`PostToolUse` (only `SessionStart`/`Stop`). Document the minimum version once confirmed live.
- **Shell PATH resolution of bare names** is inferred, not contractual; we sidestep it by writing an absolute path for Codex.
- **Codex rewriting config.toml** would drop our `_pixtuoid` sentinel (it deserializes into typed structs without the key). Codex does not generally rewrite user config; if observed, uninstall falls back to basename match. Acceptable.

## Crediting PR #59

`codex.rs`'s TOML merge/unmerge is derived from `trjh`'s work in PR #59. Commits that carry it use `Co-Authored-By: trjh`. PR #59 is closed with a comment explaining the absorption and pointing here.
