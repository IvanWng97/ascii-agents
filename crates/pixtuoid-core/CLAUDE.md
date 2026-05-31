# pixtuoid-core — agent guide

The **headless library**: no terminal dependencies (`ratatui`/`crossterm`/`stdout`
are forbidden here — see workspace invariant #1). Owns the source/decoder seam,
the reducer/state machine, sprite parsing, terminal-agnostic layout geometry,
and the pure physics/pose math. The binary (`pixtuoid`) and renderer
(`pixtuoid/src/tui`) sit on top of this. See the workspace
[`CLAUDE.md`](../../CLAUDE.md) for cross-cutting rules.

## Layout

```
src/
├── source/             Source trait, hook+jsonl decoders, listeners, SourceManager
│                       mod.rs (Source trait, AgentEvent + agent_id(), REGISTERED_SOURCES, AgentId::from_parts),
│                       decoder.rs (shared utils: describe_tool_target, make_tool_detail, decode_hook_payload),
│                       hook.rs (HookSocketListener), jsonl.rs (JsonlWatcher + initial_seed_walk),
│                       claude_code.rs / codex.rs / antigravity.rs (per-source decode + label fns + Source impls),
│                       manager.rs (SourceManager::spawn / with_source)
├── state/              SceneState + Reducer (Transport-tagged dedup + Active→Idle debounce)
├── sprite/             .sprite parser, pack.toml loader, half-block blitter, animator, Pack::merge_from
├── render/             Renderer trait + TestRenderer (feature = "test-renderer")
├── layout/             zone-based office geometry (terminal-agnostic):
│                       mod.rs (SceneLayout struct, Bounds, Point, constants, accessors),
│                       compute.rs (compute_with_seed + 4 private helpers),
│                       decor.rs (WaypointKind [+MeetingSofa/MeetingStand], Facing, WallDecor, PlantKind, PodDecor),
│                       mask.rs (build_walkable_mask — obstacle stamping for A*),
│                       approach.rs (stand_point — which walkable cell off a waypoint an agent stands at, by side nearest its desk)
├── physics.rs          pure walk-pace physics (no terminal/router deps): WalkIntent, WalkProfile,
│                       walk_profile (trapezoidal/triangular kinematics), walk_progress (t_x1000),
│                       walk_arrived, speed_mult, pause_ms_for; constants: V_CRUISE_COMMUTE=0.36,
│                       V_CRUISE_WANDER=0.25, WALK_ACCEL=6.5e-4, SPEED_MULT_MIN/MAX, PAUSE_MS_MIN/MAX
├── pose.rs             pure state→pose derivation + wander state machine (no terminal deps).
│                       ENTRY_ANIMATION_MS is demoted: not a duration knob, only the spawn-window
│                       upper bound for tui entry-routing and door-cosmetic gating. Exports
│                       derive_state_only, aimless_wander_seed, pick_aimless_dest, and the
│                       ABSOLUTE per-spot dwell knobs (replacing the old PHASE_*_FRAC fractions):
│                       dwell_ms(kind,id) [sofa/meeting 20-40s, pantry 10-18s, vending 4-8s],
│                       seated_dwell_ms(id) [desk 15-30s], est_wander_cycle_ms(id) — for tui::motion
│                       (render authority) + idle_pose (stateless overlay, fixed WANDER_*_EST_MS).
├── walkable.rs         WalkableMask (static bool grid) + OccupancyOverlay (dynamic per-frame)
└── tests/              one integration test per concern
```

## Known sharp edges (don't be surprised by these)

- **CC hook payloads DO include `tool_use_id`** in `PreToolUse` and `PostToolUse` (verified by sniffing live payloads). The decoder reads it; the reducer's hook-wins dedup actually fires.
- **CC hook `transcript_path` always points to the PARENT'S transcript**, even when a subagent is the actor — so subagent hook events hash to the parent's `AgentId`. The reducer's `active_tasks: HashMap<AgentId, HashSet<String>>` suppresses hook `ActivityStart`/`End` for any agent currently inside a `Task` tool; JSONL has correct subagent attribution via the per-subagent transcript file at `<parent_uuid>/subagents/agent-<id>.jsonl`. The Task signal travels as `ToolDetail::Task` (a typed enum variant on `AgentEvent::ActivityStart.detail`, set by `decoder::make_tool_detail` whenever `tool_name == "Task"`); the reducer pattern-matches on `d.is_task()` rather than scanning a free-form string.
- **JSONL watcher skips historical transcripts on startup.** `initial_seed_root` in `source/jsonl.rs` only emits `SessionStart` for `.jsonl` files with mtime within `DEFAULT_INITIAL_WINDOW` (currently 1 hour; configurable via `JsonlWatcher::with_initial_window`); older files have their cursor seeded at end-of-file. Within the window, files that contain a `session_end`/`SessionEnd` marker (tail-scanned last 8 KB) are also seeded at EOF. Without this, ~hundreds of stale sessions saturate the desk allocator (capacity is auto-computed from terminal size; the headless fallback is `FALLBACK_DESKS=16` in `pixtuoid/runtime.rs`). Long-idle live sessions only re-appear after they next write. A 250ms post-startup rescan catches files missed by `read_dir` during the initial seed walk (APFS metadata propagation race).
- **Agent removal needs a `SessionEnd`; abrupt exits have none and fall back to the slow stale-sweep.** A slot is GC'd `EXIT_GRACE_WINDOW` (4.5 s) after `exiting_at` is set, which only an `AgentEvent::SessionEnd` or `sweep_stale` sets. CC emits `SessionEnd` two ways: the **hook** (best-effort — 200 ms write timeout, no retry, races CC's own teardown, so it drops silently), and — for a **clean `/exit`·`/quit`** — a **durable JSONL marker** decoded by `decode_cc_line` + `cc_session_ended` (CC writes no `session_end` line, only the `<command-name>` user event, so we match that). That JSONL path is the fallback when the hook drops. But **Ctrl-C / terminal-close / kill produce NEITHER** (the process dies before the hook runs and writes no marker), and **Codex has no `SessionEnd` concept at all** (its hooks are `UserPromptSubmit`/`Stop`/`PermissionRequest`; `Stop` = turn end → Idle, not session end). Those exits are reaped only by `sweep_stale` (Active 10 min / Idle 30 min / Waiting 60 min). **This is inherent, not a bug** — transcript silence can't distinguish a dead session from a live-but-idle one without process liveness, which CC doesn't expose. Do NOT add a short silence-based reaper: it would evict legitimately-idle live sessions.
- **Subagent display names come from `attributionAgent` in JSONL.** The decoder strips the plugin prefix (`feature-dev:code-explorer` → `code-explorer`) and emits `AgentEvent::Rename` so labels read meaningfully. Parents get their `cwd` basename instead.
- **`AgentSlot.state_started_at` is `std::time::SystemTime`** — process-local in practice (no wall-clock anchoring), but the type is already serializable, so the v2 daemon split won't need a type swap. The pose system computes elapsed time relative to it for animation timing.
- **`ActivityState::Active` ≠ "tool is currently executing".** CC fires PreToolUse → PostToolUse around every individual tool call, so without debouncing the slot flickers Active/Idle on every tool. The reducer treats `ActivityEnd` as "arm pending idle" (`AgentSlot.pending_idle_at`) instead of an immediate flip; the actual transition to `Idle` is realized by `reducer::tick` after `ACTIVE_GRACE_WINDOW = 1500 ms`. Any `ActivityStart` inside the window cancels the pending idle. Net: the slot reads as continuously Active for chained tool work, and visible Idle lags real Idle by up to 1.5 s + the 1 s sweep interval (≈ 2.5 s worst case). Don't add code that depends on `Active → Idle` being instant.
- **The reducer's permission `Waiting` resolves on the gated tool's PostToolUse.** A `PermissionRequest`/decision sets `Waiting`; the matching tool's `ActivityEnd` (PostToolUse) clears it (`gated_before_waiting` tracking). It is also cleared by `SessionEnd` + retained-eviction; note `gated_before_waiting` is evicted in `tick`'s retain but historically not in `sweep_exited` — keep them in sync if you touch eviction.

## Where to look

- "How is the office laid out?" → `core::layout::SceneLayout::compute_with_seed` is the coordinator; it calls 4 private helpers: `compute_pod_desks` (pod grid), `compute_pod_decor` (aisle decor), `compute_room_walls` (wall segments + door gaps), `compute_waypoints` (all waypoint assembly). Re-exported as `tui::layout::Layout`. `core::pose::derive` for pure state→pose mapping including the Idle wander state machine (`WANDER_CYCLE_BASE_MS=7000` + per-agent jitter).
- "Why is the subagent's sprite the right one and not the parent?" → `reducer::Reducer::apply` does subagent-leak suppression via `active_tasks` before applying. `claude_code::decode_cc_line` emits `AgentEvent::Rename` from `attributionAgent`.
- "How does multi-source decoding work?" → `JsonlWatcher` takes `LineDecoder` and `LabelDeriver` fn pointers (Strategy pattern via fn pointers, not traits). CC wires `claude_code::decode_cc_line` + `cc_derive_label`; Codex wires `codex::decode_codex_line` + its label fn; Antigravity wires `antigravity::decode_ag_line` + `derive_ag_label`. `decoder.rs` holds shared utilities (`describe_tool_target`, `make_tool_detail`, `decode_hook_payload`). Each source owns its own JSONL format knowledge. `AgentId::from_parts(source, path)` namespaces IDs per source. Labels show source prefix: `cc·project` / `cx·project` / `ag·project`. **CLI attribution comes ONLY from the shim-owned `_pixtuoid_source` key, NEVER the public `source` field** (CC overloads `source` for the SessionStart *reason* — startup/resume/clear — and reading it splits the agent into an un-reapable ghost; see the WHY at `source/decoder.rs` `decode_hook_payload`). Adding a CLI = add it to `source::REGISTERED_SOURCES` (which forces a coalescing fixture under `tests/fixtures/sources/<name>/` + a `source_label_prefix` arm, both enforced by tests) **and** wire it into `pixtuoid/runtime.rs::run_async` (the registry gates the conformance tests, NOT runtime spawning — those are separate lists). Ship a fixture exercising the **SessionStart hook** so `tests/fixture_harness.rs`'s one-AgentId assertion guards against the ghost.
- "Why don't old idle sessions show on startup?" → `source::jsonl::initial_seed_walk`. Checks `check_session_ended` (tail-scans last 8 KB for `session_end`/`SessionEnd` markers) and seeds the cursor at EOF for any file with mtime > `DEFAULT_INITIAL_WINDOW` (1 hour) — no `SessionStart`. The 1-hour mtime window is the **only** freshness gate.
- "How does walk-pace physics work?" → `pixtuoid_core::physics` (pure, no terminal deps) — `WalkIntent` tags the walk kind (Entry/Exit/WanderOut/WanderBack/SnapBack); `walk_profile(path_len_octile, intent, agent_id)` returns a frozen `WalkProfile` with trapezoidal/triangular kinematics (triangular when path < `v²/a`, trapezoidal otherwise); `walk_progress(p, elapsed_ms)` emits `t_x1000 ∈ [0,1000]`; `walk_arrived` gates the Walking→Seated/AtWaypoint flip including the per-agent arrival pause. Constants: `V_CRUISE_COMMUTE = 0.36` octile/ms (brisk, Entry/Exit/SnapBack), `V_CRUISE_WANDER = 0.25` octile/ms (amble, wander legs), `WALK_ACCEL = 6.5e-4` octile/ms² (~0.55 s ramp; `L_crit ≈ 199` octile commute, `≈ 96` octile wander). Per-agent personality: `speed_mult` (bits 24..34 of splitmix64 hash, range 0.85–1.20) and `pause_ms_for` (bits 40..52, range 200–400 ms) — disjoint from each other and from `cycle_ms_for`/`personality_for`. Near desks arrive and sit before far desks (natural staggered arrivals). The *stateful* timeline that consumes these profiles lives in `pixtuoid/src/tui/motion` (the render authority) — see that nested guide.

## When refactoring

The channel type, `Source` trait, `AgentEvent` enum, and reducer signature are workspace-wide contracts — see the root [`CLAUDE.md`](../../CLAUDE.md) "When refactoring" for the full list of test files to update and the add-a-CLI checklist.
