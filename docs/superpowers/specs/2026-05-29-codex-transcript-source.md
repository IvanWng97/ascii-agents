# Codex Transcript Source — Design

**Date:** 2026-05-29
**Status:** Approved (design); architecture reviewed by code-architect.
**Branch:** `feat/codex-transcript-source` (based on `feat/multi-cli-install-hooks` / PR #63, which carries the Phase-3 `cx·` hook rendering this builds on).

## Problem

Codex sessions render as `cx·` sprites via hooks (Phase 3), but a sprite that enters
`Waiting` on a permission prompt **stays stuck** after the user approves — it never
returns to "working", and only disappears when `STALE_WAITING_TIMEOUT` (60 min, or the
sweep) fires.

Root cause, two layers:

1. **Codex emits no hook when the user approves.** Verified against a live session
   transcript (`rollout-2026-05-29T22-36-52-019e7762-….jsonl`, the user's own permission
   test): the *resume* signal — the approved command actually running — appears only in
   the session **transcript** as `response_item:function_call_output` (and/or
   `event_msg:exec_command_end`), never as a hook event. Codex 0.135 hooks fire
   `UserPromptSubmit`, `PermissionRequest`, and `Stop` — none of which mean "resumed".
2. **`ActivityEnd` while `Waiting` is a no-op.** Both `PostToolUse` and `Stop` decode to
   `AgentEvent::ActivityEnd`, and the reducer only acts on `ActivityEnd` when the slot is
   `Active` (`reducer.rs:308`). So even the events that *do* arrive can't clear `Waiting`.

The reference implementation the user cited (`rullerzhou-afk/clawd-on-desk`) solves this
exactly this way: `agents/codex.js` declares `eventSource: "hook+log-poll"` and its
715-line `codex-log-monitor.js` watches the transcript for `exec_command_end` /
`function_call_output` to know work resumed (`codex-log-monitor.js:400-417`).

## Goal

Add a Codex **transcript (JSONL) source** that supplies the lifecycle signals hooks lack —
most importantly the post-approval *resume* — so the `cx·` sprite goes
`thinking → working → Waiting (permission) → working → idle` correctly. The fix mechanism
needs **no reducer change**: `ActivityStart` unconditionally sets `Active` (`reducer.rs:290`),
which clears `Waiting`. The transcript has that `ActivityStart` trigger
(`function_call_output`); we just have to feed it in.

## Architecture

Codex becomes a **hook + JSONL coalesced** source — the same shape as Claude Code, which
already runs a `HookSocketListener` and a `JsonlWatcher` whose events merge into one sprite.
The Codex hooks already arrive through the *shared* hook socket (Phase 3); this design adds
only the JSONL half.

**Architecture decision (code-architect review): keep the current `Source` /
`JsonlWatcher` seam — do NOT introduce a per-CLI descriptor/registry abstraction.**
Rationale: the install-side `Target` struct-of-fn-pointers registry works because install is
synchronous, run-once, and structurally identical across CLIs. The source layer is the
opposite — long-lived async tasks with genuinely different topologies (`ClaudeCodeSource`
spawns two abort-linked tasks; `AntigravitySource` one; Codex is JSONL-only-plus-shared-socket).
A `const` descriptor cannot model "spawn two tasks and abort-link them"; the `Source` trait's
`async fn run(self: Box<Self>)` already is the descriptor for that variance. The two apparent
"leaks" (hook decode centralized in `decoder.rs`; `source_label_prefix` in the reducer) are
intentional and trivial respectively: hook decoding is centralized *because the socket is
physically shared* and the shim stamps `source`, so distributing it would just relocate the
`match source`; and `source_label_prefix` is a 3-line match with a documented single-source-of-truth
role. Revisit only if CLI #4/#5 (Cursor/Copilot) make the pattern actually hurt — with a
dedicated refactor PR that can absorb the four-contract-test blast radius deliberately, not
riding shotgun on a bugfix.

### The coalescing contract (verified with live data)

Hook and JSONL events must produce the **same** `AgentId` or one Codex session renders as two
sprites. Codex hook payloads carry **no `transcript_path`**, so `decode_hook_payload` falls
back to keying on `session_id` (`decoder.rs:28-32`). The JSONL side must key on that same value.

Verified facts (live):
- The rollout filename **embeds the session UUID**: `rollout-<ts>-<UUID>.jsonl`.
- `session_meta.payload.id` == that trailing UUID (checked 5/5 recent files).
- A captured hook `session_id` (`019e774e-…`) has a matching rollout file
  `rollout-…-019e774e-….jsonl`.

Therefore: `hook.session_id == session_meta.id == trailing-UUID(rollout filename)`. The JSONL
source keys `AgentId` on the **trailing 36-char UUID** extracted from the filename, which equals
the hook key. `SessionStart` is idempotent (`reducer.rs:221`, first-wins), and the hook-wins
dedup is keyed on `tool_use_id` (`reducer.rs:128-138`) — Codex JSONL events use
`tool_use_id: None`, so they are never suppressed by hook events. The two transports merge
cleanly on the UUID.

### Components

| File | Change |
|------|--------|
| `crates/pixtuoid-core/src/source/codex.rs` *(new)* | `CodexSource` (JSONL-only `Source` impl, mirrors `AntigravitySource`); `decode_codex_line`, `derive_codex_label`, `codex_id_from_path` (trailing-UUID extractor), `codex_session_ended`; `pub const SOURCE_NAME: &str = "codex"`. |
| `crates/pixtuoid-core/src/source/jsonl.rs` | Add `pub type IdDeriver = fn(&Path) -> String;` + an `id_derive` field + `.with_id_deriver()` builder, **defaulting to the path-string** (so CC/AG `AgentId`s are byte-for-byte unchanged). Use `id_derive(path)` for the generic `SessionStart` `AgentId` (currently `transcript_path_str` at `jsonl.rs:333`). Extend `extract_cwd` to also read a nested `payload.cwd` (Codex `session_meta` nests it). |
| `crates/pixtuoid-core/src/source/mod.rs` | `pub mod codex;` |
| `crates/pixtuoid-core/src/state/reducer.rs` | `source_label_prefix` match arm `crate::source::codex::SOURCE_NAME => "cx"` (replaces the bare `"codex"` literal — consistency tidy only; behavior identical). |
| `crates/pixtuoid/src/runtime.rs` | `.with_source(Box::new(CodexSource::default_paths()))` (root `~/.codex/sessions`). |

`decode_hook_payload` and the rest of the reducer are **unchanged** — Codex hook handling and
the `cx·` prefix already exist and are tested (Phase 3).

### Data flow

```
Codex session
  ├─ hooks  → shim (PIXTUOID_SOURCE=codex) → shared HookSocketListener
  │            → decode_hook_payload (source=codex)
  │              · UserPromptSubmit → SessionStart  (creates cx· agent, instant)
  │              · PermissionRequest → Waiting       (instant)
  │              · Stop → ActivityEnd
  └─ transcript ~/.codex/sessions/**/rollout-<ts>-<UUID>.jsonl
               → CodexSource / JsonlWatcher (FSEvents-driven)
                 · session_meta            → (generic SessionStart, UUID-keyed) creates agent if hook hasn't
                 · task_started            → ActivityStart (working)
                 · function_call           → Waiting if require_escalated/justification, else ActivityStart
                 · function_call_output    → ActivityStart  ← RESUME (clears Waiting)
                 · exec_command_end        → ActivityStart  ← RESUME (alt form)
                 · task_complete / turn_aborted → ActivityEnd (→ idle)

Both transports → AgentId::from_parts("codex", <UUID>) → one sprite.
```

### Transcript event mapping

Records are `{"type":<outer>, "payload":{"type":<inner>, …}}`. Key on `<outer>:<inner>`.
All emitted events use `tool_use_id: None`.

| `<outer>:<inner>` | AgentEvent | Notes |
|---|---|---|
| `event_msg:task_started` | `ActivityStart{Typing}` | turn begins → working |
| `response_item:function_call` | `Waiting{"permission"}` **iff** `arguments` (a stringified JSON) contains `sandbox_permissions == "require_escalated"` or a `justification`; else `ActivityStart{Typing}` | tool starting; escalated = approval gate |
| `response_item:function_call_output` | `ActivityStart{Typing}` | **resume — the fix** |
| `event_msg:exec_command_end` | `ActivityStart{Typing}` | alt resume form |
| `event_msg:task_complete` | `ActivityEnd` | turn done → idle (debounced) |
| `event_msg:turn_aborted` | `ActivityEnd` | interrupted → idle |
| `session_meta` and all others | `[]` | creation handled by generic `SessionStart` |

`function_call.arguments` is a JSON **string**; parse it with `serde_json::from_str` and tolerate
failure (return the non-escalated `ActivityStart`). Never panic on malformed input — log + continue.

### `codex_id_from_path`

Extract the trailing UUID from the rollout filename stem. A canonical UUID is 36 chars
(`8-4-4-4-12`). Take the file stem; if its last 36 chars parse as a hyphenated-hex UUID, return
them; otherwise fall back to the whole stem (defensive — keeps a stable, if non-coalescing, key).
This is the single helper used by both the `IdDeriver` and `decode_codex_line` so the generic
`SessionStart` and the per-line events agree.

## Error handling

Per repo convention: the JSONL watcher and decoders **log + continue** on malformed input,
never panic. No `unwrap()` in non-test code. `decode_codex_line` returns `Ok(vec![])` for
unrecognized/irrelevant records. `~/.codex/sessions` is created if missing (existing
`JsonlWatcher` behavior, harmless for non-Codex users — matches Antigravity).

## Testing (TDD)

Unit (next to `codex.rs`):
- `codex_id_from_path` extracts the trailing UUID and **equals a hook `session_id`** (pin the
  coalescing contract).
- `decode_codex_line`: each row in the mapping table → expected event; `function_call` with
  `require_escalated` → `Waiting`, plain `function_call` → `ActivityStart`; malformed
  `arguments` → `ActivityStart` (no panic); unknown record → `[]`.
- `derive_codex_label` → `cx·<basename>`; empty cwd → `cx`.

Integration:
- `tests/jsonl_watcher.rs`: feed a `rollout-<ts>-<UUID>.jsonl`; assert the emitted
  `SessionStart.agent_id == AgentId::from_parts("codex", uuid)` (UUID-keyed, not path-keyed),
  and that a CC/AG file is **still path-keyed** (pin the `IdDeriver` default — guards against
  silently breaking CC hook↔JSONL coalescing).
- `tests/reducer.rs` — **the regression test for the bug**: apply hook `PermissionRequest`
  → assert `Waiting`; then apply JSONL `function_call_output` (same agent_id) → assert the slot
  is `Active` (resume). Then `task_complete` → eventually `Idle`.
- `extract_cwd` reads nested `payload.cwd`.

All four source-contract test files are reviewed for impact; only `reducer.rs` and
`jsonl_watcher.rs` need new cases (no signature changes to the channel/`Source`/`AgentEvent`).

## Non-goals (YAGNI)

- No new CLI flag — default `~/.codex/sessions` (add a `--codex-sessions-root` only if a real
  need appears, mirroring `--projects-root`).
- No per-CLI registry/descriptor abstraction (see architecture decision above).
- No reducer/`Transport`/`AgentEvent` changes.
- No change to the Phase-3 hook path.

## Risks

1. **UUID-coalescing mismatch** — retired by live verification (hook `session_id` ==
   rollout-filename UUID). Pinned by a unit test.
2. **`IdDeriver` default regression** — if the new fn-pointer doesn't default to path-string,
   every CC/AG `AgentId` silently changes and CC hook↔JSONL coalescing breaks. Pinned by a
   `jsonl_watcher.rs` test asserting the default.
3. **Replay-on-startup ghosts** — a recently-dead Codex session's rollout (mtime within the
   1 h initial window, no session-end marker) replays its events → a transient `cx·` agent that
   the stale-sweep reaps. Same behavior as CC; acceptable. `codex_session_ended` returns `false`
   (Codex writes no session-end marker; rely on mtime window + stale-sweep).
