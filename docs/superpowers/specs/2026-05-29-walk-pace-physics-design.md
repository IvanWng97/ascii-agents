# Walk-Pace Physics Design

**Date:** 2026-05-29
**Status:** Draft
**Author:** Ivan + Claude

## Overview

Today every character walk is **fixed-duration**: a newly-spawned agent walks from
the elevator/door to its desk over a constant `ENTRY_ANIMATION_MS = 4000ms` regardless
of how far that desk is; the exit walk reuses the same constant; and idle-wander legs
take a fixed *fraction* of the per-agent cycle. The visible symptom: when several agents
arrive together, **they all sit down at the same instant** even though their desks are at
very different distances from the door — which reads as robotic and unreal.

This change replaces fixed-duration walking with a **real-world physics model**: characters
walk at a **constant cruising velocity** (with acceleration/deceleration at the ends), so
arrival time = `distance ÷ speed`. Near desks are reached (and sat at) earlier; far desks
later. Walks naturally **stagger**. On top of the constant-speed base we add four real-world
texture traits (per-person pace, accelerate/decelerate, intent-based speed, brief arrival settle).

## Goals

1. **Distance-driven duration** for all three walk types — entry (door→desk), exit
   (desk→door), and idle-wander (desk→waypoint→desk). Far desks take proportionally longer.
2. **Constant cruising velocity** along the *actual route walked* (A\* path length), not the
   straight line — a person walks the route around obstacles, at a steady pace.
3. **Real kinematics, not a tween:** accelerate from a standstill, cruise, decelerate to a stop
   (trapezoidal velocity profile; degrades to triangular for short walks). Natural start/stop is
   a *consequence of the physics*, not a bolted-on ease curve.
4. **Per-person pace** — each agent has a deterministic speed multiplier so equidistant walkers
   still don't move in lockstep.
5. **Intent-based speed** — entry/exit brisk (commuting), idle-wander slower (ambling).
6. **Brief arrival settle** — a short standing beat before the Walking→Seated/AtWaypoint flip.
7. **Preserve invariant #1** — `pixtuoid-core` stays terminal- and router-free. Pure physics
   math lives in core; anything that needs the A\* path length lives in the tui layer.
8. **No regressions** — entry/exit/wander still play; snap-back smoothing still works; the
   snapshot example and all existing tests stay green (updated where they assert old timing).
   `core::pose::derive()` and its 25+ tests are **untouched**.

## Non-Goals

- Variable speed *within* a single cruise (corner slowdown, crowd avoidance). Cruise is constant;
  corners are handled by the existing per-segment A\* remap.
- Path re-planning mid-walk for *timing*. The route may re-route per frame for *shape*
  (occupancy), but the **duration is fixed at walk-start** (commit-to-route).
- Collision/jostling physics between agents (occupancy overlay already nudges routes).
- Changing *which* waypoint an agent picks or *how often* it wanders (personality unchanged).

## Measured Geometry (calibration ground-truth)

Octile distance is the metric the router already uses: `14·min(dx,dy) + 10·(max−min)`,
i.e. **10 units per orthogonal pixel**. Measured `door_threshold → desk` octile distances
for the real computed layout (throwaway probe, since reverted):

| Terminal (buf px) | desks | nearest | farthest | **far/near ratio** |
|---|---|---|---|---|
| 192×160 | 8  | 916 (~91px)  | 1436 (~143px) | 1.57× |
| 192×160 | 16 | 206 (~20px)  | 1436 (~143px) | **6.97×** |
| 240×200 | 8  | 624 (~62px)  | 1576 (~157px) | 2.53× |
| 240×200 | 16 | 624 (~62px)  | 1784 (~178px) | 2.86× |
| 320×240 | 16 | 572 (~57px)  | 2372 (~237px) | 4.15× |

**Implication:** in a busy office the farthest desk is **4–7× the walk distance** of the
nearest. Under constant speed that is a large, clearly-visible stagger — confirming
distance-driven timing is the correct lever and that the effect reads strongly at half-block
scale. These are also the real `L` values the constants must be calibrated against (see below).

## Real-Physics Model

A walk is a frozen **`WalkProfile`** parameterized by:

- `L` — total path length in octile units, **snapshotted once at walk-start** (stable; immune
  to per-frame occupancy re-routing).
- `v` — cruise speed (octile/ms) = `v_base(intent) × speed_mult(agent_id)`.
- `a` — acceleration = deceleration (octile/ms²), a shared constant.

**Switchover length** `L_crit = v²/a` (cruise reachable iff `L ≥ L_crit`); accel distance
one side `d_a = v²/(2a) = L_crit/2`, accel time `t_a = v/a`.

**Triangular** (`L < L_crit`, never reaches cruise):
```
T = 2·sqrt(L/a)                          peak at T/2
s(t) = ½·a·t²                            0 ≤ t ≤ T/2
s(t) = L − ½·a·(T − t)²                  T/2 < t ≤ T
```

**Trapezoidal** (`L ≥ L_crit`):
```
t_c = (L − L_crit)/v                     cruise time
T   = 2·t_a + t_c = v/a + (L − L_crit)/v
s(t) = ½·a·t²                            accel:  0 ≤ t ≤ t_a
s(t) = d_a + v·(t − t_a)                 cruise: t_a < t ≤ t_a+t_c   ← genuine constant-velocity plateau
s(t) = L − ½·a·(T − t)²                  decel:  t_a+t_c < t ≤ T
```

Render progress `p = s(t)/L ∈ [0,1]`, emitted as `t_x1000 = round(1000·p)` and fed into the
**existing** per-segment A\* remap in `derive_with_routing` (which distributes a global `t`
across octile-weighted legs). **SHAPE is live (re-routed per frame); DURATION is frozen** — that
decoupling is the heart of the design.

**Arrival settle:** for `elapsed ∈ [T, T+pause_ms)` the agent holds `t_x1000 = 1000` (stands at
the destination, still in the walk sprite). `walk_arrived := elapsed ≥ T + pause_ms` gates the
Walking→Seated/AtWaypoint flip. `pause_ms` is per-agent so simultaneous arrivals desynchronize.

### Trait → parameter mapping

| Trait | Mechanism |
|---|---|
| Distance-driven duration | `T` is a function of snapshotted `L` |
| Constant cruise velocity | trapezoidal plateau at `v` |
| Accelerate / decelerate | accel/decel ramps of the profile (free from the kinematics) |
| Per-person pace | `speed_mult(agent_id)` ∈ [0.85, 1.20], hash bits 24..34 (disjoint from `cycle_ms_for`/`personality_for`) |
| Intent-based speed | `v_base`: Entry/Exit/SnapBack = commute; WanderOut/Back = amble |
| Arrival settle | `pause_ms_for(agent_id)` ∈ [200, 400], hash bits 40..52 (independent of speed) |

### Constant calibration

Constants are calibrated against the **measured door→desk octile distances** in the real office
geometry (see table above), with the goal of keeping the *effective average* walk pace ≈ the old
flat 4 s baseline while making duration distance-proportional.

```
V_CRUISE_COMMUTE = 0.36  octile/ms   — Entry / Exit / SnapBack
V_CRUISE_WANDER  = 0.25  octile/ms   — WanderOut / WanderBack
WALK_ACCEL       = 6.5e-4 octile/ms² (~0.55 s accel ramp: t_a = v/a)
SPEED_MULT_MIN   = 0.85 ;  SPEED_MULT_MAX = 1.20
PAUSE_MS_MIN     = 200  ;  PAUSE_MS_MAX   = 400
```

**Geometry-based rationale.** Measured entry distances on a 192×160 / 8-desk floor are
916–1436 octile.  At `v=0.36` (commute, speed_mult=1.0):

| desk | L (octile) | T (ms) |
|---|---|---|
| near  | 916  | ≈ 3 100 ms |
| avg   | ~1 100 | ≈ 3 800 ms |
| far   | 1 436 | ≈ 4 500 ms |
| tiny  | 206  | ≈ 1 100 ms (busy floor, 16 desks) |

Staggered-arrival spread: **1.4–3.4 s** on a busy floor — clearly visible at half-block scale.
`L_crit = v²/a ≈ 199` octile (commute), `≈ 96` octile (wander); all real entry paths are in the
trapezoidal regime.  `WALK_ACCEL = 6.5e-4` gives `t_a ≈ 0.55 s`, a natural ease-in/ease-out ramp.

The initial draft proposed `v=0.213` (human-gait calibrated, giving 5–7 s walks), but
the measured geometry showed those walks would read as sluggish against the old 4 s baseline.
The decided values keep the **effective average walk time ≈ 4 s** while delivering the full
stagger effect.  The model is identical either way; `v`/`a` are pure feel knobs.

## Architecture

**Backbone:** pure core physics module + the tui layer as the sole **motion-timing authority**.
The tui snapshots the A\* path length once per walk-start, freezes a `WalkProfile`, and drives
`t_x1000` per-frame off the frozen duration while re-routing path *shape* per frame.

### New pure core module — `crates/pixtuoid-core/src/physics.rs`

Imports only `crate::AgentId`. No `SystemTime`, no `Router`, no `layout`, no `Point`. Holds the
`WalkIntent` enum, `WalkProfile` struct, all physics constants, and pure fns
`walk_profile` / `walk_progress` / `walk_arrived` / `speed_mult` / `pause_ms_for`.
Fully unit-tested in-file. (`pub mod physics;` in `lib.rs`.)

```rust
pub enum WalkIntent { Entry, Exit, WanderOut, WanderBack, SnapBack }

pub struct WalkProfile {
    pub duration_ms: u64,      // accel→cruise→decel, EXCLUDING pause
    pub pause_ms: u64,         // per-agent arrival settle
    pub path_len_octile: u32,  // snapshotted length
    pub v_cruise: f32,         // effective cruise after speed_mult
    pub accel: f32,
}

pub fn speed_mult(agent_id: AgentId) -> f32;        // [0.85,1.20], bits 24..34
pub fn pause_ms_for(agent_id: AgentId) -> u64;      // [200,400], bits 40..52
pub fn walk_profile(len_octile: u32, intent: WalkIntent, id: AgentId) -> WalkProfile;
pub fn walk_progress(p: &WalkProfile, elapsed_ms: u64) -> u16;  // t_x1000, saturates at 1000
pub fn walk_arrived(p: &WalkProfile, elapsed_ms: u64) -> bool;  // elapsed ≥ duration + pause
```
`f32` (matches the pixel pipeline): screen ≤ ~4096 px → ≤ ~57k octile; f32's 24-bit mantissa
keeps all `a·t²` products exact. Tests use `eps = 2` on `t_x1000`.

### New tui module — `crates/pixtuoid/src/tui/motion.rs`

```rust
pub enum WanderPhase { Seated, WalkingOut, AtWaypoint, WalkingBack }

pub struct MotionState {
    pub agent_id: AgentId,
    pub entry: Option<(SystemTime, WalkProfile)>,           // (walk_started_at, profile)
    pub exit:  Option<(SystemTime, WalkProfile)>,
    pub snap_back: Option<(SystemTime, WalkProfile, Point)>,
    // cyclic wander:
    pub wander_cycle_n: u64,
    pub wander_phase: WanderPhase,
    pub wander_phase_started_at: SystemTime,
    pub wander_profile: Option<WalkProfile>,               // current out/back leg, snapshotted at transition
    pub wander_dest: Point,
    pub wander_dest_kind: Option<WaypointKind>,
    pub wander_dest_wp_idx: Option<usize>,
}
```
Also `octile_path_len(&[Point])`, reusing the existing `octile_distance` (promoted to
`pub(in crate::tui)`). `MotionState` ≈ 120 B; 16 agents × 5 floors ≈ 10 KB.

**Ownership:** `FloorCtx` (in `tui/floor.rs`) gains `pub motion: HashMap<AgentId, MotionState>`
and `pub door_anim_max_ms: u64` (per-floor cache of the longest in-flight entry/exit physics
duration — replaces the hardcoded `ENTRY_ANIMATION_MS` window in door cosmetics). One map per
floor (an agent lives on exactly one floor). **Eviction:** add
`fctx.motion.retain(|id,_| scene.agents.contains_key(id))` to the existing coffee retain block
in `tui_renderer.rs`.

### `derive_with_routing` becomes the motion authority

New signature gains `motion: &mut HashMap<AgentId, MotionState>` (threaded exactly like
`history`). Dispatch order:

1. desk guard (unchanged).
2. **EXIT** (`exiting_at` set): on first sighting, route desk→door, snapshot `octile_path_len`,
   store `exit = (exiting_at, walk_profile(len, Exit, id))`. Each frame: `t_x1000 =
   walk_progress`, update floor `door_anim_max_ms`; `walk_arrived` → `None` (GC, as today).
3. **ENTRY** (no `motion.entry` yet AND `now-created_at < ENTRY_ANIMATION_MS` — kept only as the
   spawn-window gate bounding the route call): route door→desk, snapshot, store `entry`. While
   present and `!walk_arrived` → physics-driven Walking. When `walk_arrived` → fall through to
   state pose (near desks finish early — the stagger).
4. Otherwise call `core::derive` for the raw state pose. Non-wander poses → existing snap-back
   override (now a `SnapBack`-intent `WalkProfile`, capped at `SNAP_BACK_MS`) + existing polyline
   mapper.
5. Idle agents in the wander cycle → `advance_wander()` (below) owns `t_x1000` via physics, then
   the **same** existing polyline segment-mapper, `history.record`, and jitter-correction run
   verbatim. Physics only replaces the *source* of the global `t_x1000`.

### Stateful elastic wander timeline (`advance_wander`)

The hard part: core's `idle_pose` used `cycle_n = elapsed/cycle_ms` and fixed phase fractions;
physics makes walk legs variable-length, so phase time can't be a fixed fraction. Solution — an
explicit per-phase clock in `MotionState`:

- **Seated** and **AtWaypoint dwell** stay fixed-fraction of `cycle_ms` (unchanged knobs).
- **WalkingOut / WalkingBack** are physics-driven (snapshot the leg's A\* length at the phase
  transition; `walk_profile(len, WanderOut/Back, id)`).
- The cycle becomes **elastic** (total length varies) — harmless because each phase is anchored
  to its *own* `wander_phase_started_at`, not a global modulo. No clamp needed (this supersedes
  both candidate designs' cycle-overrun concerns).
- `wander_cycle_n` increments deterministically on each completed `WalkingBack`, so destination
  selection (`takes_trip` / `is_aimless_cycle` / `waypoint_index_for_cycle` / `pick_aimless_dest`)
  stays **identical** to today.
- **INIT / bootstrap:** fresh Idle (detected by `wander_phase_started_at < slot.state_started_at`)
  seeds at Seated anchored to `state_started_at`. An agent Idle a long time before first render
  fast-forwards `cycle_n` (jump approximation — only seated/dwell phases are skipped, zero visual
  impact) to resync destinations with what `core::derive` would have computed.

Per-frame, by phase: Seated→(at seated_dur, on a trip cycle) snapshot walk-out, →WalkingOut;
WalkingOut→(walk_arrived) snapshot walk-back, →AtWaypoint; AtWaypoint→(at dwell_dur) →WalkingBack;
WalkingBack→(walk_arrived) `cycle_n += 1`, →Seated.

### Blast radius (threading the `motion` borrow)

`lib.rs` (+`pub mod physics`), new `physics.rs`, new `motion.rs`, `tui/mod.rs`
(+`pub mod motion`); `tui/pose.rs` (authority + param + in-file test call sites);
`tui/floor.rs` (FloorCtx fields); `tui/renderer.rs` (`DrawCtx.motion`); `pixel_painter/mod.rs`
(`PixelCtx.motion` + door tests); `pixel_painter/anchors.rs` (`character_anchor` +
`compute_door_frame_idx` reads `door_anim_max_ms` not `ENTRY_ANIMATION_MS`); `tui_renderer.rs`
(construct + retain + multi-floor/transition branches); `hit_test.rs` + `widgets/tooltip.rs`
(`character_anchor` call sites); `examples/snapshot.rs` (+ regenerate visual baseline).
`core::pose.rs` gets **no code change** — only a doc comment demoting `ENTRY_ANIMATION_MS` to the
non-routing/door-cosmetic fallback.

## Implementation Plan (phased, TDD)

0. **Pure core `physics.rs`** — write the full core test list first (red), then implement. Lands
   independently, reviewable on its own. `cargo test -p pixtuoid-core`.
1. **Tui scaffolding (no behavior change)** — `motion.rs` (struct/enum/`octile_path_len`),
   `FloorCtx` fields, promote `octile_distance`. Compiles, nothing wired.
2. **Thread the param (behavior-preserving)** — add `motion` to `DrawCtx`/`PixelCtx` and all call
   sites; `derive_with_routing` gains the param but **ignores** it (today's behavior). Mechanical;
   `cargo test --workspace` stays green.
3. **Entry/exit physics (TDD)** — implement EXIT then ENTRY snapshot+profile branches; door
   `door_anim_max_ms` write + `compute_door_frame_idx` read; entry/exit tests red→green.
4. **Snap-back through physics** — convert override to `SnapBack` profile capped at `SNAP_BACK_MS`;
   keep the 8px/900ms gates; snap-back tests stay green.
5. **Cyclic wander timeline (the hard part, TDD)** — `advance_wander()` + per-phase clock +
   bootstrap + `cycle_n`; reuse core destination fns; full wander test list red→green.
6. **Integration + visual** — workspace tests; rebuild snapshot example; render `--cols 192
   --rows 80`; crop + read to confirm staggered arrivals + accel/decel read visually; regenerate
   baseline; `scripts/preflight.sh`.
7. **Docs** — CLAUDE.md "Where to look" (physics module, motion authority, elastic wander),
   `MotionState` ownership on `FloorCtx`, `ENTRY_ANIMATION_MS` demotion.

## Test Strategy

**Core (pure, no router/layout):** triangular & trapezoidal `duration_ms` formulas; `p(0)=0`,
`p(T)=1`, `p(T/2)≈500`; **cruise plateau** (equal Δ`t_x1000` across the cruise band — proves
constant velocity); progress saturation + monotonicity; `walk_arrived` false during pause / true
after; zero-length no-panic; `speed_mult` range+determinism; `pause_ms` range + independence from
speed; intent ordering (commute faster than wander).

**Tui motion (StubRouter):** entry duration scales with path length; nearer desk arrives earlier;
5 same-`created_at` agents → 5 distinct durations (stagger); exit snapshotted once; exit uses
commute speed; each wander phase transition (seated→out→atwaypoint→back→seated, `cycle_n++`);
dwell time independent of path length; far waypoint full-cycle wall-time longer (walk legs differ,
seated/dwell identical); **shape-changes-duration-stable** (re-route mid-walk → duration unchanged,
segment changes); arrival pause holds the *walk* pose (not a desk pose) during `[T, T+pause)`;
per-agent speed applied.

**Regression:** all four `snap_back_*` tests pass (profile-driven); every existing `core::pose.rs`
test passes unchanged; snapshot example renders without panic (visual baseline changes by design).

## Success Criteria

1. Spawning N agents together, the farthest-desk agent is still walking after the nearest-desk
   agent has sat — staggered arrival, no synchronized sit.
2. Walks visibly ease in and out (no instant start/stop).
3. Equidistant agents differ slightly in pace (per-person multiplier).
4. Entry/exit read brisker than idle-wander ambling.
5. `pixtuoid-core` has zero router/terminal dependencies (invariant #1 holds).
6. `cargo test --workspace --features pixtuoid-core/test-renderer` green; preflight clean.

## Open Questions (resolve during implementation)

1. **Walk anchor continuity:** wander walk-out currently starts from bare `desk`; entry/snap-back
   use `desk+(6,4)` so the walking anchor matches the seated anchor. Recommend the wander
   *return* leg end at `desk+(6,4)` to avoid a seat-snap. Confirm against `anchors::seated_anchor`
   vs `walking_anchor`.
2. **Bootstrap catch-up:** for an agent Idle a long time before first render, prefer the
   `cycle_n` jump approximation over iterating (only seated/dwell skipped → nil visual impact).
3. **AtWaypoint overlay reservation** (`pixel_painter/mod.rs` builds the occupancy overlay from
   `core::derive`, not routing): now that wander phase is stateful in the tui, the overlay pass
   may disagree for one frame on who is AtWaypoint. Likely benign (overlay is advisory for A\*);
   consider building it from the motion map for exactness.
4. ~~**`v`/`a` final values:** physically-exact vs snappier.~~ **RESOLVED:** `V_CRUISE_COMMUTE=0.36`,
   `V_CRUISE_WANDER=0.25`, `WALK_ACCEL=6.5e-4`. Calibrated to measured office geometry;
   effective average walk ≈ 4 s with a 1.4–3.4 s stagger. (See Constant calibration.)
5. **Exit arrival pause:** exit ends in GC (`None`), no pose flip. Recommend no pause on exit.
6. **Visual baseline:** the snapshot baseline changes deterministically; confirm its
   location/process so Phase 6 regenerates the right artifact and the PR documents the diff.

## Implementation Status (shipped in PR #61)

Built across 8 TDD phases. Where things landed (read this first if iterating later):

- **Core physics** — `pixtuoid-core/src/physics.rs` (`WalkProfile`, `walk_profile`/`walk_progress`/
  `walk_arrived`, `speed_mult`/`pause_ms_for` with a splitmix64 finalizer). Pure; invariant #1 holds.
- **Motion authority** — `tui/pose.rs::derive_with_routing` snapshots the A\* length into a frozen
  `WalkProfile` and drives `t_x1000`; per-agent `MotionState` lives on `FloorCtx`.
- **Elastic wander** — `tui/motion.rs::advance_wander` (per-phase clock, idempotent via
  `last_advanced_at`, deterministic `cycle_n`; destination selection unchanged via shared
  `aimless_wander_seed`). OQ #1 resolved: walk-out is symmetric at `desk+(6,4)` (no stand-up jump).
- **Window-bounded walks (post-review fixes):** snap-back and exit walks are **time-compressed** so
  they complete within their GC windows — snap-back by `SNAP_BACK_MS` (was teleporting), exit before
  the reducer's `EXIT_GRACE_WINDOW` (was vanishing mid-corridor). Both have regression tests.
- **Door cosmetic** — `recompute_door_anim_max_ms` only counts *in-flight* walks (`MotionState.entry`
  is never cleared, so an arrived profile would otherwise hold the door open for the agent's life).

### Deferred follow-ups (low severity, intentionally not done)

- **Transition-floor motion eviction:** `render_transition_floor` doesn't evict `fctx.motion`; the
  leak is bounded and self-heals when the floor is next viewed on the normal path. One-line fix.
- **OQ #3 — overlay AtWaypoint divergence:** the occupancy overlay reserves cells from stateless
  `core::derive`, which now drifts from physics-timed `advance_wander` by *seconds* (confirmed in
  review, not one frame). Advisory for A\* routing only (no state/correctness impact); a fix means
  routing the overlay pass through `advance_wander` on the render hot path — deferred as not worth
  the risk for an aesthetic issue.

### Tuning knob

`V_CRUISE_COMMUTE`/`V_CRUISE_WANDER`/`WALK_ACCEL` in `physics.rs` are the only feel dials — calibrated
to the measured office geometry (see Constant calibration). Tests assert *relative* behavior, so
re-tuning these never breaks the suite; judge from a live `pixtuoid run`.
