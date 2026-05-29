# Walk-Pace Physics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Task numbers are scoped per-phase** (Phase 3 / Task 2 etc.).

**Goal:** Replace fixed-duration character walking with a real-physics constant-velocity model so near desks are reached (and sat at) before far desks — natural staggered arrival instead of synchronized sitting.

**Architecture:** Pure kinematics in `pixtuoid-core/src/physics.rs` (trapezoidal/triangular velocity profile, no router/terminal deps — invariant #1). The tui `derive_with_routing` becomes the motion-timing authority: it snapshots the A* path length once at walk-start, freezes a `WalkProfile`, and drives `t_x1000` per frame off the frozen duration while the A* router still drives path *shape* per frame. Per-agent `MotionState` is cached on `FloorCtx`. The cyclic wander timeline becomes a stateful elastic per-phase clock that keeps `cycle_n` deterministic so destination selection is unchanged.

**Tech Stack:** Rust (Cargo workspace), ratatui/crossterm (tui only), TDD via `cargo test`.

**Spec:** `docs/superpowers/specs/2026-05-29-walk-pace-physics-design.md`

**Phase order (strict dependency chain):** 0 (core physics) → 1 (tui scaffolding) → 2 (thread param) → 3 (entry/exit) → 4 (snap-back) → 5 (wander) → 6 (integration/visual) → 7 (docs). Each phase must compile + be green before the next.

---


## ⚠️ Canonical Corrections (authoritative — override any conflicting inline text below)

The 8 phases were drafted independently; a 3-critic review found 13 blocker + 16 major defects. The fixes below are the **single source of truth**. Where a later phase's inline code/instructions disagree with a decision here, **this section wins** — and when implementing, update *every* occurrence so types stay consistent.

**A. `MotionState::new` — one signature.** `pub fn new(agent_id: AgentId) -> Self`: all `Option` fields `None`, `wander_cycle_n: 0`, `wander_phase: WanderPhase::Seated`, `wander_phase_started_at: SystemTime::UNIX_EPOCH` (epoch sentinel that `advance_wander` fresh-Idle detection needs), `wander_dest: Point { x: 0, y: 0 }`, kinds `None`. **Add field** `last_advanced_at: SystemTime` (init `UNIX_EPOCH`) for idempotency (see F). Defined once in Phase 1 (its test asserts `== UNIX_EPOCH`). Delete the redefinitions in Phase 3 & Phase 5.

**B. One polyline helper.** `route_walking_pose(slot, now, layout, router, overlay, history, pose) -> Option<Pose>`, defined in Phase 3, records `history` with **`now`** (never `slot.last_event_at` — that silently breaks snap-back). Phase 5 reuses it. There is no `apply_polyline_routing`.

**C. No double-walk.** Core `derive()` stays UNTOUCHED (TestRenderer/non-routing + overlay pass still use it; its 25+ tests stay green). Phase 0 ADDS a pure `pub fn derive_state_only(slot, now, layout) -> Option<Pose>` = `derive` minus the exit-override and entry-override blocks (the `match slot.state {…}` tail only). `derive_with_routing` owns entry/exit/wander timing via `motion`; for the non-walk state pose it calls `derive_state_only` (so a physics-arrived entry does not restart a linear walk); for Idle it calls `advance_wander` (tui-owned wander) instead of core's `idle_pose` wander output. Preserve `SeatedThinking` (decide explicitly whether `advance_wander` or `derive_state_only` returns it; keep it working). The Phase 3 test `nearer_desk_arrives_before_farther_desk` must pass under this.

**D. `door_anim_max_ms` live on the MAIN path** (which goes through `draw_scene`, not `render_to_rgb_buffer`). Add `pub door_anim_max_ms: u64` to `DrawCtx`; in `tui_renderer.rs::render`, after `draw_scene` returns, re-borrow the current `FloorCtx` and recompute `fctx.door_anim_max_ms` from `fctx.motion.values()` (max in-flight entry/exit `duration_ms`) for the NEXT frame, and set it in the `DrawCtx` literal; in `draw_scene` pass `ctx.door_anim_max_ms` into `PixelCtx` (replace the hardcoded `0`); add the field to both `DrawCtx` literals in `examples/snapshot.rs` and to `render_transition_floor`'s `PixelCtx`.

**E. `pick_aimless_dest` is `pub`** in `pixtuoid-core/src/pose.rs` (currently private) so Phase 5 calls `pick_aimless_dest(layout, seed)`. `tui::layout::Layout` is a type alias for `SceneLayout` — there is **no `.inner`/`.0`/`.scene_layout`**; pass `layout` directly. Fix every `&layout.inner`.

**F. `advance_wander` idempotency.** It runs 2+ times/frame per agent (seated-overlay pass + character loop + `character_anchor`). Perform transitions (re-anchor `wander_phase_started_at`, `wander_cycle_n += 1`, snapshot new leg) only when `now > last_advanced_at`, then set `last_advanced_at = now`; when `now <= last_advanced_at`, compute the pose from existing phase state WITHOUT mutating. Phase 5 test: calling it twice with the same `now` leaves `wander_cycle_n`/`wander_phase` unchanged.

**G. Physics `×1000.0` bug.** `WALK_ACCEL` is octile/ms², so `2*sqrt(L/a)` and `v/a` are ALREADY ms. Remove the spurious `* 1000.0` in `walk_profile` AND in the Phase 0 test formulas (`triangular_duration_formula`, `trapezoidal_duration_formula`, `t_a_ms` in `cruise_plateau_has_constant_delta`) in BOTH the Task 1 stub copy and the Task 3 impl copy. (The `(1000.0 * s / l)` in `walk_progress` is the `t_x1000` scaling — **correct, keep it**.) After the fix L=1200/agent ≈ 6722 ms, not 6,722,050.

**H. `speed_mult` / `pause_ms_for` distribution.** FNV-1a doesn't avalanche short similar inputs (only ~2 distinct values → `speed_mult_varies_across_agents` fails). Add a splitmix64 finalizer before slicing, in BOTH fns: `let h = agent_id.raw(); let z = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); let z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); let z = z ^ (z >> 31);` then slice `(z >> 24) & 0x3FF` (speed_mult) and a disjoint window (pause_ms_for).

**I. De-duplicate exact-string edits.** Promote `octile_distance` to `pub(in crate::tui)` ONCE (Phase 1); add the `motion` param to `derive_with_routing` ONCE (Phase 2). Later phases reference these as "already done" — do not re-issue the Edit (old_string is already changed).

**J. Phase 3 `PixelCtx`/`DrawCtx` blocks are ADDITIVE** — keep the `motion` field Phase 2 added; only INSERT `door_anim_max_ms`. No full-struct replacements that drop `motion`.

**K. Single eviction.** Keep only Phase 2's `self.floor_ctxs[self.current_floor].motion.retain(|id, _| scene.agents.contains_key(id));` placed BEFORE the `let fctx = &mut …` binding. Delete Phase 3's `fctx.motion.retain(...)` (borrow-conflicts + duplicate).

**L. Clippy/borrow fixes.** Delete the dead `let ms = slot.agent_id;` in the EXIT branch. Destructure the non-Copy exit profile without moving: `let e = mstate.exit.as_ref()?; let started_at = e.0; let profile = &e.1;`. Delete the bogus `use pixtuoid_core::physics::WALKING_FRAME_MS;` (it lives in `pixtuoid_core::pose`, already in scope via the existing `pub use`).

**M. Test robustness.** `nearer_desk_arrives_*`/`entry_duration_scales_*`: don't assume `home_desks[0]` is nearest — compute `octile_distance(door_threshold, desk+offset)` per desk, pick argmin/argmax, assert max ≥ 1.5× min before relying on ordering. Phase 5 bootstrap `cycle_n` test: assert an exact value derived from `cycle_ms_for` (or document the analytic ± bound) — no guessed tolerance. Phase 3 red steps: precondition "Phase 2 merged; `cargo build -p pixtuoid` passes" so the red step is a genuine assertion FAIL, not a compile error.

---

## Phase 0: Pure core physics.rs (TDD)

### Task 1: Write the full failing test suite for physics.rs

**Files:**
- Create `/Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/crates/pixtuoid-core/src/physics.rs`
- Modify `/Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/crates/pixtuoid-core/src/lib.rs`

- [ ] Add `pub mod physics;` to `lib.rs` after the existing module declarations (insert after `pub mod walkable;`):

```rust
// crates/pixtuoid-core/src/lib.rs  — add this line after `pub mod walkable;`
pub mod physics;
```

- [ ] Create `crates/pixtuoid-core/src/physics.rs` with the complete test module (stubs only — no impl yet, just enough to compile the test file's imports):

```rust
//! Pure physics model for character walking.
//!
//! Imports only `crate::AgentId`. No router, no layout, no terminal deps.
//! All kinematics are f32; screen is ≤ ~4096 px → ≤ ~57k octile, well
//! within f32's 24-bit mantissa.

use crate::AgentId;

// ── Intent ────────────────────────────────────────────────────────────────────

/// Why is this walk happening? Determines which cruise speed is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkIntent {
    /// Agent spawned, walking door → desk. Brisk commute speed.
    Entry,
    /// Session ended, walking desk → door. Brisk commute speed.
    Exit,
    /// Idle wander: desk → waypoint leg. Ambling speed.
    WanderOut,
    /// Idle wander: waypoint → desk leg. Ambling speed.
    WanderBack,
    /// Routing correction snap-back. Brisk commute speed.
    SnapBack,
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Cruise speed for Entry / Exit / SnapBack walks (octile/ms ≈ 1.6 m/s).
pub const V_CRUISE_COMMUTE: f32 = 0.213;
/// Cruise speed for WanderOut / WanderBack walks (octile/ms ≈ 1.1 m/s).
pub const V_CRUISE_WANDER: f32 = 0.146;
/// Shared acceleration/deceleration constant (octile/ms²). Gives ~0.5 s ramp.
pub const WALK_ACCEL: f32 = 3.7e-4;

/// Minimum per-agent speed multiplier.
pub const SPEED_MULT_MIN: f32 = 0.85;
/// Maximum per-agent speed multiplier.
pub const SPEED_MULT_MAX: f32 = 1.20;

/// Minimum arrival settle pause (ms).
pub const PAUSE_MS_MIN: u64 = 200;
/// Maximum arrival settle pause (ms).
pub const PAUSE_MS_MAX: u64 = 400;

// ── Profile ───────────────────────────────────────────────────────────────────

/// Frozen kinematic profile for one walk leg, computed once at walk-start.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkProfile {
    /// Accel → cruise → decel total time, **excluding** arrival pause.
    pub duration_ms: u64,
    /// Per-agent arrival settle before the pose flips to seated/at-waypoint.
    pub pause_ms: u64,
    /// Snapshotted A* path length (octile units).
    pub path_len_octile: u32,
    /// Effective cruise speed after `speed_mult` applied.
    pub v_cruise: f32,
    /// Acceleration constant (same as `WALK_ACCEL`; stored for walk_progress).
    pub accel: f32,
}

// ── Public API stubs (will be implemented in Task 3) ─────────────────────────

/// Deterministic per-agent speed multiplier in [SPEED_MULT_MIN, SPEED_MULT_MAX].
/// Uses hash bits 24..34 (disjoint from cycle_ms_for bits 16..28 upper range
/// only by small overlap, but personality_for uses bits 0..14, so these bit
/// windows serve distinct purposes by construction).
pub fn speed_mult(_agent_id: AgentId) -> f32 {
    todo!()
}

/// Deterministic per-agent arrival pause in [PAUSE_MS_MIN, PAUSE_MS_MAX].
/// Uses hash bits 40..52 (disjoint from speed_mult bits 24..34).
pub fn pause_ms_for(_agent_id: AgentId) -> u64 {
    todo!()
}

/// Compute the frozen kinematic profile for a walk of `path_len_octile` units.
pub fn walk_profile(path_len_octile: u32, intent: WalkIntent, agent_id: AgentId) -> WalkProfile {
    let _ = (path_len_octile, intent, agent_id);
    todo!()
}

/// Render progress as `t_x1000 = round(1000 * s(elapsed_ms) / L)`.
/// Saturates at 1000 once `elapsed_ms >= p.duration_ms`.
/// During the pause window `[duration_ms, duration_ms + pause_ms)` returns 1000.
pub fn walk_progress(p: &WalkProfile, elapsed_ms: u64) -> u16 {
    let _ = (p, elapsed_ms);
    todo!()
}

/// Returns `true` when the full walk + pause has elapsed.
pub fn walk_arrived(p: &WalkProfile, elapsed_ms: u64) -> bool {
    let _ = (p, elapsed_ms);
    todo!()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ids ──────────────────────────────────────────────────────────

    fn id(n: u8) -> AgentId {
        AgentId::from_parts("test", &format!("agent-{n}"))
    }

    // ── Constant sanity ─────────────────────────────────────────────────────

    #[test]
    fn commute_faster_than_wander() {
        assert!(
            V_CRUISE_COMMUTE > V_CRUISE_WANDER,
            "commute speed ({V_CRUISE_COMMUTE}) must exceed wander speed ({V_CRUISE_WANDER})"
        );
    }

    // ── speed_mult ──────────────────────────────────────────────────────────

    #[test]
    fn speed_mult_in_range() {
        for n in 0..=50u8 {
            let m = speed_mult(id(n));
            assert!(
                m >= SPEED_MULT_MIN && m <= SPEED_MULT_MAX,
                "agent {n}: speed_mult {m} out of [{SPEED_MULT_MIN}, {SPEED_MULT_MAX}]"
            );
        }
    }

    #[test]
    fn speed_mult_is_deterministic() {
        let a = id(7);
        assert_eq!(
            speed_mult(a),
            speed_mult(a),
            "speed_mult must be deterministic for the same AgentId"
        );
    }

    #[test]
    fn speed_mult_varies_across_agents() {
        let values: Vec<f32> = (0..20u8).map(|n| speed_mult(id(n))).collect();
        let distinct: std::collections::HashSet<u32> =
            values.iter().map(|v| v.to_bits()).collect();
        assert!(
            distinct.len() >= 5,
            "expected variance in speed_mult across agents, got {distinct:?}"
        );
    }

    // ── pause_ms_for ────────────────────────────────────────────────────────

    #[test]
    fn pause_ms_in_range() {
        for n in 0..=50u8 {
            let p = pause_ms_for(id(n));
            assert!(
                p >= PAUSE_MS_MIN && p <= PAUSE_MS_MAX,
                "agent {n}: pause_ms {p} out of [{PAUSE_MS_MIN}, {PAUSE_MS_MAX}]"
            );
        }
    }

    #[test]
    fn pause_ms_independent_of_speed_mult() {
        // Verify at least some agents have different pause_ms while sharing
        // the same broad speed bucket — i.e. the two values are not identical
        // linear functions of each other.
        let pairs: Vec<(f32, u64)> = (0..50u8).map(|n| (speed_mult(id(n)), pause_ms_for(id(n)))).collect();
        // Correlation: count agents whose speed_mult is in the lower half of
        // the range but whose pause_ms is in the upper half, and vice versa.
        let speed_mid = (SPEED_MULT_MIN + SPEED_MULT_MAX) / 2.0;
        let pause_mid = (PAUSE_MS_MIN + PAUSE_MS_MAX) / 2;
        let cross_a = pairs.iter().filter(|(s, p)| *s < speed_mid && *p > pause_mid).count();
        let cross_b = pairs.iter().filter(|(s, p)| *s >= speed_mid && *p <= pause_mid).count();
        assert!(
            cross_a + cross_b >= 4,
            "pause_ms should be independent of speed_mult; cross-quadrant count too low: {cross_a}+{cross_b}"
        );
    }

    // ── walk_profile: triangular regime ─────────────────────────────────────

    /// L_crit = v²/a. A path shorter than L_crit never reaches cruise.
    fn l_crit(v: f32) -> f32 {
        v * v / WALK_ACCEL
    }

    #[test]
    fn triangular_duration_formula() {
        // For L < L_crit: T = 2·sqrt(L/a). Use v_commute with speed_mult=1.0
        // by choosing an agent whose speed_mult is exactly 1.0 … which we
        // can't guarantee. Instead use a SHORT path and verify the formula
        // relationship rather than an absolute value.
        //
        // Strategy: pick L = L_crit/4 (well into triangular regime for any
        // agent speed in [0.85,1.20]·V_CRUISE_COMMUTE).
        // T_expected = 2·sqrt(L/a); allow ±5ms for rounding.
        let v_min = V_CRUISE_COMMUTE * SPEED_MULT_MIN;
        let l_crit_min = l_crit(v_min);
        let l = (l_crit_min / 4.0) as u32; // guaranteed triangular for all agents

        for n in 0..10u8 {
            let profile = walk_profile(l, WalkIntent::Entry, id(n));
            let v = profile.v_cruise;
            let l_crit_v = l_crit(v);
            assert!(
                (l as f32) < l_crit_v,
                "agent {n}: L={l} should be < L_crit={l_crit_v}"
            );
            // T = 2·sqrt(L / a)
            let t_expected_ms = (2.0 * ((l as f32) / WALK_ACCEL).sqrt()) as u64;
            let diff = profile.duration_ms.abs_diff(t_expected_ms);
            assert!(
                diff <= 5,
                "agent {n}: triangular T={} expected≈{t_expected_ms} (diff={diff}ms)",
                profile.duration_ms
            );
        }
    }

    #[test]
    fn trapezoidal_duration_formula() {
        // For L >= L_crit: T = v/a + (L - L_crit)/v.
        // Use L = 1200 (≫ L_crit for all agents under commute speed).
        let l: u32 = 1200;

        for n in 0..10u8 {
            let profile = walk_profile(l, WalkIntent::Entry, id(n));
            let v = profile.v_cruise;
            let l_f = l as f32;
            let lc = l_crit(v);
            assert!(
                l_f >= lc,
                "agent {n}: L={l_f} should be >= L_crit={lc} for trapezoidal"
            );
            // T = t_a + t_c + t_a = v/a + (L-L_crit)/v
            let t_a = v / WALK_ACCEL;
            let t_c = (l_f - lc) / v;
            let t_expected_ms = (2.0 * t_a + t_c) as u64;
            let diff = profile.duration_ms.abs_diff(t_expected_ms);
            assert!(
                diff <= 5,
                "agent {n}: trapezoidal T={} expected≈{t_expected_ms} (diff={diff}ms)",
                profile.duration_ms
            );
        }
    }

    // ── walk_progress: boundary values ──────────────────────────────────────

    const EPS: u16 = 2; // tolerance on t_x1000

    #[test]
    fn progress_at_zero_is_zero() {
        let profile = walk_profile(1000, WalkIntent::Entry, id(0));
        let p = walk_progress(&profile, 0);
        assert!(p <= EPS, "p(0) should be ≈0, got {p}");
    }

    #[test]
    fn progress_at_duration_is_1000() {
        let profile = walk_profile(1000, WalkIntent::Entry, id(0));
        let p = walk_progress(&profile, profile.duration_ms);
        assert!(
            p >= 1000 - EPS,
            "p(T) should be ≈1000, got {p}"
        );
    }

    #[test]
    fn progress_at_half_duration_triangular_is_near_500() {
        // In the triangular regime, s(T/2) = L/2 exactly (symmetry), so p=500.
        let v_min = V_CRUISE_COMMUTE * SPEED_MULT_MIN;
        let l_crit_min = l_crit(v_min);
        let l = (l_crit_min / 4.0) as u32;
        let profile = walk_profile(l, WalkIntent::Entry, id(0));
        let half = profile.duration_ms / 2;
        let p = walk_progress(&profile, half);
        assert!(
            (500u16).abs_diff(p) <= EPS + 10,
            "triangular p(T/2) should be ≈500, got {p}"
        );
    }

    #[test]
    fn progress_at_half_duration_trapezoidal() {
        // In the trapezoidal regime, T/2 falls somewhere in the cruise band
        // (for long paths). p should be > 400 and < 600 (symmetry; doesn't
        // need to be exactly 500 because accel != decel fractions differ).
        let l: u32 = 1200;
        let profile = walk_profile(l, WalkIntent::Entry, id(0));
        let half = profile.duration_ms / 2;
        let p = walk_progress(&profile, half);
        assert!(
            (400..=600).contains(&p),
            "trapezoidal p(T/2) should be in 400..=600, got {p}"
        );
    }

    // ── walk_progress: cruise plateau proves constant velocity ───────────────

    #[test]
    fn cruise_plateau_has_constant_delta() {
        // During cruise, Δs per Δt is constant → equal Δ(t_x1000) for equal Δt.
        // Use L=1200 (trapezoidal, clear cruise band).
        let l: u32 = 1200;
        let profile = walk_profile(l, WalkIntent::Entry, id(3));
        let v = profile.v_cruise;
        let lc = l_crit(v);
        // t_a = time to reach cruise (ms)
        let t_a_ms = (v / WALK_ACCEL) as u64;
        // sample 5 points in the cruise band
        let cruise_start = t_a_ms + 50;
        let cruise_end = profile.duration_ms - t_a_ms - 50; // symmetric decel
        assert!(
            cruise_start < cruise_end,
            "need a cruise band: t_a={t_a_ms}ms, T={}ms, L={l}, Lc={lc}",
            profile.duration_ms
        );
        let step = (cruise_end - cruise_start) / 5;
        let samples: Vec<u16> = (0..=5)
            .map(|i| walk_progress(&profile, cruise_start + i * step))
            .collect();
        let deltas: Vec<i32> = samples.windows(2).map(|w| w[1] as i32 - w[0] as i32).collect();
        let first = deltas[0];
        for (i, d) in deltas.iter().enumerate() {
            assert!(
                (d - first).abs() <= EPS as i32,
                "cruise Δ[{i}]={d} differs from Δ[0]={first} by more than {EPS} — not constant velocity"
            );
        }
    }

    // ── walk_progress: saturation and monotonicity ───────────────────────────

    #[test]
    fn progress_saturates_at_1000() {
        let profile = walk_profile(500, WalkIntent::Entry, id(1));
        // Well past duration
        let p = walk_progress(&profile, profile.duration_ms * 3);
        assert_eq!(p, 1000, "progress must saturate at 1000");
    }

    #[test]
    fn progress_is_monotone() {
        let profile = walk_profile(800, WalkIntent::WanderOut, id(2));
        let samples: Vec<u16> = (0..=20)
            .map(|i| walk_progress(&profile, i * profile.duration_ms / 20))
            .collect();
        for w in samples.windows(2) {
            assert!(
                w[1] >= w[0],
                "progress must be non-decreasing, got {} then {}",
                w[0],
                w[1]
            );
        }
    }

    // ── walk_arrived ─────────────────────────────────────────────────────────

    #[test]
    fn arrived_false_before_duration() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        assert!(
            !walk_arrived(&profile, profile.duration_ms - 1),
            "must not arrive before duration_ms elapses"
        );
    }

    #[test]
    fn arrived_false_during_pause() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        // At exactly duration_ms we are in the pause window.
        assert!(
            !walk_arrived(&profile, profile.duration_ms),
            "must not arrive at duration_ms (still in pause)"
        );
        // Midway through pause.
        let mid_pause = profile.duration_ms + profile.pause_ms / 2;
        assert!(
            !walk_arrived(&profile, mid_pause),
            "must not arrive mid-pause"
        );
    }

    #[test]
    fn arrived_true_after_pause() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        let after = profile.duration_ms + profile.pause_ms;
        assert!(
            walk_arrived(&profile, after),
            "must arrive once duration + pause elapsed"
        );
    }

    #[test]
    fn progress_holds_1000_during_pause_window() {
        // During [duration_ms, duration_ms+pause_ms), t_x1000 should be 1000
        // (agent is standing at the destination in the walk sprite).
        let profile = walk_profile(700, WalkIntent::WanderBack, id(5));
        let during_pause = profile.duration_ms + profile.pause_ms / 2;
        let p = walk_progress(&profile, during_pause);
        assert_eq!(p, 1000, "progress during pause window must be 1000, got {p}");
    }

    // ── zero-length path ─────────────────────────────────────────────────────

    #[test]
    fn zero_length_no_panic() {
        let profile = walk_profile(0, WalkIntent::SnapBack, id(6));
        // Must not panic; progress should immediately be 1000.
        let p = walk_progress(&profile, 0);
        assert_eq!(p, 1000, "zero-length walk should report full progress at t=0");
        assert!(
            walk_arrived(&profile, profile.pause_ms),
            "zero-length walk should arrive after its pause"
        );
    }

    // ── intent ordering ──────────────────────────────────────────────────────

    #[test]
    fn commute_intents_faster_than_wander_intents() {
        let l: u32 = 800;
        let agent = id(9);
        let commute_dur = walk_profile(l, WalkIntent::Entry, agent).duration_ms;
        let wander_dur  = walk_profile(l, WalkIntent::WanderOut, agent).duration_ms;
        assert!(
            commute_dur < wander_dur,
            "Entry ({commute_dur}ms) must be faster than WanderOut ({wander_dur}ms) for same path length"
        );
        let exit_dur = walk_profile(l, WalkIntent::Exit, agent).duration_ms;
        let back_dur = walk_profile(l, WalkIntent::WanderBack, agent).duration_ms;
        assert!(exit_dur < back_dur);
        let snap_dur = walk_profile(l, WalkIntent::SnapBack, agent).duration_ms;
        assert!(snap_dur < wander_dur);
    }

    #[test]
    fn exit_uses_commute_speed() {
        let l: u32 = 800;
        let a = id(0);
        let entry = walk_profile(l, WalkIntent::Entry, a);
        let exit  = walk_profile(l, WalkIntent::Exit,  a);
        assert_eq!(
            entry.v_cruise.to_bits(),
            exit.v_cruise.to_bits(),
            "Exit and Entry must use the same cruise speed (commute)"
        );
    }

    #[test]
    fn wander_out_and_back_use_same_speed() {
        let l: u32 = 600;
        let a = id(1);
        let out  = walk_profile(l, WalkIntent::WanderOut,  a);
        let back = walk_profile(l, WalkIntent::WanderBack, a);
        assert_eq!(
            out.v_cruise.to_bits(),
            back.v_cruise.to_bits(),
            "WanderOut and WanderBack must use the same cruise speed"
        );
    }
}
```

- [ ] Run to confirm compilation error (stubs use `todo!()`, tests reference them, so it compiles but panics at runtime):

```
cargo test -p pixtuoid-core physics -- --nocapture 2>&1 | head -40
```

Expected: tests compile and then **FAIL** with `not yet implemented` panics (todo!() fires). If there are compile errors, fix them before proceeding.

- [ ] Commit:

```
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  add crates/pixtuoid-core/src/physics.rs crates/pixtuoid-core/src/lib.rs
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  commit -m "test(physics): full failing test suite for walk-pace physics core"
```

---

### Task 2: Run the failing tests (red confirmation)

**Files:** (no changes — read-only verification step)

- [ ] Run the physics tests and confirm every test in the suite fails with `not yet implemented`:

```
cargo test -p pixtuoid-core physics -- --nocapture 2>&1 | tail -30
```

Expected output contains lines like:
```
test physics::tests::commute_faster_than_wander ... FAILED
test physics::tests::speed_mult_in_range ... FAILED
...
test result: FAILED. 0 passed; N failed
```

If any test unexpectedly passes, investigate — the stubs should all `todo!()`.

- [ ] Confirm the rest of the workspace still compiles (physics.rs is isolated):

```
cargo build --workspace 2>&1 | grep -E "^error" | head -20
```

Expected: no errors (todo!() stubs compile fine; only tests panic at runtime).

---

### Task 3: Implement physics.rs (green)

**Files:**
- Modify `/Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/crates/pixtuoid-core/src/physics.rs`

Replace the four stub functions with real implementations. Keep the type declarations, constants, and test module exactly as written in Task 1. Only the four `pub fn` bodies change:

- [ ] Replace the stub implementations with the following complete implementations:

```rust
//! Pure physics model for character walking.
//!
//! Imports only `crate::AgentId`. No router, no layout, no terminal deps.
//! All kinematics are f32; screen is ≤ ~4096 px → ≤ ~57k octile, well
//! within f32's 24-bit mantissa.

use crate::AgentId;

// ── Intent ────────────────────────────────────────────────────────────────────

/// Why is this walk happening? Determines which cruise speed is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkIntent {
    /// Agent spawned, walking door → desk. Brisk commute speed.
    Entry,
    /// Session ended, walking desk → door. Brisk commute speed.
    Exit,
    /// Idle wander: desk → waypoint leg. Ambling speed.
    WanderOut,
    /// Idle wander: waypoint → desk leg. Ambling speed.
    WanderBack,
    /// Routing correction snap-back. Brisk commute speed.
    SnapBack,
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Cruise speed for Entry / Exit / SnapBack walks (octile/ms ≈ 1.6 m/s).
pub const V_CRUISE_COMMUTE: f32 = 0.213;
/// Cruise speed for WanderOut / WanderBack walks (octile/ms ≈ 1.1 m/s).
pub const V_CRUISE_WANDER: f32 = 0.146;
/// Shared acceleration/deceleration constant (octile/ms²). Gives ~0.5 s ramp.
pub const WALK_ACCEL: f32 = 3.7e-4;

/// Minimum per-agent speed multiplier.
pub const SPEED_MULT_MIN: f32 = 0.85;
/// Maximum per-agent speed multiplier.
pub const SPEED_MULT_MAX: f32 = 1.20;

/// Minimum arrival settle pause (ms).
pub const PAUSE_MS_MIN: u64 = 200;
/// Maximum arrival settle pause (ms).
pub const PAUSE_MS_MAX: u64 = 400;

// ── Profile ───────────────────────────────────────────────────────────────────

/// Frozen kinematic profile for one walk leg, computed once at walk-start.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkProfile {
    /// Accel → cruise → decel total time, **excluding** arrival pause.
    pub duration_ms: u64,
    /// Per-agent arrival settle before the pose flips to seated/at-waypoint.
    pub pause_ms: u64,
    /// Snapshotted A* path length (octile units).
    pub path_len_octile: u32,
    /// Effective cruise speed after `speed_mult` applied.
    pub v_cruise: f32,
    /// Acceleration constant (same as `WALK_ACCEL`; stored for walk_progress).
    pub accel: f32,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Deterministic per-agent speed multiplier in [SPEED_MULT_MIN, SPEED_MULT_MAX].
///
/// Uses bits 24..34 of the agent's hash (10 bits → 1024 buckets), mapping
/// linearly to [0.85, 1.20]. Disjoint from `personality_for` (bits 0..14) and
/// from the low-16 bits used by `cycle_ms_for`.
pub fn speed_mult(agent_id: AgentId) -> f32 {
    // FNV-1a does not avalanche mid/high bits for short, similar AgentId
    // inputs (desk-adjacent ids collide to ~2 buckets). Finalize with
    // splitmix64 before slicing so distinct agents get distinct speeds.
    let h = agent_id.raw();
    let z = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    let z = z ^ (z >> 31);
    let bits = (z >> 24) & 0x3FF; // 0..=1023
    let t = bits as f32 / 1023.0; // [0.0, 1.0]
    SPEED_MULT_MIN + t * (SPEED_MULT_MAX - SPEED_MULT_MIN)
}

/// Deterministic per-agent arrival pause in [PAUSE_MS_MIN, PAUSE_MS_MAX].
///
/// Uses bits 40..52 of the agent's hash (12 bits → 4096 buckets), mapping
/// linearly to [200, 400]. Independent of `speed_mult` (bits 24..34).
pub fn pause_ms_for(agent_id: AgentId) -> u64 {
    // Same splitmix64 finalize as speed_mult, but a disjoint bit window so
    // pause is independent of speed (a fast walker is not always a brief pauser).
    let h = agent_id.raw();
    let z = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    let z = z ^ (z >> 31);
    let bits = (z >> 40) & 0xFFF; // 0..=4095
    let t = bits as f64 / 4095.0; // [0.0, 1.0]
    PAUSE_MS_MIN + (t * (PAUSE_MS_MAX - PAUSE_MS_MIN) as f64) as u64
}

/// Compute the frozen kinematic profile for a walk of `path_len_octile` units.
///
/// Kinematics (all in octile and ms units):
///   L = path_len_octile, v = v_base(intent) * speed_mult(agent_id), a = WALK_ACCEL
///   L_crit = v²/a  (path must be ≥ L_crit to reach cruise)
///   Triangular  (L < L_crit): T = 2·sqrt(L/a)
///   Trapezoidal (L ≥ L_crit): T = v/a + (L - L_crit)/v   [= 2·t_a + t_c]
///
/// Zero-length paths: duration_ms = 0 so walk_progress returns 1000 immediately.
pub fn walk_profile(path_len_octile: u32, intent: WalkIntent, agent_id: AgentId) -> WalkProfile {
    let v_base = match intent {
        WalkIntent::Entry | WalkIntent::Exit | WalkIntent::SnapBack => V_CRUISE_COMMUTE,
        WalkIntent::WanderOut | WalkIntent::WanderBack => V_CRUISE_WANDER,
    };
    let v = v_base * speed_mult(agent_id);
    let a = WALK_ACCEL;
    let l = path_len_octile as f32;

    let duration_ms = if path_len_octile == 0 {
        0u64
    } else {
        // L_crit = v²/a; compare in octile units.
        let l_crit = v * v / a;
        let t_secs = if l < l_crit {
            // Triangular: T = 2·sqrt(L/a)
            2.0 * (l / a).sqrt()
        } else {
            // Trapezoidal: T = 2·(v/a) + (L - L_crit)/v
            let t_a = v / a;
            let t_c = (l - l_crit) / v;
            2.0 * t_a + t_c
        };
        // Convert seconds → milliseconds and round.
        t_secs.round() as u64
    };

    WalkProfile {
        duration_ms,
        pause_ms: pause_ms_for(agent_id),
        path_len_octile,
        v_cruise: v,
        accel: a,
    }
}

/// Render progress as `t_x1000 = round(1000 · s(elapsed_ms) / L)`.
///
/// - `elapsed_ms < duration_ms`: physics kinematics (accel/cruise/decel).
/// - `elapsed_ms >= duration_ms`: saturates at 1000 (also covers pause window).
/// - Zero-length profile: always returns 1000.
pub fn walk_progress(p: &WalkProfile, elapsed_ms: u64) -> u16 {
    if p.path_len_octile == 0 || elapsed_ms >= p.duration_ms {
        return 1000;
    }

    let l = p.path_len_octile as f32;
    let v = p.v_cruise;
    let a = p.accel;
    // Convert elapsed to seconds (octile/ms units → octile/s² would be a=3.7e-4*1e6=370,
    // but we keep everything in ms throughout: a is in octile/ms², t in ms).
    let t = elapsed_ms as f32; // ms
    let l_crit = v * v / a; // octile

    let s = if l < l_crit {
        // Triangular regime.
        // T_ms = 2·sqrt(L/a) (a in octile/ms²)
        let t_half = (l / a).sqrt(); // ms to peak
        if t <= t_half {
            0.5 * a * t * t
        } else {
            let t_total = 2.0 * t_half;
            let dt = t_total - t;
            l - 0.5 * a * dt * dt
        }
    } else {
        // Trapezoidal regime.
        let t_a = v / a; // accel time (ms)
        let d_a = 0.5 * a * t_a * t_a; // = L_crit/2 = v²/(2a)
        let t_c = (l - l_crit) / v; // cruise time (ms)
        let t_cruise_end = t_a + t_c;
        let t_total = 2.0 * t_a + t_c;

        if t <= t_a {
            // Accel phase.
            0.5 * a * t * t
        } else if t <= t_cruise_end {
            // Cruise phase: constant velocity.
            d_a + v * (t - t_a)
        } else {
            // Decel phase.
            let dt = t_total - t;
            l - 0.5 * a * dt * dt
        }
    };

    // Clamp s to [0, L] before dividing (floating-point edge cases at boundaries).
    let s_clamped = s.max(0.0).min(l);
    (1000.0 * s_clamped / l).round() as u16
}

/// Returns `true` when the full walk + pause has elapsed.
///
/// `elapsed_ms >= duration_ms + pause_ms` — the pose flip to seated/at-waypoint
/// happens only after the arrival settle beat completes.
pub fn walk_arrived(p: &WalkProfile, elapsed_ms: u64) -> bool {
    elapsed_ms >= p.duration_ms + p.pause_ms
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ids ──────────────────────────────────────────────────────────

    fn id(n: u8) -> AgentId {
        AgentId::from_parts("test", &format!("agent-{n}"))
    }

    // ── Constant sanity ─────────────────────────────────────────────────────

    #[test]
    fn commute_faster_than_wander() {
        assert!(
            V_CRUISE_COMMUTE > V_CRUISE_WANDER,
            "commute speed ({V_CRUISE_COMMUTE}) must exceed wander speed ({V_CRUISE_WANDER})"
        );
    }

    // ── speed_mult ──────────────────────────────────────────────────────────

    #[test]
    fn speed_mult_in_range() {
        for n in 0..=50u8 {
            let m = speed_mult(id(n));
            assert!(
                m >= SPEED_MULT_MIN && m <= SPEED_MULT_MAX,
                "agent {n}: speed_mult {m} out of [{SPEED_MULT_MIN}, {SPEED_MULT_MAX}]"
            );
        }
    }

    #[test]
    fn speed_mult_is_deterministic() {
        let a = id(7);
        assert_eq!(
            speed_mult(a),
            speed_mult(a),
            "speed_mult must be deterministic for the same AgentId"
        );
    }

    #[test]
    fn speed_mult_varies_across_agents() {
        let values: Vec<f32> = (0..20u8).map(|n| speed_mult(id(n))).collect();
        let distinct: std::collections::HashSet<u32> =
            values.iter().map(|v| v.to_bits()).collect();
        assert!(
            distinct.len() >= 5,
            "expected variance in speed_mult across agents, got {distinct:?}"
        );
    }

    // ── pause_ms_for ────────────────────────────────────────────────────────

    #[test]
    fn pause_ms_in_range() {
        for n in 0..=50u8 {
            let p = pause_ms_for(id(n));
            assert!(
                p >= PAUSE_MS_MIN && p <= PAUSE_MS_MAX,
                "agent {n}: pause_ms {p} out of [{PAUSE_MS_MIN}, {PAUSE_MS_MAX}]"
            );
        }
    }

    #[test]
    fn pause_ms_independent_of_speed_mult() {
        // Verify at least some agents have different pause_ms while sharing
        // the same broad speed bucket — i.e. the two values are not identical
        // linear functions of each other.
        let pairs: Vec<(f32, u64)> = (0..50u8).map(|n| (speed_mult(id(n)), pause_ms_for(id(n)))).collect();
        // Correlation: count agents whose speed_mult is in the lower half of
        // the range but whose pause_ms is in the upper half, and vice versa.
        let speed_mid = (SPEED_MULT_MIN + SPEED_MULT_MAX) / 2.0;
        let pause_mid = (PAUSE_MS_MIN + PAUSE_MS_MAX) / 2;
        let cross_a = pairs.iter().filter(|(s, p)| *s < speed_mid && *p > pause_mid).count();
        let cross_b = pairs.iter().filter(|(s, p)| *s >= speed_mid && *p <= pause_mid).count();
        assert!(
            cross_a + cross_b >= 4,
            "pause_ms should be independent of speed_mult; cross-quadrant count too low: {cross_a}+{cross_b}"
        );
    }

    // ── walk_profile: triangular regime ─────────────────────────────────────

    /// L_crit = v²/a. A path shorter than L_crit never reaches cruise.
    fn l_crit(v: f32) -> f32 {
        v * v / WALK_ACCEL
    }

    #[test]
    fn triangular_duration_formula() {
        // For L < L_crit: T = 2·sqrt(L/a). Use v_commute with speed_mult=1.0
        // by choosing an agent whose speed_mult is exactly 1.0 … which we
        // can't guarantee. Instead use a SHORT path and verify the formula
        // relationship rather than an absolute value.
        //
        // Strategy: pick L = L_crit/4 (well into triangular regime for any
        // agent speed in [0.85,1.20]·V_CRUISE_COMMUTE).
        // T_expected = 2·sqrt(L/a); allow ±5ms for rounding.
        let v_min = V_CRUISE_COMMUTE * SPEED_MULT_MIN;
        let l_crit_min = l_crit(v_min);
        let l = (l_crit_min / 4.0) as u32; // guaranteed triangular for all agents

        for n in 0..10u8 {
            let profile = walk_profile(l, WalkIntent::Entry, id(n));
            let v = profile.v_cruise;
            let l_crit_v = l_crit(v);
            assert!(
                (l as f32) < l_crit_v,
                "agent {n}: L={l} should be < L_crit={l_crit_v}"
            );
            // T = 2·sqrt(L / a)
            let t_expected_ms = (2.0 * ((l as f32) / WALK_ACCEL).sqrt()) as u64;
            let diff = profile.duration_ms.abs_diff(t_expected_ms);
            assert!(
                diff <= 5,
                "agent {n}: triangular T={} expected≈{t_expected_ms} (diff={diff}ms)",
                profile.duration_ms
            );
        }
    }

    #[test]
    fn trapezoidal_duration_formula() {
        // For L >= L_crit: T = v/a + (L - L_crit)/v.
        // Use L = 1200 (≫ L_crit for all agents under commute speed).
        let l: u32 = 1200;

        for n in 0..10u8 {
            let profile = walk_profile(l, WalkIntent::Entry, id(n));
            let v = profile.v_cruise;
            let l_f = l as f32;
            let lc = l_crit(v);
            assert!(
                l_f >= lc,
                "agent {n}: L={l_f} should be >= L_crit={lc} for trapezoidal"
            );
            // T = t_a + t_c + t_a = v/a + (L-L_crit)/v
            let t_a = v / WALK_ACCEL;
            let t_c = (l_f - lc) / v;
            let t_expected_ms = (2.0 * t_a + t_c) as u64;
            let diff = profile.duration_ms.abs_diff(t_expected_ms);
            assert!(
                diff <= 5,
                "agent {n}: trapezoidal T={} expected≈{t_expected_ms} (diff={diff}ms)",
                profile.duration_ms
            );
        }
    }

    // ── walk_progress: boundary values ──────────────────────────────────────

    const EPS: u16 = 2; // tolerance on t_x1000

    #[test]
    fn progress_at_zero_is_zero() {
        let profile = walk_profile(1000, WalkIntent::Entry, id(0));
        let p = walk_progress(&profile, 0);
        assert!(p <= EPS, "p(0) should be ≈0, got {p}");
    }

    #[test]
    fn progress_at_duration_is_1000() {
        let profile = walk_profile(1000, WalkIntent::Entry, id(0));
        let p = walk_progress(&profile, profile.duration_ms);
        assert!(
            p >= 1000 - EPS,
            "p(T) should be ≈1000, got {p}"
        );
    }

    #[test]
    fn progress_at_half_duration_triangular_is_near_500() {
        // In the triangular regime, s(T/2) = L/2 exactly (symmetry), so p=500.
        let v_min = V_CRUISE_COMMUTE * SPEED_MULT_MIN;
        let l_crit_min = l_crit(v_min);
        let l = (l_crit_min / 4.0) as u32;
        let profile = walk_profile(l, WalkIntent::Entry, id(0));
        let half = profile.duration_ms / 2;
        let p = walk_progress(&profile, half);
        assert!(
            (500u16).abs_diff(p) <= EPS + 10,
            "triangular p(T/2) should be ≈500, got {p}"
        );
    }

    #[test]
    fn progress_at_half_duration_trapezoidal() {
        // In the trapezoidal regime, T/2 falls somewhere in the cruise band
        // (for long paths). p should be > 400 and < 600 (symmetry; doesn't
        // need to be exactly 500 because accel != decel fractions differ).
        let l: u32 = 1200;
        let profile = walk_profile(l, WalkIntent::Entry, id(0));
        let half = profile.duration_ms / 2;
        let p = walk_progress(&profile, half);
        assert!(
            (400..=600).contains(&p),
            "trapezoidal p(T/2) should be in 400..=600, got {p}"
        );
    }

    // ── walk_progress: cruise plateau proves constant velocity ───────────────

    #[test]
    fn cruise_plateau_has_constant_delta() {
        // During cruise, Δs per Δt is constant → equal Δ(t_x1000) for equal Δt.
        // Use L=1200 (trapezoidal, clear cruise band).
        let l: u32 = 1200;
        let profile = walk_profile(l, WalkIntent::Entry, id(3));
        let v = profile.v_cruise;
        let lc = l_crit(v);
        // t_a = time to reach cruise (ms)
        let t_a_ms = (v / WALK_ACCEL) as u64;
        // sample 5 points in the cruise band
        let cruise_start = t_a_ms + 50;
        let cruise_end = profile.duration_ms - t_a_ms - 50; // symmetric decel
        assert!(
            cruise_start < cruise_end,
            "need a cruise band: t_a={t_a_ms}ms, T={}ms, L={l}, Lc={lc}",
            profile.duration_ms
        );
        let step = (cruise_end - cruise_start) / 5;
        let samples: Vec<u16> = (0..=5)
            .map(|i| walk_progress(&profile, cruise_start + i * step))
            .collect();
        let deltas: Vec<i32> = samples.windows(2).map(|w| w[1] as i32 - w[0] as i32).collect();
        let first = deltas[0];
        for (i, d) in deltas.iter().enumerate() {
            assert!(
                (d - first).abs() <= EPS as i32,
                "cruise Δ[{i}]={d} differs from Δ[0]={first} by more than {EPS} — not constant velocity"
            );
        }
    }

    // ── walk_progress: saturation and monotonicity ───────────────────────────

    #[test]
    fn progress_saturates_at_1000() {
        let profile = walk_profile(500, WalkIntent::Entry, id(1));
        // Well past duration
        let p = walk_progress(&profile, profile.duration_ms * 3);
        assert_eq!(p, 1000, "progress must saturate at 1000");
    }

    #[test]
    fn progress_is_monotone() {
        let profile = walk_profile(800, WalkIntent::WanderOut, id(2));
        let samples: Vec<u16> = (0..=20)
            .map(|i| walk_progress(&profile, i * profile.duration_ms / 20))
            .collect();
        for w in samples.windows(2) {
            assert!(
                w[1] >= w[0],
                "progress must be non-decreasing, got {} then {}",
                w[0],
                w[1]
            );
        }
    }

    // ── walk_arrived ─────────────────────────────────────────────────────────

    #[test]
    fn arrived_false_before_duration() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        assert!(
            !walk_arrived(&profile, profile.duration_ms - 1),
            "must not arrive before duration_ms elapses"
        );
    }

    #[test]
    fn arrived_false_during_pause() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        // At exactly duration_ms we are in the pause window.
        assert!(
            !walk_arrived(&profile, profile.duration_ms),
            "must not arrive at duration_ms (still in pause)"
        );
        // Midway through pause.
        let mid_pause = profile.duration_ms + profile.pause_ms / 2;
        assert!(
            !walk_arrived(&profile, mid_pause),
            "must not arrive mid-pause"
        );
    }

    #[test]
    fn arrived_true_after_pause() {
        let profile = walk_profile(600, WalkIntent::Exit, id(4));
        let after = profile.duration_ms + profile.pause_ms;
        assert!(
            walk_arrived(&profile, after),
            "must arrive once duration + pause elapsed"
        );
    }

    #[test]
    fn progress_holds_1000_during_pause_window() {
        // During [duration_ms, duration_ms+pause_ms), t_x1000 should be 1000
        // (agent is standing at the destination in the walk sprite).
        let profile = walk_profile(700, WalkIntent::WanderBack, id(5));
        let during_pause = profile.duration_ms + profile.pause_ms / 2;
        let p = walk_progress(&profile, during_pause);
        assert_eq!(p, 1000, "progress during pause window must be 1000, got {p}");
    }

    // ── zero-length path ─────────────────────────────────────────────────────

    #[test]
    fn zero_length_no_panic() {
        let profile = walk_profile(0, WalkIntent::SnapBack, id(6));
        // Must not panic; progress should immediately be 1000.
        let p = walk_progress(&profile, 0);
        assert_eq!(p, 1000, "zero-length walk should report full progress at t=0");
        assert!(
            walk_arrived(&profile, profile.pause_ms),
            "zero-length walk should arrive after its pause"
        );
    }

    // ── intent ordering ──────────────────────────────────────────────────────

    #[test]
    fn commute_intents_faster_than_wander_intents() {
        let l: u32 = 800;
        let agent = id(9);
        let commute_dur = walk_profile(l, WalkIntent::Entry, agent).duration_ms;
        let wander_dur  = walk_profile(l, WalkIntent::WanderOut, agent).duration_ms;
        assert!(
            commute_dur < wander_dur,
            "Entry ({commute_dur}ms) must be faster than WanderOut ({wander_dur}ms) for same path length"
        );
        let exit_dur = walk_profile(l, WalkIntent::Exit, agent).duration_ms;
        let back_dur = walk_profile(l, WalkIntent::WanderBack, agent).duration_ms;
        assert!(exit_dur < back_dur);
        let snap_dur = walk_profile(l, WalkIntent::SnapBack, agent).duration_ms;
        assert!(snap_dur < wander_dur);
    }

    #[test]
    fn exit_uses_commute_speed() {
        let l: u32 = 800;
        let a = id(0);
        let entry = walk_profile(l, WalkIntent::Entry, a);
        let exit  = walk_profile(l, WalkIntent::Exit,  a);
        assert_eq!(
            entry.v_cruise.to_bits(),
            exit.v_cruise.to_bits(),
            "Exit and Entry must use the same cruise speed (commute)"
        );
    }

    #[test]
    fn wander_out_and_back_use_same_speed() {
        let l: u32 = 600;
        let a = id(1);
        let out  = walk_profile(l, WalkIntent::WanderOut,  a);
        let back = walk_profile(l, WalkIntent::WanderBack, a);
        assert_eq!(
            out.v_cruise.to_bits(),
            back.v_cruise.to_bits(),
            "WanderOut and WanderBack must use the same cruise speed"
        );
    }
}
```

- [ ] Run the test suite and confirm all physics tests pass:

```
cargo test -p pixtuoid-core physics -- --nocapture 2>&1
```

Expected:
```
test physics::tests::arrived_false_before_duration ... ok
test physics::tests::arrived_false_during_pause ... ok
test physics::tests::arrived_true_after_pause ... ok
test physics::tests::commute_faster_than_wander ... ok
test physics::tests::commute_intents_faster_than_wander_intents ... ok
test physics::tests::cruise_plateau_has_constant_delta ... ok
test physics::tests::exit_uses_commute_speed ... ok
test physics::tests::pause_ms_in_range ... ok
test physics::tests::pause_ms_independent_of_speed_mult ... ok
test physics::tests::progress_at_duration_is_1000 ... ok
test physics::tests::progress_at_half_duration_trapezoidal ... ok
test physics::tests::progress_at_half_duration_triangular_is_near_500 ... ok
test physics::tests::progress_at_zero_is_zero ... ok
test physics::tests::progress_holds_1000_during_pause_window ... ok
test physics::tests::progress_is_monotone ... ok
test physics::tests::progress_saturates_at_1000 ... ok
test physics::tests::speed_mult_in_range ... ok
test physics::tests::speed_mult_is_deterministic ... ok
test physics::tests::speed_mult_varies_across_agents ... ok
test physics::tests::trapezoidal_duration_formula ... ok
test physics::tests::triangular_duration_formula ... ok
test physics::tests::wander_out_and_back_use_same_speed ... ok
test physics::tests::zero_length_no_panic ... ok

test result: ok. 23 passed; 0 failed
```

---

### Task 4: Full workspace green check + commit

**Files:** (no changes — verification + commit)

- [ ] Run the full pixtuoid-core test suite to confirm no regressions:

```
cargo test -p pixtuoid-core -- --nocapture 2>&1 | tail -10
```

Expected: `test result: ok. N passed; 0 failed` (N will be the existing count + 23 new physics tests).

- [ ] Run the full workspace build to confirm invariant #1 (no terminal deps leaked into core):

```
cargo build --workspace 2>&1 | grep -E "^error" | head -20
```

Expected: no errors.

- [ ] Run clippy on the core crate:

```
cargo clippy -p pixtuoid-core -- -D warnings 2>&1 | grep -E "^error" | head -20
```

If clippy complains about `let _ = (...)` in the stubs (which are now gone), there are no stubs remaining. If it flags anything else, fix it before committing.

- [ ] Run rustfmt check:

```
cargo fmt --all --check 2>&1
```

If it reports diffs, run `cargo fmt --all` and re-check.

- [ ] Commit the complete implementation:

```
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  add crates/pixtuoid-core/src/physics.rs crates/pixtuoid-core/src/lib.rs
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  commit -m "feat(physics): pure core walk-pace physics module (TDD, 23 tests green)

- WalkIntent enum (Entry/Exit/WanderOut/WanderBack/SnapBack)
- WalkProfile struct with duration_ms, pause_ms, path_len_octile, v_cruise, accel
- speed_mult: bits 24..34 → [0.85, 1.20] deterministic per-agent multiplier
- pause_ms_for: bits 40..52 → [200, 400] independent of speed
- walk_profile: trapezoidal/triangular kinematics from snapshotted path length
- walk_progress: t_x1000 with physics ramps + saturation at 1000
- walk_arrived: gates pose flip after duration + pause
- All 23 tests pass; core invariant #1 holds (no terminal/router deps)"
```



## Phase 1: Tui scaffolding (no behavior change)

### Task 1: Promote `octile_distance` to `pub(in crate::tui)` in `tui/pose.rs`

**Files:** Modify `crates/pixtuoid/src/tui/pose.rs` (line 239)

- [ ] 1. Change the visibility of `octile_distance` from `fn` to `pub(in crate::tui)`:

```rust
// crates/pixtuoid/src/tui/pose.rs  — only the fn declaration changes:
pub(in crate::tui) fn octile_distance(a: Point, b: Point) -> u32 {
    let dx = (a.x as i32 - b.x as i32).unsigned_abs();
    let dy = (a.y as i32 - b.y as i32).unsigned_abs();
    14 * dx.min(dy) + 10 * (dx.max(dy) - dx.min(dy))
}
```

- [ ] 2. Verify the workspace still compiles:

```
cargo build -p pixtuoid 2>&1 | grep -E "^error"
```
Expected: no output (zero errors).

- [ ] 3. Commit:

```
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  commit -am "refactor(tui/pose): promote octile_distance to pub(in crate::tui)"
```

---

### Task 2: Create `crates/pixtuoid/src/tui/motion.rs`

**Files:** Create `crates/pixtuoid/src/tui/motion.rs`

- [ ] 1. Create the file with the complete content below.  `WalkProfile` and `WalkIntent` come from `pixtuoid_core::physics` (that module is added in Phase 0; the types are referenced here so the scaffolding compiles once Phase 0 lands). `WaypointKind` and `Point` are already re-exported from `crate::tui::layout`.

```rust
//! Per-agent walk-timing state owned by the TUI layer.
//!
//! `MotionState` is the single source of truth for in-flight walk profiles
//! (entry, exit, snap-back, and wander phases). It is keyed on `AgentId`
//! inside `FloorCtx::motion` and evicted when the agent leaves the scene.
//!
//! `octile_path_len` converts an A*-routed `&[Point]` slice into the same
//! octile distance metric the router uses, delegating to the already-
//! promoted `pose::octile_distance`.

use std::time::SystemTime;

use pixtuoid_core::physics::{WalkIntent, WalkProfile};
use pixtuoid_core::AgentId;

use crate::tui::layout::{Point, WaypointKind};
use crate::tui::pose::octile_distance;

/// Phase the wander cycle is currently in for a given agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WanderPhase {
    /// Sitting at the desk between trips.
    Seated,
    /// Walking from desk to the chosen waypoint.
    WalkingOut,
    /// Standing/sitting at the waypoint during the dwell beat.
    AtWaypoint,
    /// Walking from the waypoint back to the desk.
    WalkingBack,
}

impl Default for WanderPhase {
    fn default() -> Self {
        WanderPhase::Seated
    }
}

/// Per-agent walk-timing state owned by the TUI layer.
///
/// One `MotionState` exists per live agent (per floor). Fields are
/// `Option` so the struct can be default-initialised for new agents
/// and populated lazily on the first relevant walk-start frame.
#[derive(Debug, Clone)]
pub struct MotionState {
    pub agent_id: AgentId,

    // --- entry / exit / snap-back one-shot walks ---

    /// `(walk_started_at, profile)` snapshotted once at door-crossing.
    pub entry: Option<(SystemTime, WalkProfile)>,
    /// `(walk_started_at, profile)` snapshotted once when `exiting_at` fires.
    pub exit: Option<(SystemTime, WalkProfile)>,
    /// `(walk_started_at, profile, snap_target)` for the state-transition
    /// snap-back walk (replaces the old `since_state < SNAP_BACK_MS` guard).
    pub snap_back: Option<(SystemTime, WalkProfile, Point)>,

    // --- cyclic wander state ---

    /// Monotonically increasing wander cycle counter. Incremented each time
    /// `WalkingBack` completes. Determines which waypoint destination is
    /// selected (mirrors `core::pose`'s `cycle_n` derivation).
    pub wander_cycle_n: u64,
    /// Current phase of the wander cycle.
    pub wander_phase: WanderPhase,
    /// Wall-clock instant the current phase began. Every phase transition
    /// resets this so each leg has its own independent clock.
    pub wander_phase_started_at: SystemTime,
    /// Walk profile for the current out- or back-leg, snapshotted at the
    /// phase transition. `None` while `Seated` or `AtWaypoint`.
    pub wander_profile: Option<WalkProfile>,
    /// Destination pixel of the current wander trip (desk→waypoint→desk).
    /// Reset on each new `WalkingOut` phase.
    pub wander_dest: Point,
    /// Kind of the current wander waypoint, if it is a named waypoint.
    pub wander_dest_kind: Option<WaypointKind>,
    /// Index into `layout.waypoints` for the current wander destination,
    /// if it is a named waypoint.
    pub wander_dest_wp_idx: Option<usize>,
}

impl MotionState {
    /// Construct a fresh `MotionState` for `agent_id`.
    /// All optional fields are `None`; wander starts in `Seated` phase
    /// anchored to `now` so `advance_wander` can detect bootstrap on first call.
    pub fn new(agent_id: AgentId, now: SystemTime) -> Self {
        Self {
            agent_id,
            entry: None,
            exit: None,
            snap_back: None,
            wander_cycle_n: 0,
            wander_phase: WanderPhase::Seated,
            wander_phase_started_at: now,
            wander_profile: None,
            // Placeholder — replaced on first WalkingOut transition.
            wander_dest: Point { x: 0, y: 0 },
            wander_dest_kind: None,
            wander_dest_wp_idx: None,
        }
    }
}

/// Sum of octile distances along a routed polyline.
///
/// Reuses `pose::octile_distance` (the same metric A* uses) so the
/// snapshotted path length is consistent with per-segment timing.
///
/// Returns 0 for a path with fewer than 2 points (no segments).
pub fn octile_path_len(path: &[Point]) -> u32 {
    if path.len() < 2 {
        return 0;
    }
    path.windows(2)
        .map(|w| octile_distance(w[0], w[1]))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::AgentId;
    use std::time::{Duration, SystemTime};

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn id() -> AgentId {
        AgentId::from_transcript_path("/test/motion.jsonl")
    }

    // --- octile_path_len --------------------------------------------------

    #[test]
    fn path_len_empty_is_zero() {
        assert_eq!(octile_path_len(&[]), 0);
    }

    #[test]
    fn path_len_single_point_is_zero() {
        let p = Point { x: 10, y: 20 };
        assert_eq!(octile_path_len(&[p]), 0);
    }

    #[test]
    fn path_len_orthogonal_segment() {
        // 5 px right: octile = 10*5 = 50
        let a = Point { x: 0, y: 0 };
        let b = Point { x: 5, y: 0 };
        assert_eq!(octile_path_len(&[a, b]), 50);
    }

    #[test]
    fn path_len_diagonal_segment() {
        // 3 px diagonal: octile = 14*3 = 42
        let a = Point { x: 0, y: 0 };
        let b = Point { x: 3, y: 3 };
        assert_eq!(octile_path_len(&[a, b]), 42);
    }

    #[test]
    fn path_len_multi_segment_sums() {
        // right 4 (40) + down 3 (30) = 70
        let a = Point { x: 0, y: 0 };
        let b = Point { x: 4, y: 0 };
        let c = Point { x: 4, y: 3 };
        assert_eq!(octile_path_len(&[a, b, c]), 70);
    }

    // --- MotionState::new -------------------------------------------------

    #[test]
    fn motion_state_new_default_fields() {
        let now = t0();
        let ms = MotionState::new(id(), now);
        assert!(ms.entry.is_none());
        assert!(ms.exit.is_none());
        assert!(ms.snap_back.is_none());
        assert_eq!(ms.wander_cycle_n, 0);
        assert_eq!(ms.wander_phase, WanderPhase::Seated);
        assert_eq!(ms.wander_phase_started_at, now);
        assert!(ms.wander_profile.is_none());
        assert!(ms.wander_dest_kind.is_none());
        assert!(ms.wander_dest_wp_idx.is_none());
    }

    #[test]
    fn wander_phase_default_is_seated() {
        assert_eq!(WanderPhase::default(), WanderPhase::Seated);
    }
}
```

- [ ] 2. Note: this file references `pixtuoid_core::physics::{WalkIntent, WalkProfile}`. That module is added in Phase 0. If Phase 0 has not landed yet, temporarily stub the import with `use pixtuoid_core::physics as _physics; type WalkIntent = _physics::WalkIntent; type WalkProfile = _physics::WalkProfile;` — but the correct approach is to merge Phase 0 first. The plan assembler must ensure Phase 0 (core physics module) is committed before this phase.

---

### Task 3: Register `motion` in `tui/mod.rs` and add `motion` + `door_anim_max_ms` fields to `FloorCtx`

**Files:**
- Modify `crates/pixtuoid/src/tui/mod.rs` (line 12, after `pub mod pose;`)
- Modify `crates/pixtuoid/src/tui/floor.rs` (lines 9–82)

- [ ] 1. Add `pub mod motion;` to `crates/pixtuoid/src/tui/mod.rs` after the existing `pub mod pose;` line:

```rust
// crates/pixtuoid/src/tui/mod.rs — add one line after `pub mod pose;`:
pub mod motion;
```

The file after the addition (showing only the pub-mod block, no other changes):

```rust
pub mod anim;
pub mod chitchat;
pub mod embedded_pack;
pub mod floor;
pub mod frame_cache;
pub mod hit_test;
pub mod layout;
pub mod motion;        // ← new
pub mod pathfind;
pub mod pet;
pub mod pixel_painter;
pub mod pose;
pub mod renderer;
pub mod theme;
pub mod tui_renderer;
pub mod widgets;
```

- [ ] 2. Add `pub motion: HashMap<AgentId, MotionState>` and `pub door_anim_max_ms: u64` to `FloorCtx`, and initialise them in `FloorCtx::new()`.

Replace the entire `FloorCtx` struct, its `Default` impl, and its `impl FloorCtx` block in `crates/pixtuoid/src/tui/floor.rs`. The file header and all other code (FloorMeta, LightingState, FloorTransition, free fns, tests) remain untouched. Only the three items below change:

```rust
// ── Add to the top-of-file use block (after line 16 `use crate::tui::pose::PoseHistory;`) ──
use std::collections::HashMap;

use crate::tui::motion::MotionState;

// ── FloorCtx struct (replaces lines 58-64) ──
/// Per-floor rendering state. Each floor gets its own pathfinder,
/// occupancy overlay, pose history, recolored-frame cache, lighting
/// fade state, and motion map so floors are fully independent.
pub struct FloorCtx {
    pub router: AStarRouter,
    pub overlay: OccupancyOverlay,
    pub history: PoseHistory,
    pub cache: FrameCache,
    pub light: LightingState,
    /// Per-agent walk-timing state (physics profiles for entry/exit/wander).
    /// Evicted alongside `history` and `cache` when the agent leaves.
    pub motion: HashMap<AgentId, MotionState>,
    /// Longest in-flight entry- or exit-walk `duration_ms + pause_ms` on
    /// this floor (ms). Written each frame by `derive_with_routing`; read by
    /// `compute_door_frame_idx` to drive door-open cosmetics without a
    /// hardcoded `ENTRY_ANIMATION_MS`.
    pub door_anim_max_ms: u64,
}

// ── Default impl (replaces lines 66-70) ──
impl Default for FloorCtx {
    fn default() -> Self {
        Self::new()
    }
}

// ── FloorCtx::new() (replaces lines 72-82) ──
impl FloorCtx {
    pub fn new() -> Self {
        Self {
            router: AStarRouter::new(),
            overlay: OccupancyOverlay::new(),
            history: PoseHistory::new(),
            cache: FrameCache::new(),
            light: LightingState::new(),
            motion: HashMap::new(),
            door_anim_max_ms: 0,
        }
    }
}
```

- [ ] 3. The `use` additions must sit alongside the existing imports. The full top-of-file import block for `floor.rs` after the change:

```rust
use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::state::{AgentSlot, SceneState};
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::tui::frame_cache::FrameCache;
use crate::tui::motion::MotionState;
use crate::tui::pathfind::AStarRouter;
use crate::tui::pose::PoseHistory;
```

- [ ] 4. Build the workspace to confirm no compile errors:

```
cargo build -p pixtuoid 2>&1 | grep -E "^error"
```
Expected: no output.

- [ ] 5. Run the full test suite to confirm no regressions:

```
cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -20
```
Expected: all tests pass, zero failures.

- [ ] 6. Commit:

```
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  commit -am "feat(tui): motion.rs scaffolding + FloorCtx motion/door_anim_max_ms fields"
```

---

### Task 4: Compile-only smoke test for the scaffolding

**Files:** No new files — verify the build is clean.

- [ ] 1. Confirm `cargo build -p pixtuoid` succeeds without warnings about unused fields (they are `pub` so the compiler doesn't warn, but check anyway):

```
cargo build -p pixtuoid 2>&1 | grep -iE "(warning|error)\[" | grep -v "^$"
```
Expected: zero `error[...]` lines. Any `warning[unused_import]` lines indicate a stray import that must be removed before proceeding.

- [ ] 2. Confirm the motion module tests pass in isolation:

```
cargo test -p pixtuoid tui::motion 2>&1 | tail -15
```
Expected output contains lines like:
```
test tui::motion::tests::path_len_empty_is_zero ... ok
test tui::motion::tests::path_len_single_point_is_zero ... ok
test tui::motion::tests::path_len_orthogonal_segment ... ok
test tui::motion::tests::path_len_diagonal_segment ... ok
test tui::motion::tests::path_len_multi_segment_sums ... ok
test tui::motion::tests::motion_state_new_default_fields ... ok
test tui::motion::tests::wander_phase_default_is_seated ... ok

test result: ok. 7 passed; 0 failed
```

- [ ] 3. Confirm the four existing `pose.rs` snap-back tests still pass:

```
cargo test -p pixtuoid tui::pose::tests 2>&1 | tail -15
```
Expected: `test result: ok. N passed; 0 failed` (N ≥ 9, including the four snap-back tests and the existing history/routing tests).

- [ ] 4. If all checks pass, the phase is complete. No additional commit is needed (the Task 3 commit covers everything).



## Phase 2: Thread the motion param (behavior-preserving)

### Task 1: Promote `octile_distance` and add `motion` to `derive_with_routing` signature

**Files:**
- Modify `crates/pixtuoid/src/tui/pose.rs` (lines 239–243, 80–87, 245 in-file test call sites)

- [ ] 1. Confirm the test suite is currently green before touching anything:
  ```
  cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -5
  ```
  Expected: `test result: ok.` (all tests pass).

- [ ] 2. In `crates/pixtuoid/src/tui/pose.rs` change `fn octile_distance` from private to `pub(in crate::tui)` (line 239) and add the `motion` import + parameter to `derive_with_routing`. Replace the existing signature block and body opening with:

  ```rust
  // at top of file — add these imports alongside the existing ones:
  use std::collections::HashMap;
  use crate::tui::motion::MotionState;
  ```

  Change `octile_distance` visibility (line 239):
  ```rust
  pub(in crate::tui) fn octile_distance(a: Point, b: Point) -> u32 {
      let dx = (a.x as i32 - b.x as i32).unsigned_abs();
      let dy = (a.y as i32 - b.y as i32).unsigned_abs();
      14 * dx.min(dy) + 10 * (dx.max(dy) - dx.min(dy))
  }
  ```

  Change `derive_with_routing` signature (replace line 80–87):
  ```rust
  pub fn derive_with_routing(
      slot: &AgentSlot,
      now: SystemTime,
      layout: &Layout,
      router: &mut dyn Router,
      overlay: &OccupancyOverlay,
      history: &mut PoseHistory,
      motion: &mut HashMap<AgentId, MotionState>,
  ) -> Option<Pose> {
  ```
  The body is unchanged — `motion` is accepted but not read yet.

- [ ] 3. Fix the **in-file test call sites** in the `#[cfg(test)]` block at the bottom of `pose.rs`. Every call to `derive_with_routing` must gain a `&mut HashMap::new()` final argument. There are 8 call sites in the test functions `snap_back_walks_from_history_when_state_just_flipped`, `snap_back_skipped_when_prev_within_min_distance`, `snap_back_skipped_after_900ms_window`, `snap_back_skipped_without_recent_history`, `multi_segment_path_maps_t_to_segment_via_octile_distance`, `at_waypoint_pose_records_position_to_history`, `delegates_to_derive_for_oob_desk`, and the final call in `at_waypoint_pose_records_position_to_history`.

  For each test function, add `let mut motion: HashMap<AgentId, MotionState> = HashMap::new();` (after the existing `let mut router = ...` line) then append `&mut motion` to the `derive_with_routing` call. Example (snap_back test):
  ```rust
  // inside each test fn, after `let mut router = StubRouter::straight();`
  use crate::tui::motion::MotionState;
  let mut motion: std::collections::HashMap<pixtuoid_core::AgentId, MotionState> =
      std::collections::HashMap::new();
  // then change the derive_with_routing call:
  match derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history, &mut motion) {
  ```

  Apply the same pattern to ALL eight test call sites.

- [ ] 4. Run only the pose tests to verify the in-file tests still pass:
  ```
  cargo test -p pixtuoid tui::pose -- --nocapture 2>&1 | tail -20
  ```
  Expected: all `snap_back_*`, `multi_segment_*`, `delegates_*`, `pose_history_*`, `at_waypoint_*` tests → PASS.

- [ ] 5. Commit:
  ```
  git add crates/pixtuoid/src/tui/pose.rs
  git commit -m "refactor(motion): promote octile_distance + thread motion param through derive_with_routing"
  ```

---

### Task 2: Add `motion` field to `DrawCtx` and `PixelCtx`

**Files:**
- Modify `crates/pixtuoid/src/tui/renderer.rs` (struct `DrawCtx`, ~line 64–104)
- Modify `crates/pixtuoid/src/tui/pixel_painter/mod.rs` (struct `PixelCtx`, ~line 69–88)

- [ ] 1. In `crates/pixtuoid/src/tui/renderer.rs`, add the import at the top (alongside existing `use crate::tui::pose;`):
  ```rust
  use crate::tui::motion::MotionState;
  ```

  Add the field to `DrawCtx` (after the `history` field, around line 69):
  ```rust
  pub history: &'a mut pose::PoseHistory,
  /// Per-floor motion state — threaded like `history`. Agents' `MotionState`
  /// entries are initialized and advanced by `derive_with_routing` (Phase 3+).
  /// Phase 2 wires the borrow; the field is accepted but not read yet.
  pub motion: &'a mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
  ```

- [ ] 2. In `crates/pixtuoid/src/tui/pixel_painter/mod.rs`, add the import (alongside existing `use crate::tui::pose;`):
  ```rust
  use crate::tui::motion::MotionState;
  ```

  Add the field to `PixelCtx` (after the `history` field, around line 78):
  ```rust
  pub history: &'a mut pose::PoseHistory,
  /// Forwarded from `DrawCtx.motion` — identical lifetime, identical
  /// borrow rules. Phase 3+ will read/write entries; Phase 2 just stores the reference.
  pub motion: &'a mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
  ```

- [ ] 3. Check compilation (there will be errors at all construction sites — that is expected, they are fixed in Task 3):
  ```
  cargo check -p pixtuoid 2>&1 | grep "^error" | head -20
  ```
  Expected: errors like `missing field 'motion'` at `DrawCtx {` and `PixelCtx {` construction sites (renderer.rs `draw_scene`, tui_renderer.rs `render`, `render_transition_floor`, snapshot.rs, and the two `derive_with_routing` calls inside pixel_painter/mod.rs that also call `history`).

- [ ] 4. Commit the struct-only changes before fixing call sites (lets git bisect cleanly):
  ```
  git add crates/pixtuoid/src/tui/renderer.rs crates/pixtuoid/src/tui/pixel_painter/mod.rs
  git commit -m "refactor(motion): add motion field to DrawCtx + PixelCtx (structs only)"
  ```

---

### Task 3: Wire `motion` at all construction sites in `tui_renderer.rs` and `renderer.rs`

**Files:**
- Modify `crates/pixtuoid/src/tui/tui_renderer.rs` (normal render path ~line 563–590; transition `render_transition_floor` fn signature + body ~line 611–657; transition call sites ~line 443–476)
- Modify `crates/pixtuoid/src/tui/renderer.rs` (`draw_scene` → `PixelCtx` construction ~line 217–236; `hit_test_agent` call ~line 244–254; `paint_label_widgets` call ~line 273–283)

- [ ] 1. In `tui_renderer.rs`, add `motion.retain` to the existing coffee/stain eviction block (immediately after the `coffee_stains.retain` line, ~line 556):
  ```rust
  self.coffee_holders
      .retain(|id| scene.agents.contains_key(id));
  self.coffee_fetched_at
      .retain(|id, _| scene.agents.contains_key(id));
  self.coffee_stains
      .retain(|id, _| scene.agents.contains_key(id));
  // Phase 2: evict motion state for departed agents.
  self.floor_ctxs[self.current_floor]
      .motion
      .retain(|id, _| scene.agents.contains_key(id));
  ```

- [ ] 2. In `tui_renderer.rs`, add `motion` to the normal-path `DrawCtx` construction block (~line 563). After `history: &mut fctx.history,` add:
  ```rust
  motion: &mut fctx.motion,
  ```

- [ ] 3. In `tui_renderer.rs`, update `render_transition_floor`'s signature to accept and forward `motion`. Add the parameter after `history`:
  ```rust
  #[allow(clippy::too_many_arguments)]
  fn render_transition_floor(
      scene: &SceneState,
      fctx: &mut FloorCtx,
      buf: &mut RgbBuffer,
      floor_meta: FloorMeta,
      buf_w: u16,
      buf_h: u16,
      active_pet: Option<&PetState>,
      floor_pet_kind: Option<PetKind>,
      theme: &'static crate::tui::theme::Theme,
      coffee_holders: &std::collections::HashSet<pixtuoid_core::AgentId>,
      coffee_fetched_at: &std::collections::HashMap<pixtuoid_core::AgentId, SystemTime>,
      coffee_stains: &std::collections::HashMap<pixtuoid_core::AgentId, Vec<StainPos>>,
      chitchat_state: &mut std::collections::HashMap<
          (usize, usize),
          crate::tui::chitchat::ActiveChitchat,
      >,
      pack: &Pack,
      now: SystemTime,
  ) {
  ```
  The body of `render_transition_floor` constructs a `PixelCtx`. Add `motion: &mut fctx.motion,` after `history: &mut fctx.history,` there.

- [ ] 4. In `tui_renderer.rs`, both transition call sites for `render_transition_floor` (~lines 443 and 460) do not need changes — the function takes `fctx` mutably and the `motion` field is read from inside. Verify the calls compile by adding the import at the top of `tui_renderer.rs` if not already present:
  ```rust
  // no new import needed — MotionState is accessed via fctx.motion which is already typed
  ```

- [ ] 5. In `renderer.rs`, update `draw_scene`'s `PixelCtx` construction (~line 217) to forward `ctx.motion`:
  ```rust
  let pixel_result = render_to_rgb_buffer(&mut PixelCtx {
      scene,
      layout: &layout,
      pack,
      now,
      buf: ctx.buf,
      cache: ctx.cache,
      router: ctx.router,
      overlay: ctx.overlay,
      history: ctx.history,
      motion: ctx.motion,   // <-- add this line
      theme,
      floor: ctx.floor,
      active_pet: ctx.active_pet,
      floor_pet_kind: ctx.floor_pet_kind,
      chitchat_state: ctx.chitchat_state,
      coffee_holders: ctx.coffee_holders,
      coffee_fetched_at: ctx.coffee_fetched_at,
      coffee_stains: ctx.coffee_stains,
      light: ctx.light,
  });
  ```

- [ ] 6. In `renderer.rs`, the `hit_test_agent` call and `paint_label_widgets` call both invoke `derive_with_routing` indirectly through `character_anchor`. Those will be fixed in Task 4 (anchors.rs). Leave those call sites alone for now.

- [ ] 7. Compile check — expect remaining errors only from `snapshot.rs`, `pixel_painter/mod.rs` derive_with_routing calls, and `anchors.rs`:
  ```
  cargo check -p pixtuoid 2>&1 | grep "^error" | head -20
  ```

- [ ] 8. Commit:
  ```
  git add crates/pixtuoid/src/tui/tui_renderer.rs crates/pixtuoid/src/tui/renderer.rs
  git commit -m "refactor(motion): wire motion into DrawCtx + PixelCtx construction + retain eviction"
  ```

---

### Task 4: Fix remaining call sites — `pixel_painter/mod.rs`, `anchors.rs`, `hit_test.rs`, `widgets/tooltip.rs`, and `examples/snapshot.rs`

**Files:**
- Modify `crates/pixtuoid/src/tui/pixel_painter/mod.rs` (two `derive_with_routing` call sites, ~lines 531–538, 841–848)
- Modify `crates/pixtuoid/src/tui/pixel_painter/anchors.rs` (`character_anchor` fn signature + body, ~line 124–134)
- Modify `crates/pixtuoid/src/tui/hit_test.rs` (`hit_test_agent` call to `character_anchor`, ~line 39)
- Modify `crates/pixtuoid/src/tui/widgets/tooltip.rs` (`paint_label_widgets` call to `character_anchor`, ~line 45)
- Modify `crates/pixtuoid/examples/snapshot.rs` (two `DrawCtx` construction sites, ~lines 229–257, 688–716)

- [ ] 1. In `pixel_painter/mod.rs`, the two `derive_with_routing` calls need `ctx.motion`. Both are inside `render_to_rgb_buffer`:

  First call (seated_agents map, ~line 531–538):
  ```rust
  let p = pose::derive_with_routing(
      a,
      ctx.now,
      ctx.layout,
      ctx.router,
      ctx.overlay,
      ctx.history,
      ctx.motion,
  );
  ```

  Second call (character loop, ~line 841–848):
  ```rust
  let Some(p) = pose::derive_with_routing(
      agent,
      ctx.now,
      ctx.layout,
      ctx.router,
      ctx.overlay,
      ctx.history,
      ctx.motion,
  ) else {
      continue;
  };
  ```

- [ ] 2. In `pixel_painter/anchors.rs`, update `character_anchor` to accept and forward `motion`. Add the import:
  ```rust
  use crate::tui::motion::MotionState;
  ```

  Change the signature (~line 124):
  ```rust
  #[allow(clippy::too_many_arguments)]
  pub(in crate::tui) fn character_anchor(
      agent: &AgentSlot,
      layout: &crate::tui::layout::Layout,
      now: SystemTime,
      router: &mut dyn Router,
      overlay: &OccupancyOverlay,
      history: &mut pose::PoseHistory,
      motion: &mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
  ) -> Option<Point> {
      let desk = *layout.home_desks.get(agent.desk_index)?;
      let pose = pose::derive_with_routing(agent, now, layout, router, overlay, history, motion)?;
      let anchor = match pose {
          Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } => seated_anchor(desk),
          Pose::StandingAtDesk => standing_at_desk_anchor(desk),
          Pose::AtWaypoint { wp, kind } => {
              let wp_obj = layout.waypoints.get(wp)?;
              match kind {
                  WaypointKind::Couch => back_couch_anchor(wp_obj.pos),
                  _ => waypoint_anchor(wp_obj.pos),
              }
          }
          Pose::AimlessAt { dest } => waypoint_anchor(dest),
          Pose::Walking {
              from, to, t_x1000, ..
          } => walking_anchor(walking_position(from, to, t_x1000)),
      };
      Some(anchor)
  }
  ```

- [ ] 3. In `hit_test.rs`, update `hit_test_agent` to accept and forward `motion`. Add the import at the top:
  ```rust
  use crate::tui::motion::MotionState;
  ```

  Change `hit_test_agent` signature and its `character_anchor` call:
  ```rust
  #[allow(clippy::too_many_arguments)]
  pub(crate) fn hit_test_agent(
      scene: &SceneState,
      layout: &Layout,
      now: SystemTime,
      router: &mut dyn Router,
      overlay: &OccupancyOverlay,
      history: &mut pose::PoseHistory,
      motion: &mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
      mx: u16,
      my: u16,
  ) -> Option<AgentId> {
      const SPRITE_W_CELLS: u16 = 8;
      const SPRITE_H_CELLS: u16 = 6;
      for agent in scene.agents.values() {
          let Some(anchor) =
              character_anchor(agent, layout, now, router, overlay, history, motion)
          else {
              continue;
          };
          let cell_x = anchor.x;
          let cell_y = anchor.y / 2;
          if mx >= cell_x
              && mx < cell_x.saturating_add(SPRITE_W_CELLS)
              && my >= cell_y
              && my < cell_y.saturating_add(SPRITE_H_CELLS)
          {
              return Some(agent.agent_id);
          }
      }
      None
  }
  ```

- [ ] 4. In `renderer.rs`, update all three call sites of `hit_test_agent` to forward `ctx.motion` (lines ~244–254 in `draw_scene` and lines ~275–283 for `paint_label_widgets`). The `hit_test_agent` call:
  ```rust
  let hovered = mouse_pos.and_then(|(mx, my)| {
      hit_test_agent(
          scene,
          &layout,
          now,
          ctx.router,
          ctx.overlay,
          ctx.history,
          ctx.motion,
          mx,
          my,
      )
  });
  ```

  The `paint_label_widgets` call:
  ```rust
  paint_label_widgets(
      f,
      scene,
      &layout,
      now,
      ctx.router,
      ctx.overlay,
      ctx.history,
      ctx.motion,
      actual_scene,
      hovered,
      theme,
  );
  ```

- [ ] 5. In `widgets/tooltip.rs`, update `paint_label_widgets` signature and its `character_anchor` call. Add the import:
  ```rust
  use crate::tui::motion::MotionState;
  ```

  Change the signature:
  ```rust
  #[allow(clippy::too_many_arguments)]
  pub(crate) fn paint_label_widgets(
      f: &mut ratatui::Frame<'_>,
      scene: &SceneState,
      layout: &Layout,
      now: SystemTime,
      router: &mut dyn Router,
      overlay: &OccupancyOverlay,
      history: &mut pose::PoseHistory,
      motion: &mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
      scene_rect: Rect,
      hovered: Option<AgentId>,
      theme: &crate::tui::theme::Theme,
  ) {
  ```

  Update the `character_anchor` call inside (line ~45):
  ```rust
  let Some(anchor) =
      character_anchor(agent, layout, now, router, overlay, history, motion)
  else {
      continue;
  };
  ```

- [ ] 6. In `examples/snapshot.rs`, add the `motion` field to both `DrawCtx` construction sites (static snapshot ~line 229 and GIF loop ~line 688).

  Add before the `DrawCtx {` block — declare a `motion` map:
  ```rust
  let mut motion: std::collections::HashMap<pixtuoid_core::AgentId,
      pixtuoid::tui::motion::MotionState> = std::collections::HashMap::new();
  ```

  Then inside `DrawCtx { ... }` add:
  ```rust
  motion: &mut motion,
  ```

  For the GIF loop, the `DrawCtx` is constructed inside a `for` loop. Declare `motion` **outside** the loop (same scope as `chitchat_state` and `light`) so it persists across frames:
  ```rust
  let mut motion: std::collections::HashMap<pixtuoid_core::AgentId,
      pixtuoid::tui::motion::MotionState> = std::collections::HashMap::new();
  let mut chitchat_state = std::collections::HashMap::new();
  let mut light = pixtuoid::tui::floor::LightingState::new();
  for i in 0..frame_count {
      // ...
      let mut draw_ctx = DrawCtx {
          // ... existing fields ...
          motion: &mut motion,
          // ...
      };
  ```

- [ ] 7. Run the full workspace test suite:
  ```
  cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -20
  ```
  Expected: all tests PASS. Output ends with `test result: ok.`

- [ ] 8. Commit:
  ```
  git add \
    crates/pixtuoid/src/tui/pixel_painter/mod.rs \
    crates/pixtuoid/src/tui/pixel_painter/anchors.rs \
    crates/pixtuoid/src/tui/hit_test.rs \
    crates/pixtuoid/src/tui/widgets/tooltip.rs \
    crates/pixtuoid/src/tui/renderer.rs \
    crates/pixtuoid/examples/snapshot.rs
  git commit -m "refactor(motion): thread motion param through all call sites (behavior-preserving)"
  ```

---

### Task 5: Verify end-to-end green and snapshot compiles

**Files:** none (verification only)

- [ ] 1. Run the complete workspace test suite one final time:
  ```
  cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -10
  ```
  Expected: `test result: ok. N passed; 0 failed; 0 ignored`

- [ ] 2. Build the snapshot example to confirm it links:
  ```
  cargo build --release --example snapshot 2>&1 | tail -5
  ```
  Expected: `Finished release profile`.

- [ ] 3. Run the snapshot example to confirm it renders without panic:
  ```
  ./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap_phase2.png 2>&1
  ```
  Expected: `wrote /tmp/snap_phase2.png` with no errors or panics. Visual output is identical to pre-phase-2 (no behavior change).

- [ ] 4. Verify clippy is clean:
  ```
  cargo clippy --workspace --all-targets --features pixtuoid-core/test-renderer -- -D warnings 2>&1 | grep "^error" | head -10
  ```
  Expected: no output (no errors).

- [ ] 5. Commit the verification milestone:
  ```
  git commit --allow-empty -m "chore(motion): phase 2 complete — motion param threaded, all tests green"
  ```



## Phase 3: Entry/Exit Physics (TDD)

### Task 1: Write failing entry/exit physics tests in `tui/pose.rs`

**Files:** Modify `crates/pixtuoid/src/tui/pose.rs`

- [ ] 1. Append the following test module extension inside `#[cfg(test)] mod tests { ... }` at the bottom of `crates/pixtuoid/src/tui/pose.rs`. These tests call `derive_with_routing` with the new `motion` parameter (introduced by Phase 2). All five tests must **FAIL** before the implementation in Task 3 because the entry/exit branches are not wired yet.

```rust
    // ---- Phase 3: entry/exit physics tests --------------------------------
    // These live alongside the existing snap_back_* tests.
    // Requires: physics::walk_profile, motion::MotionState (Phase 0-2 outputs).

    use crate::tui::motion::{MotionState, WanderPhase};
    use pixtuoid_core::physics::{walk_profile, WalkIntent};
    use std::collections::HashMap;

    fn make_motion_map() -> HashMap<AgentId, MotionState> {
        HashMap::new()
    }

    /// Build an entry slot (Idle, just created). desk_index 0 = nearest desk.
    fn entry_slot_near(created_at: SystemTime) -> AgentSlot {
        let mut s = active_slot(created_at, created_at);
        s.state = pixtuoid_core::state::ActivityState::Idle;
        s.desk_index = 0;
        s
    }

    /// Build an entry slot for a far desk index.
    fn entry_slot_far(created_at: SystemTime, desk_index: usize) -> AgentSlot {
        let mut s = entry_slot_near(created_at);
        s.desk_index = desk_index;
        // Give each far slot a distinct agent_id so speed_mult differs.
        s.agent_id = AgentId::from_transcript_path(&format!("/far/{desk_index}.jsonl"));
        s
    }

    /// Build an exiting slot: state_started_at from long ago, exiting_at = now.
    fn exiting_slot(exiting_at: SystemTime, created_at: SystemTime) -> AgentSlot {
        let mut s = active_slot(exiting_at - Duration::from_secs(30), created_at);
        s.exiting_at = Some(exiting_at);
        s.agent_id = AgentId::from_transcript_path("/exit/slot.jsonl");
        s
    }

    #[test]
    fn entry_duration_scales_with_path_longer_desk_takes_longer() {
        // Near desk (index 0) and far desk (highest index in layout) must
        // produce different physics durations. The far desk's WalkProfile
        // must have a strictly larger duration_ms.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout(); // 120×96, 4 desks
        let near = entry_slot_near(now); // desk 0
        let max_desk = l.home_desks.len().saturating_sub(1);
        let far = entry_slot_far(now, max_desk);

        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();

        // Two separate motion maps — each agent's first call snapshots its own profile.
        let mut motion_near: HashMap<AgentId, MotionState> = make_motion_map();
        let mut motion_far: HashMap<AgentId, MotionState> = make_motion_map();
        let mut hist_near = PoseHistory::new();
        let mut hist_far = PoseHistory::new();
        let mut router_n = StubRouter::straight();
        let mut router_f = StubRouter::straight();

        // First call: snapshots the entry profile.
        let _pn = derive_with_routing(
            &near, now, &l, &mut router_n, &overlay, &mut hist_near, &mut motion_near,
        );
        let _pf = derive_with_routing(
            &far, now, &l, &mut router_f, &overlay, &mut hist_far, &mut motion_far,
        );

        let dur_near = motion_near[&near.agent_id]
            .entry
            .as_ref()
            .expect("entry profile set for near desk")
            .1
            .duration_ms;
        let dur_far = motion_far[&far.agent_id]
            .entry
            .as_ref()
            .expect("entry profile set for far desk")
            .1
            .duration_ms;

        assert!(
            dur_far >= dur_near,
            "far desk duration {dur_far} must be >= near desk {dur_near}"
        );
    }

    #[test]
    fn nearer_desk_arrives_before_farther_desk() {
        // Same created_at, same StubRouter (straight-line). Run enough frames
        // so the near desk agent walk_arrived flips; the far desk must still
        // be Walking at that point.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let near = entry_slot_near(now);
        let max_desk = l.home_desks.len().saturating_sub(1);
        let far = entry_slot_far(now, max_desk);

        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut motion_near = make_motion_map();
        let mut motion_far = make_motion_map();
        let mut hist_near = PoseHistory::new();
        let mut hist_far = PoseHistory::new();
        let mut router_n = StubRouter::straight();
        let mut router_f = StubRouter::straight();

        // Snapshot on first call.
        let _ = derive_with_routing(
            &near, now, &l, &mut router_n, &overlay, &mut hist_near, &mut motion_near,
        );
        let _ = derive_with_routing(
            &far, now, &l, &mut router_f, &overlay, &mut hist_far, &mut motion_far,
        );

        // Advance time past the near desk's duration+pause but stay within
        // the far desk's window. Use the near desk's profile to compute exact time.
        let near_profile = motion_near[&near.agent_id]
            .entry
            .as_ref()
            .unwrap()
            .1
            .clone();
        // One ms past the near desk's full trip (duration + pause).
        let done_ms = near_profile.duration_ms + near_profile.pause_ms + 1;
        let t1 = now + Duration::from_millis(done_ms);

        let p_near = derive_with_routing(
            &near, t1, &l, &mut router_n, &overlay, &mut hist_near, &mut motion_near,
        );
        let p_far = derive_with_routing(
            &far, t1, &l, &mut router_f, &overlay, &mut hist_far, &mut motion_far,
        );

        assert!(
            !matches!(p_near, Some(Pose::Walking { .. })),
            "near desk must have arrived (no longer Walking), got {p_near:?}"
        );
        assert!(
            matches!(p_far, Some(Pose::Walking { .. })),
            "far desk must still be Walking, got {p_far:?}"
        );
    }

    #[test]
    fn five_same_created_at_agents_have_distinct_entry_durations() {
        // Speed_mult is per-agent-id → 5 distinct IDs must produce 5
        // distinct physics durations even for the same desk index, confirming
        // stagger.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();

        let ids: Vec<AgentId> = (0..5)
            .map(|i| AgentId::from_transcript_path(&format!("/stagger/{i}.jsonl")))
            .collect();

        let mut durations = Vec::new();
        for &id in &ids {
            let mut slot = entry_slot_near(now);
            slot.agent_id = id;
            let mut motion = make_motion_map();
            let mut hist = PoseHistory::new();
            let mut router = StubRouter::straight();
            let _ = derive_with_routing(
                &slot, now, &l, &mut router, &overlay, &mut hist, &mut motion,
            );
            let dur = motion[&id]
                .entry
                .as_ref()
                .expect("entry profile set")
                .1
                .duration_ms;
            durations.push(dur);
        }

        let unique: std::collections::HashSet<u64> = durations.iter().copied().collect();
        assert!(
            unique.len() >= 4,
            "expected ≥4 distinct durations among 5 agents, got {unique:?}"
        );
    }

    #[test]
    fn exit_profile_snapshotted_once_not_on_subsequent_calls() {
        // Second and third calls to derive_with_routing for an exiting agent
        // must NOT overwrite the profile's started_at — exit is commit-to-route.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let slot = exiting_slot(now, now - Duration::from_secs(60));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut motion = make_motion_map();
        let mut hist = PoseHistory::new();
        let mut router = StubRouter::straight();

        // First call: snapshot.
        let _ = derive_with_routing(
            &slot, now, &l, &mut router, &overlay, &mut hist, &mut motion,
        );
        let (started_at_1, _) = motion[&slot.agent_id]
            .exit
            .as_ref()
            .expect("exit profile set on first call")
            .clone();

        // Second call 100 ms later: must not re-snapshot.
        let t1 = now + Duration::from_millis(100);
        let _ = derive_with_routing(
            &slot, t1, &l, &mut router, &overlay, &mut hist, &mut motion,
        );
        let (started_at_2, _) = motion[&slot.agent_id]
            .exit
            .as_ref()
            .expect("exit profile still present")
            .clone();

        assert_eq!(
            started_at_1, started_at_2,
            "exit started_at must not change on subsequent calls"
        );
    }

    #[test]
    fn exit_uses_commute_speed_faster_than_wander() {
        // Exit profiles must use V_CRUISE_COMMUTE, not V_CRUISE_WANDER.
        // Proxy: compare v_cruise on the exit profile against the constant.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let slot = exiting_slot(now, now - Duration::from_secs(60));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut motion = make_motion_map();
        let mut hist = PoseHistory::new();
        let mut router = StubRouter::straight();

        let _ = derive_with_routing(
            &slot, now, &l, &mut router, &overlay, &mut hist, &mut motion,
        );
        let profile = &motion[&slot.agent_id]
            .exit
            .as_ref()
            .expect("exit profile set")
            .1;
        // v_cruise stored in WalkProfile is v_base * speed_mult — it must be
        // derived from V_CRUISE_COMMUTE (0.213), NOT V_CRUISE_WANDER (0.146).
        // The minimum possible commute v_cruise = 0.213 * 0.85 ≈ 0.181,
        // while the maximum wander v_cruise = 0.146 * 1.20 ≈ 0.175.
        // There's a gap: anything >= 0.176 is unambiguously commute.
        let min_commute = pixtuoid_core::physics::V_CRUISE_COMMUTE
            * pixtuoid_core::physics::SPEED_MULT_MIN;
        let max_wander =
            pixtuoid_core::physics::V_CRUISE_WANDER * pixtuoid_core::physics::SPEED_MULT_MAX;
        assert!(
            min_commute > max_wander,
            "test invariant: commute and wander speed ranges must not overlap"
        );
        assert!(
            profile.v_cruise >= min_commute * 0.99, // small f32 tolerance
            "exit v_cruise {:.4} must be in commute range (>= {min_commute:.4})",
            profile.v_cruise
        );
    }
```

- [ ] 2. Run the new tests — all five must **FAIL** (entry/exit branches not yet implemented):

```
cargo test -p pixtuoid --test pose 2>&1 | head -40
```

Expected: `FAILED` with `"entry profile set"` / `"exit profile set"` panics or missing-motion-param compile errors (if Phase 2 isn't complete yet, that's expected at compile time).

---

### Task 2: Add `door_anim_max_ms` to `FloorCtx` and update `compute_door_frame_idx` signature

**Files:** Modify `crates/pixtuoid/src/tui/floor.rs`, Modify `crates/pixtuoid/src/tui/pixel_painter/anchors.rs`

- [ ] 1. In `crates/pixtuoid/src/tui/floor.rs`, add the `door_anim_max_ms` field to `FloorCtx`:

```rust
/// Per-floor rendering state. Each floor gets its own pathfinder,
/// occupancy overlay, pose history, recolored-frame cache, lighting
/// fade state, motion map, and a cached max-in-flight walk duration for
/// the elevator door cosmetic.
pub struct FloorCtx {
    pub router: AStarRouter,
    pub overlay: OccupancyOverlay,
    pub history: PoseHistory,
    pub cache: FrameCache,
    pub light: LightingState,
    pub motion: std::collections::HashMap<pixtuoid_core::AgentId, crate::tui::motion::MotionState>,
    /// Per-frame max of all active entry/exit physics durations (ms).
    /// Written by `derive_with_routing` when it snapshots or checks an
    /// entry/exit profile; read by `compute_door_frame_idx` instead of the
    /// old `ENTRY_ANIMATION_MS` constant so the door open time matches the
    /// actual physics walk duration.
    pub door_anim_max_ms: u64,
}
```

- [ ] 2. Update `FloorCtx::new()` to initialize the new fields:

```rust
impl FloorCtx {
    pub fn new() -> Self {
        Self {
            router: AStarRouter::new(),
            overlay: OccupancyOverlay::new(),
            history: PoseHistory::new(),
            cache: FrameCache::new(),
            light: LightingState::new(),
            motion: std::collections::HashMap::new(),
            door_anim_max_ms: 0,
        }
    }
}
```

- [ ] 3. In `crates/pixtuoid/src/tui/pixel_painter/anchors.rs`, change `compute_door_frame_idx` to accept `door_anim_max_ms: u64` for the entry window, while the exit window continues using `EXIT_GRACE_WINDOW`. Full replacement of the function body (line 164–211):

```rust
/// Compute the elevator door frame (0=closed, 1=half, 2=open) from the
/// agents currently in flight. Stateless: each agent contributes a
/// per-frame value based on how far through their entry/exit window they
/// are; we take the MAX across all agents so the door is at least as open
/// as the most-in-progress agent needs.
///
/// `door_anim_max_ms` is the per-floor cached maximum entry/exit physics
/// duration (written by `derive_with_routing`). Falls back to
/// `ENTRY_ANIMATION_MS` if zero (e.g. when no entry is in flight).
pub(super) fn compute_door_frame_idx(
    agents: &[AgentSlot],
    now: SystemTime,
    door_anim_max_ms: u64,
) -> usize {
    fn frame_for_progress(elapsed_ms: u64, total_ms: u64) -> usize {
        if elapsed_ms < DOOR_TRANSITION_MS {
            if elapsed_ms < DOOR_TRANSITION_MS / 2 {
                1
            } else {
                2
            }
        } else if elapsed_ms + DOOR_TRANSITION_MS > total_ms {
            let remaining = total_ms.saturating_sub(elapsed_ms);
            if remaining < DOOR_TRANSITION_MS / 2 {
                0
            } else {
                1
            }
        } else {
            2
        }
    }

    // Use the physics-derived window when available; fall back to the
    // fixed constant so the door cosmetic still works before any entry
    // is in flight (door_anim_max_ms == 0 at frame 0).
    let entry_window_ms = if door_anim_max_ms > 0 {
        door_anim_max_ms
    } else {
        pose::ENTRY_ANIMATION_MS
    };

    let mut max_frame: usize = 0;
    for a in agents {
        if a.exiting_at.is_none() {
            if let Ok(d) = now.duration_since(a.created_at) {
                let ms = d.as_millis() as u64;
                if ms < entry_window_ms {
                    max_frame = max_frame.max(frame_for_progress(ms, entry_window_ms));
                }
            }
        }
        if let Some(exit_at) = a.exiting_at {
            if let Ok(d) = now.duration_since(exit_at) {
                let ms = d.as_millis() as u64;
                let exit_window_ms =
                    pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64;
                if ms < exit_window_ms {
                    max_frame = max_frame.max(frame_for_progress(ms, exit_window_ms));
                }
            }
        }
    }
    max_frame
}
```

- [ ] 4. Update the call site in `pixel_painter/mod.rs` (around line 757) to pass the new argument. Find the line `let frame_idx = compute_door_frame_idx(&agents, ctx.now);` and change it to:

```rust
        let frame_idx = compute_door_frame_idx(&agents, ctx.now, ctx.door_anim_max_ms);
```

- [ ] 5. Add `door_anim_max_ms: u64` to `PixelCtx` in `pixel_painter/mod.rs` (around line 70 in the struct):

```rust
pub struct PixelCtx<'a> {
    pub scene: &'a SceneState,
    pub layout: &'a Layout,
    pub pack: &'a Pack,
    pub now: SystemTime,
    pub buf: &'a mut RgbBuffer,
    pub cache: &'a mut FrameCache,
    pub router: &'a mut dyn Router,
    pub overlay: &'a mut OccupancyOverlay,
    pub history: &'a mut pose::PoseHistory,
    pub theme: &'a crate::tui::theme::Theme,
    pub floor: crate::tui::floor::FloorMeta,
    pub active_pet: Option<&'a crate::tui::renderer::PetState>,
    pub floor_pet_kind: Option<PetKind>,
    pub chitchat_state: &'a mut HashMap<(usize, usize), ActiveChitchat>,
    pub coffee_holders: &'a std::collections::HashSet<pixtuoid_core::AgentId>,
    pub coffee_fetched_at: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    pub coffee_stains: &'a HashMap<pixtuoid_core::AgentId, Vec<crate::tui::tui_renderer::StainPos>>,
    pub light: &'a mut crate::tui::floor::LightingState,
    /// Per-floor max in-flight entry/exit physics duration, for door cosmetics.
    pub door_anim_max_ms: u64,
}
```

- [ ] 6. Update the `render_to_rgb_buffer` call in `renderer.rs::draw_scene` (around line 217) to pass `door_anim_max_ms: 0`. This will be the real value in `tui_renderer.rs` — for `draw_scene` (called outside the floor path) use 0 as the safe default:

```rust
    let pixel_result = render_to_rgb_buffer(&mut PixelCtx {
        scene,
        layout: &layout,
        pack,
        now,
        buf: ctx.buf,
        cache: ctx.cache,
        router: ctx.router,
        overlay: ctx.overlay,
        history: ctx.history,
        theme,
        floor,
        active_pet: ctx.active_pet,
        floor_pet_kind: ctx.floor_pet_kind,
        chitchat_state: ctx.chitchat_state,
        coffee_holders: ctx.coffee_holders,
        coffee_fetched_at: ctx.coffee_fetched_at,
        coffee_stains: ctx.coffee_stains,
        light: ctx.light,
        door_anim_max_ms: 0,
    });
```

- [ ] 7. Update the existing door-frame tests in `pixel_painter/mod.rs` to pass the extra argument. Find the six `compute_door_frame_idx` calls in the test module and add `, pose::ENTRY_ANIMATION_MS` as the third argument so they exercise the fallback path:

```rust
    // In each test, change:
    //   compute_door_frame_idx(&[], now)
    // to:
    //   compute_door_frame_idx(&[], now, 0)
    // and:
    //   compute_door_frame_idx(&[slot], now)
    // to:
    //   compute_door_frame_idx(&[slot], now, 0)
    // etc.
```

Concretely, every occurrence of `compute_door_frame_idx(` in the test block gains `, 0)` as the last argument:

```rust
    #[test]
    fn door_frame_closed_when_no_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(compute_door_frame_idx(&[], now, 0), 0);
    }

    #[test]
    fn door_frame_just_spawned_is_half_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let slot = entry_slot(50, now);
        assert_eq!(compute_door_frame_idx(&[slot], now, 0), 1);
    }

    #[test]
    fn door_frame_after_opening_ramp_is_fully_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let s1 = entry_slot(150, now);
        assert_eq!(compute_door_frame_idx(&[s1], now, 0), 2);
        let s2 = entry_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[s2], now, 0), 2);
    }

    #[test]
    fn door_frame_closing_then_closed_at_end_of_entry() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let mid_close = entry_slot(pose::ENTRY_ANIMATION_MS - 150, now);
        assert_eq!(compute_door_frame_idx(&[mid_close], now, 0), 1);
        let near_end = entry_slot(pose::ENTRY_ANIMATION_MS - 50, now);
        assert_eq!(compute_door_frame_idx(&[near_end], now, 0), 0);
    }

    #[test]
    fn door_frame_expired_entry_contributes_nothing() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let old = entry_slot(pose::ENTRY_ANIMATION_MS + 1, now);
        assert_eq!(compute_door_frame_idx(&[old], now, 0), 0);
    }

    #[test]
    fn door_frame_exit_window_uses_4500ms_total() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let exiting = exit_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[exiting], now, 0), 2);
    }

    #[test]
    fn door_frame_takes_max_across_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let opening = entry_slot(50, now);
        let open = entry_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[opening, open], now, 0), 2);
    }
```

- [ ] 8. Verify compilation:

```
cargo build -p pixtuoid 2>&1 | head -40
```

Expected: **compiles** (no errors). The door tests still pass because `0` triggers the `ENTRY_ANIMATION_MS` fallback.

---

### Task 3: Implement entry/exit physics branches in `derive_with_routing`

**Files:** Modify `crates/pixtuoid/src/tui/pose.rs`

- [ ] 1. Add the required imports at the top of `pose.rs` (after the existing `use` block):

```rust
use std::collections::HashMap;

use pixtuoid_core::physics::{walk_profile, walk_arrived, walk_progress, WalkIntent};
use crate::tui::motion::{MotionState, WanderPhase, octile_path_len};
```

- [ ] 2. Promote `octile_distance` from private to `pub(in crate::tui)` (it is currently a private `fn`). Change the declaration on line 239:

```rust
pub(in crate::tui) fn octile_distance(a: Point, b: Point) -> u32 {
    let dx = (a.x as i32 - b.x as i32).unsigned_abs();
    let dy = (a.y as i32 - b.y as i32).unsigned_abs();
    14 * dx.min(dy) + 10 * (dx.max(dy) - dx.min(dy))
}
```

- [ ] 3. Change the signature of `derive_with_routing` (Phase 2 already added the param — if not, add it here). The complete new signature and full body replacing the current function:

```rust
/// Routed variant of `derive`. For Walking poses, asks `router` for an
/// A*-routed polyline and converts the global t (0..1000) into a
/// per-segment Walking pose so the character traces the path
/// corner-by-corner instead of cutting through obstacles.
///
/// `motion` drives entry/exit physics: on first sighting an entering or
/// exiting agent the A* path length is snapshotted into a `WalkProfile`
/// (commit-to-route); subsequent frames compute `t_x1000` from
/// `walk_progress` against the frozen profile.
pub fn derive_with_routing(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut PoseHistory,
    motion: &mut HashMap<AgentId, MotionState>,
) -> Option<Pose> {
    let desk = *layout.home_desks.get(slot.desk_index)?;

    // ---- EXIT branch -------------------------------------------------------
    // Takes priority over entry and state-driven poses.
    if let Some(exit_time) = slot.exiting_at {
        let door_target = layout.door_threshold?;
        let ms = slot.agent_id;

        let mstate = motion.entry(slot.agent_id).or_insert_with(|| {
            MotionState::new(slot.agent_id)
        });

        // Snapshot the exit profile on first sighting.
        if mstate.exit.is_none() {
            let from = Point {
                x: desk.x + 6,
                y: desk.y + 4,
            };
            let to_jittered = {
                let h = slot.agent_id.raw();
                let jx = ((h % 9) as i32 - 4) as i16;
                let jy = (((h >> 16) % 9) as i32 - 4) as i16;
                Point {
                    x: door_target.x.saturating_add_signed(jx),
                    y: door_target.y.saturating_add_signed(jy),
                }
            };
            let path = router.route(&layout.walkable, overlay, from, to_jittered);
            let path_len = octile_path_len(&path).max(1);
            let profile = walk_profile(path_len, WalkIntent::Exit, slot.agent_id);
            mstate.exit = Some((exit_time, profile));
        }

        let (started_at, ref profile) = *mstate.exit.as_ref()?;
        let elapsed_ms = now
            .duration_since(started_at)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;

        // GC: walk fully done including pause → return None so the slot
        // disappears (same as old ENTRY_ANIMATION_MS gate).
        if walk_arrived(profile, elapsed_ms) {
            return None;
        }

        let t_x1000 = walk_progress(profile, elapsed_ms);
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        let from = Point { x: desk.x + 6, y: desk.y + 4 };

        return Some(apply_polyline_routing(
            Pose::Walking { from, to: door_target, t_x1000, frame, carrying_coffee: false },
            slot, layout, router, overlay, history,
        ));
    }

    // ---- ENTRY branch ------------------------------------------------------
    // Gate: spawn window check reuses ENTRY_ANIMATION_MS only as a bound on
    // how long we try to route. Physics duration is the real walk time.
    let since_spawn = now
        .duration_since(slot.created_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    let door_from = layout.door_threshold;
    if let Some(door) = door_from {
        // Entry window: still within the routing window OR profile is active.
        let mstate = motion.entry(slot.agent_id).or_insert_with(|| {
            MotionState::new(slot.agent_id)
        });

        // Snapshot on first sighting if we're within the spawn window.
        if mstate.entry.is_none() && since_spawn < ENTRY_ANIMATION_MS {
            let to_desk = Point { x: desk.x + 6, y: desk.y + 4 };
            let to_jittered = {
                let h = slot.agent_id.raw();
                let jx = ((h % 9) as i32 - 4) as i16;
                let jy = (((h >> 16) % 9) as i32 - 4) as i16;
                Point {
                    x: to_desk.x.saturating_add_signed(jx),
                    y: to_desk.y.saturating_add_signed(jy),
                }
            };
            let path = router.route(&layout.walkable, overlay, door, to_jittered);
            let path_len = octile_path_len(&path).max(1);
            let profile = walk_profile(path_len, WalkIntent::Entry, slot.agent_id);
            mstate.entry = Some((slot.created_at, profile));
        }

        if let Some((started_at, ref profile)) = mstate.entry.clone() {
            let elapsed_ms = now
                .duration_since(started_at)
                .unwrap_or(Duration::ZERO)
                .as_millis() as u64;

            if !walk_arrived(profile, elapsed_ms) {
                let t_x1000 = walk_progress(profile, elapsed_ms);
                let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
                let to_desk = Point { x: desk.x + 6, y: desk.y + 4 };
                return Some(apply_polyline_routing(
                    Pose::Walking {
                        from: door,
                        to: to_desk,
                        t_x1000,
                        frame,
                        carrying_coffee: false,
                    },
                    slot, layout, router, overlay, history,
                ));
            }
            // walk_arrived — fall through to state-driven pose (near desks
            // arrive early, far desks still Walking here).
        }
    }

    // ---- STATE-DRIVEN pose -------------------------------------------------
    let raw = derive(slot, now, layout)?;

    // Snap-back override (unchanged from original).
    let desk_pose = matches!(
        raw,
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } | Pose::StandingAtDesk
    );
    let since_state = now
        .duration_since(slot.state_started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    let pose = if desk_pose && since_state < SNAP_BACK_MS {
        if let Some(prev) = history.recent(slot.agent_id, 300, now) {
            let dist =
                (prev.x as i32 - desk.x as i32).abs() + (prev.y as i32 - desk.y as i32).abs();
            if dist >= SNAP_BACK_MIN_DIST {
                let snap_target = Point { x: desk.x + 6, y: desk.y + 4 };
                let t = (since_state * 1000 / SNAP_BACK_MS).min(1000) as u16;
                let frame = ((since_state / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
                Pose::Walking {
                    from: prev,
                    to: snap_target,
                    t_x1000: t,
                    frame,
                    carrying_coffee: false,
                }
            } else {
                raw
            }
        } else {
            raw
        }
    } else {
        raw
    };

    Some(apply_polyline_routing(pose, slot, layout, router, overlay, history))
}
```

- [ ] 4. Extract the polyline-routing segment mapper into a private helper `apply_polyline_routing` so the three branches above share it. Add this function immediately after `derive_with_routing` in `pose.rs`:

```rust
/// Apply A*-based segment mapping to a Walking pose; for non-Walking poses
/// record waypoint/aimless positions to history and return as-is.
fn apply_polyline_routing(
    pose: Pose,
    slot: &AgentSlot,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut PoseHistory,
) -> Pose {
    let Pose::Walking { from, to, t_x1000, frame, carrying_coffee } = pose else {
        let pt = match &pose {
            Pose::AtWaypoint { wp, .. } => layout.waypoints.get(*wp).map(|w| w.pos),
            Pose::AimlessAt { dest } => Some(*dest),
            _ => None,
        };
        if let Some(p) = pt {
            // SAFETY: SystemTime::now() is fine here — this is a display helper,
            // not a physics timestamp. We don't have `now` in scope, so we use
            // the slot's last_event_at as a proxy (always recent enough for the
            // 300 ms history window).
            history.record(slot.agent_id, p, slot.last_event_at);
        }
        return pose;
    };

    let h = slot.agent_id.raw();
    let jx = ((h % 9) as i32 - 4) as i16;
    let jy = (((h >> 16) % 9) as i32 - 4) as i16;
    let to_jittered = Point {
        x: to.x.saturating_add_signed(jx),
        y: to.y.saturating_add_signed(jy),
    };
    let mut path = router.route(&layout.walkable, overlay, from, to_jittered);
    if let Some(last) = path.last_mut() {
        *last = to;
    }

    if path.len() <= 2 {
        history.record(slot.agent_id, walking_position(from, to, t_x1000), slot.last_event_at);
        return Pose::Walking { from, to, t_x1000, frame, carrying_coffee };
    }

    let mut leg_lens: Vec<u32> = Vec::with_capacity(path.len() - 1);
    for w in path.windows(2) {
        leg_lens.push(octile_distance(w[0], w[1]));
    }
    let total: u32 = leg_lens.iter().sum();
    if total == 0 {
        return Pose::Walking { from, to, t_x1000, frame, carrying_coffee };
    }
    let traveled = (t_x1000 as u32 * total) / 1000;
    let mut acc: u32 = 0;
    for (i, &leg) in leg_lens.iter().enumerate() {
        if acc + leg >= traveled {
            let into_leg = traveled - acc;
            let seg_t = (into_leg * 1000)
                .checked_div(leg)
                .map(|t| t.min(1000) as u16)
                .unwrap_or(1000);
            let cur_pos = walking_position(path[i], path[i + 1], seg_t);
            history.record(slot.agent_id, cur_pos, slot.last_event_at);
            return Pose::Walking {
                from: path[i],
                to: path[i + 1],
                t_x1000: seg_t,
                frame,
                carrying_coffee,
            };
        }
        acc += leg;
    }
    let last = path.len() - 1;
    history.record(slot.agent_id, path[last], slot.last_event_at);
    Pose::Walking {
        from: path[last - 1],
        to: path[last],
        t_x1000: 1000,
        frame,
        carrying_coffee,
    }
}
```

- [ ] 5. Add `MotionState::new` constructor to `motion.rs` (Phase 2 created the struct but may not have the constructor). Confirm the constructor exists or add it:

```rust
impl MotionState {
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            entry: None,
            exit: None,
            snap_back: None,
            wander_cycle_n: 0,
            wander_phase: WanderPhase::Seated,
            wander_phase_started_at: std::time::SystemTime::UNIX_EPOCH,
            wander_profile: None,
            wander_dest: crate::tui::layout::Point { x: 0, y: 0 },
            wander_dest_kind: None,
            wander_dest_wp_idx: None,
        }
    }
}
```

- [ ] 6. Run:

```
cargo build -p pixtuoid 2>&1 | head -60
```

Expected: **compiles**.

---

### Task 4: Update all `derive_with_routing` call sites to pass `motion`

**Files:** Modify `crates/pixtuoid/src/tui/pixel_painter/mod.rs`, `crates/pixtuoid/src/tui/pixel_painter/anchors.rs`, `crates/pixtuoid/src/tui/renderer.rs`, `crates/pixtuoid/src/tui/tui_renderer.rs`

- [ ] 1. In `pixel_painter/mod.rs`, `derive_with_routing` is called in two places: inside `seated_agents` map construction (~line 531) and inside the character loop (~line 841). Both calls need `ctx.motion` added. Change them:

```rust
    // seated_agents map (around line 531):
    let seated_agents: HashMap<usize, bool> = agents
        .iter()
        .filter(|a| a.desk_index < ctx.layout.home_desks.len() && a.exiting_at.is_none())
        .map(|a| {
            let p = pose::derive_with_routing(
                a,
                ctx.now,
                ctx.layout,
                ctx.router,
                ctx.overlay,
                ctx.history,
                ctx.motion,
            );
            let seated = matches!(p, Some(Pose::SeatedTyping { .. } | Pose::SeatedThinking));
            (a.desk_index, seated)
        })
        .collect();

    // character loop (around line 841):
        let Some(p) = pose::derive_with_routing(
            agent,
            ctx.now,
            ctx.layout,
            ctx.router,
            ctx.overlay,
            ctx.history,
            ctx.motion,
        ) else {
            continue;
        };
```

- [ ] 2. Add `motion` to `PixelCtx`:

```rust
    pub motion: &'a mut std::collections::HashMap<pixtuoid_core::AgentId, crate::tui::motion::MotionState>,
```

(This field was not yet added in Task 2 step 5 — add it now, after `light`.)

- [ ] 3. In `pixel_painter/anchors.rs`, `character_anchor` calls `derive_with_routing`. Add a `motion` parameter and thread it:

```rust
#[allow(clippy::too_many_arguments)]
pub(in crate::tui) fn character_anchor(
    agent: &AgentSlot,
    layout: &crate::tui::layout::Layout,
    now: SystemTime,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut pose::PoseHistory,
    motion: &mut std::collections::HashMap<pixtuoid_core::AgentId, crate::tui::motion::MotionState>,
) -> Option<Point> {
    let desk = *layout.home_desks.get(agent.desk_index)?;
    let pose = pose::derive_with_routing(agent, now, layout, router, overlay, history, motion)?;
    let anchor = match pose {
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } => seated_anchor(desk),
        Pose::StandingAtDesk => standing_at_desk_anchor(desk),
        Pose::AtWaypoint { wp, kind } => {
            let wp_obj = layout.waypoints.get(wp)?;
            match kind {
                WaypointKind::Couch => back_couch_anchor(wp_obj.pos),
                _ => waypoint_anchor(wp_obj.pos),
            }
        }
        Pose::AimlessAt { dest } => waypoint_anchor(dest),
        Pose::Walking { from, to, t_x1000, .. } => {
            walking_anchor(walking_position(from, to, t_x1000))
        }
    };
    Some(anchor)
}
```

- [ ] 4. Update all `character_anchor` call sites in `widgets/` and `hit_test.rs` to pass `motion`. Search:

```
cargo build -p pixtuoid 2>&1 | grep "character_anchor" | head -20
```

For each compiler error, add `motion` as the last argument. The callers in `renderer.rs::draw_scene` and `hit_test.rs::hit_test_agent` thread `ctx.history`; those same callers must now also thread `ctx.motion` (which Phase 2 added to `DrawCtx`). If `DrawCtx` does not yet have `motion`, add it now:

```rust
// In renderer.rs DrawCtx:
pub motion: &'a mut std::collections::HashMap<
    pixtuoid_core::AgentId,
    crate::tui::motion::MotionState,
>,
```

- [ ] 5. In `tui_renderer.rs`, where `PixelCtx` is constructed (the transition path and the main render path), add `motion: &mut fctx.motion` and `door_anim_max_ms: fctx.door_anim_max_ms`. For the transition helper `render_transition_floor`, add both fields. Find existing `PixelCtx {` constructions and add the two new fields.

- [ ] 6. After `derive_with_routing` writes the entry/exit profile, update `fctx.door_anim_max_ms`. In `tui_renderer.rs` render loop, after the `render_to_rgb_buffer` call, add (per floor):

```rust
        // Sync door_anim_max_ms from the motion map: max of all active
        // entry/exit profile durations so the elevator door cosmetic matches.
        let new_max = fctx.motion.values().fold(0u64, |acc, ms| {
            let entry_dur = ms.entry.as_ref().map(|(_, p)| p.duration_ms + p.pause_ms).unwrap_or(0);
            let exit_dur  = ms.exit.as_ref().map(|(_, p)| p.duration_ms + p.pause_ms).unwrap_or(0);
            acc.max(entry_dur).max(exit_dur)
        });
        fctx.door_anim_max_ms = new_max;
```

- [ ] 7. Add motion eviction to the existing coffee retain block in `tui_renderer.rs`. Find `coffee_stains.retain(|id, _| ...)` and add immediately after:

```rust
        fctx.motion.retain(|id, _| scene.agents.contains_key(id));
```

- [ ] 8. Verify workspace compiles:

```
cargo build --workspace 2>&1 | head -60
```

Expected: **compiles**.

---

### Task 5: Run the new tests — expect GREEN

- [ ] 1. Run the five new entry/exit tests:

```
cargo test -p pixtuoid -- entry_duration_scales_with_path entry_duration_scales_with_path_longer_desk nearer_desk_arrives five_same_created_at exit_profile_snapshotted exit_uses_commute 2>&1
```

Expected: all **PASS**.

- [ ] 2. Run the full snap_back test suite to confirm no regressions:

```
cargo test -p pixtuoid -- snap_back 2>&1
```

Expected: all four `snap_back_*` tests **PASS**.

- [ ] 3. Run the door-frame tests:

```
cargo test -p pixtuoid -- door_frame 2>&1
```

Expected: all door-frame tests **PASS** (they pass `0` for `door_anim_max_ms` which falls through to the `ENTRY_ANIMATION_MS` fallback).

- [ ] 4. Run the full workspace test suite:

```
cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -20
```

Expected: **test result: ok**.

---

### Task 6: Add `door_anim_max_ms` physics-driven door test

**Files:** Modify `crates/pixtuoid/src/tui/pixel_painter/anchors.rs` (test module)

- [ ] 1. Add one test that confirms `compute_door_frame_idx` uses a physics-derived window when `door_anim_max_ms > 0`. Append to the existing `door_frame_*` test block:

```rust
    #[test]
    fn door_frame_uses_physics_window_when_nonzero() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Slot spawned 3 s ago; with old ENTRY_ANIMATION_MS=4000 it would still
        // be mid-flight. Supply a short physics window (2500 ms) so it reads as
        // near the closing ramp instead.
        let short_window_ms: u64 = 2_500;
        // elapsed=3000, total=2500 → elapsed > total → door should be in closing
        // ramp or closed (remaining = 0 → frame 0).
        let slot = entry_slot(3_000, now);
        let frame = compute_door_frame_idx(&[slot], now, short_window_ms);
        assert_eq!(
            frame, 0,
            "with short physics window elapsed>total should yield closed door, got frame {frame}"
        );

        // Slot spawned 500 ms ago; physics window = 2500 ms → still well in the
        // middle (fully open frame = 2).
        let slot_mid = entry_slot(500, now);
        let frame_mid = compute_door_frame_idx(&[slot_mid], now, short_window_ms);
        assert_eq!(
            frame_mid, 2,
            "500ms into 2500ms window should be fully open, got frame {frame_mid}"
        );
    }
```

- [ ] 2. Run:

```
cargo test -p pixtuoid -- door_frame_uses_physics_window 2>&1
```

Expected: **PASS**.

---

### Task 7: Commit

- [ ] 1. Run preflight to confirm clean state:

```
cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -5
cargo clippy --workspace -- -D warnings 2>&1 | head -20
cargo fmt --all --check 2>&1
```

Expected: tests pass, clippy clean, fmt clean.

- [ ] 2. Stage and commit:

```
git add \
  crates/pixtuoid/src/tui/pose.rs \
  crates/pixtuoid/src/tui/floor.rs \
  crates/pixtuoid/src/tui/motion.rs \
  crates/pixtuoid/src/tui/pixel_painter/mod.rs \
  crates/pixtuoid/src/tui/pixel_painter/anchors.rs \
  crates/pixtuoid/src/tui/renderer.rs \
  crates/pixtuoid/src/tui/tui_renderer.rs

git commit -m "feat(motion): entry/exit physics — snapshot A* path, physics-driven t_x1000

- derive_with_routing gains entry/exit branches: first sighting routes
  door↔desk, snapshots octile_path_len, stores (anchor_time, WalkProfile).
- Per-frame: t_x1000 = walk_progress(profile, elapsed); walk_arrived
  gates fall-through (entry) / None GC (exit).
- compute_door_frame_idx accepts door_anim_max_ms; falls back to
  ENTRY_ANIMATION_MS when zero so door cosmetic is always correct.
- FloorCtx.door_anim_max_ms updated from motion map each render tick.
- Motion map evicted alongside coffee_holders in retain block.
- 5 new TDD tests: duration scales with path, nearer arrives first,
  5 distinct durations stagger, exit snapshotted once, exit commute speed.
- All snap_back_* and door_frame_* tests still green."
```



## Phase 4: Snap-back through physics

### Task 1: Write failing tests for physics-eased snap-back

**Files:**
- Modify `crates/pixtuoid/src/tui/pose.rs` (append inside `#[cfg(test)] mod tests`)

The four existing `snap_back_*` tests exercise gate logic; they must keep compiling and passing.
This task adds two new failing tests:
- `snap_back_uses_physics_progress_not_linear` — asserts the eased `t_x1000` at 25% of
  `duration_ms` is strictly less than 250 (accel ramp means we haven't reached 25% of the
  path yet at 25% of the time).
- `snap_back_profile_stored_in_motion_state` — asserts `MotionState.snap_back` is populated
  after the first call and reused (same `duration_ms`) on the second call.

The new signature of `derive_with_routing` gained `motion: &mut HashMap<AgentId, MotionState>`
in Phase 2. All existing tests call it without that param — in Phase 2 every call site was
updated. By this phase, all existing tests already pass the `motion` param.

- [ ] 1. In `crates/pixtuoid/src/tui/pose.rs`, inside `#[cfg(test)] mod tests { ... }`, add the
       two new test functions shown below (after the existing `pose_history_recent_expires` test):

```rust
#[test]
fn snap_back_uses_physics_progress_not_linear() {
    use std::collections::HashMap;
    use pixtuoid_core::physics::{walk_profile, walk_progress, WalkIntent};
    use crate::tui::motion::MotionState;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    let prev = Point { x: desk.x + 50, y: desk.y + 30 };

    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<pixtuoid_core::AgentId, MotionState> = HashMap::new();

    // First frame: state just flipped, snap-back profile is created.
    let _pose0 = derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history, &mut motion);
    let ms = motion.get(&slot.agent_id).expect("MotionState created");
    let (_, ref profile) = ms.snap_back.as_ref().expect("snap_back profile stored");
    let dur_ms = profile.duration_ms;
    assert!(dur_ms > 0, "profile duration must be > 0");

    // Advance time to 25% of profile duration.
    let quarter_time = now + Duration::from_millis(dur_ms / 4);
    let slot_quarter = active_slot(now, now - Duration::from_secs(60));
    // Observe the Walking pose at quarter-time.
    let mut history2 = PoseHistory::new();
    history2.record(slot_quarter.agent_id, prev, now - Duration::from_millis(50));
    let p = derive_with_routing(&slot_quarter, quarter_time, &l, &mut router, &overlay, &mut history2, &mut motion);

    match p {
        Some(Pose::Walking { t_x1000, .. }) => {
            // Physics accel ramp: at 25% of duration the agent has NOT
            // yet covered 25% of the path. Linear would give t_x1000=250.
            // The triangular/trapezoidal profile guarantees t_x1000 < 250
            // (accel phase, distance grows as t²).
            assert!(
                t_x1000 < 250,
                "physics ease-in: expected t_x1000 < 250 at 25% of duration, got {t_x1000}"
            );
        }
        other => panic!("expected Walking pose at 25% of snap-back duration, got {other:?}"),
    }
}

#[test]
fn snap_back_profile_stored_in_motion_state() {
    use std::collections::HashMap;
    use crate::tui::motion::MotionState;

    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let l = layout();
    let slot = active_slot(now, now - Duration::from_secs(60));
    let desk = l.home_desks[0];
    let prev = Point { x: desk.x + 50, y: desk.y + 30 };

    let mut history = PoseHistory::new();
    history.record(slot.agent_id, prev, now - Duration::from_millis(50));
    let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
    let mut router = StubRouter::straight();
    let mut motion: HashMap<pixtuoid_core::AgentId, MotionState> = HashMap::new();

    // Frame 1: creates the profile.
    let _p1 = derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history, &mut motion);
    let dur1 = motion.get(&slot.agent_id)
        .and_then(|ms| ms.snap_back.as_ref())
        .map(|(_, p)| p.duration_ms)
        .expect("profile created on frame 1");

    // Frame 2: 100ms later — profile must be REUSED (same duration_ms).
    let slot2 = active_slot(now, now - Duration::from_secs(60));
    let t2 = now + Duration::from_millis(100);
    let _p2 = derive_with_routing(&slot2, t2, &l, &mut router, &overlay, &mut history, &mut motion);
    let dur2 = motion.get(&slot2.agent_id)
        .and_then(|ms| ms.snap_back.as_ref())
        .map(|(_, p)| p.duration_ms)
        .expect("profile still present on frame 2");

    assert_eq!(dur1, dur2, "snap-back profile must be snapshotted once and reused");
}
```

- [ ] 2. Run the two new tests and confirm they FAIL (the impl still uses linear progress):

```
cargo test -p pixtuoid \
  'tui::pose::tests::snap_back_uses_physics_progress_not_linear' \
  'tui::pose::tests::snap_back_profile_stored_in_motion_state' \
  -- --nocapture 2>&1 | tail -20
```

Expected: `FAILED` (either compile error because the signature not yet updated, or assertion failure).

---

### Task 2: Implement physics-eased snap-back in `derive_with_routing`

**Files:**
- Modify `crates/pixtuoid/src/tui/pose.rs` (the snap-back override block, lines ~88–135)

Replace the linear `t = since_state * 1000 / SNAP_BACK_MS` computation with:

1. On first detection (`motion.snap_back` not yet set for this agent): compute `octile_path_len`
   for `[prev, snap_target]` (a two-point path), call `walk_profile(len, WalkIntent::SnapBack, id)`,
   store `(now, profile)` in `motion.entry(agent_id).or_default().snap_back`.
2. On subsequent frames: read the stored `(started_at, profile)`, compute
   `elapsed = now.duration_since(started_at)`, derive `t_x1000 = walk_progress(profile, elapsed_ms)`.
3. `walk_arrived(profile, elapsed_ms)` replaces the `since_state >= SNAP_BACK_MS` gate for
   clearing the snap-back. When arrived, remove `motion.snap_back` (set to `None`) and fall
   through to the raw desk pose.
4. The outer 900ms gate (`since_state < SNAP_BACK_MS`) stays as a hard wall so a stale
   `motion.snap_back` entry (from a previous snap-back cycle that didn't clear) can never replay.
5. The 8px distance gate stays unchanged.

The `frame` index is still computed from `elapsed_ms / WALKING_FRAME_MS`.

- [ ] 1. Update the snap-back override block in `derive_with_routing`. The complete new block
       (replacing lines 90–135 of the current file, after Phase 2/3 have already changed the
       function signature):

```rust
    let desk_pose = matches!(
        raw,
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } | Pose::StandingAtDesk
    );
    let since_state = now
        .duration_since(slot.state_started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    let pose = if desk_pose && since_state < SNAP_BACK_MS {
        if let Some(prev) = history.recent(slot.agent_id, 300, now) {
            let desk = *layout.home_desks.get(slot.desk_index)?;
            let dist =
                (prev.x as i32 - desk.x as i32).abs() + (prev.y as i32 - desk.y as i32).abs();
            if dist >= SNAP_BACK_MIN_DIST {
                let snap_target = Point {
                    x: desk.x + 6,
                    y: desk.y + 4,
                };
                // Retrieve or snapshot the physics profile for this snap-back.
                let ms_entry = motion
                    .entry(slot.agent_id)
                    .or_insert_with(|| crate::tui::motion::MotionState::new(slot.agent_id));
                let (started_at, profile) = ms_entry.snap_back.get_or_insert_with(|| {
                    let path = [prev, snap_target];
                    let len = crate::tui::motion::octile_path_len(&path);
                    let p = pixtuoid_core::physics::walk_profile(
                        len,
                        pixtuoid_core::physics::WalkIntent::SnapBack,
                        slot.agent_id,
                    );
                    (now, p)
                });
                let elapsed_ms = now
                    .duration_since(*started_at)
                    .unwrap_or(Duration::ZERO)
                    .as_millis() as u64;
                if pixtuoid_core::physics::walk_arrived(profile, elapsed_ms) {
                    // Physics says we've arrived + settled — clear the profile
                    // so the next state transition gets a fresh snapshot.
                    ms_entry.snap_back = None;
                    raw
                } else {
                    let t_x1000 = pixtuoid_core::physics::walk_progress(profile, elapsed_ms);
                    let frame =
                        ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
                    Pose::Walking {
                        from: prev,
                        to: snap_target,
                        t_x1000,
                        frame,
                        carrying_coffee: false,
                    }
                }
            } else {
                raw
            }
        } else {
            raw
        }
    } else {
        // Hard wall: clear any stale profile so the next snap-back starts fresh.
        if let Some(ms) = motion.get_mut(&slot.agent_id) {
            if ms.snap_back.is_some() {
                ms.snap_back = None;
            }
        }
        raw
    };
```

- [ ] 2. Verify that the four existing snap-back gate tests still pass and the two new tests now pass:

```
cargo test -p pixtuoid \
  'tui::pose::tests::snap_back' \
  -- --nocapture 2>&1 | tail -30
```

Expected output: all 6 `snap_back_*` tests show `ok`.

---

### Task 3: Run the full workspace test suite and confirm no regressions

**Files:** none (compile + test only)

- [ ] 1. Run the workspace tests:

```
cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -20
```

Expected: `test result: ok. N passed; 0 failed; ...`

- [ ] 2. If a compile error appears, it will be in a call site that wasn't updated in Phase 2 (e.g.,
       `character_anchor` in `pixel_painter/anchors.rs` which calls `derive_with_routing` with the
       old arity). Fix any such sites by passing the `motion` map already threaded through `PixelCtx`
       (Phase 2 added `pub motion: &'a mut HashMap<AgentId, MotionState>` to `PixelCtx`). No new
       logic — just forward the borrow.

       If `character_anchor` needs updating (it calls `derive_with_routing` internally), add the
       `motion` parameter to its signature too:

```rust
pub(in crate::tui) fn character_anchor(
    agent: &AgentSlot,
    layout: &crate::tui::layout::Layout,
    now: SystemTime,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut pose::PoseHistory,
    motion: &mut std::collections::HashMap<
        pixtuoid_core::AgentId,
        crate::tui::motion::MotionState,
    >,
) -> Option<Point> {
    let desk = *layout.home_desks.get(agent.desk_index)?;
    let pose = pose::derive_with_routing(agent, now, layout, router, overlay, history, motion)?;
    // rest of match unchanged
    let anchor = match pose {
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } => seated_anchor(desk),
        Pose::StandingAtDesk => standing_at_desk_anchor(desk),
        Pose::AtWaypoint { wp, kind } => {
            let wp_obj = layout.waypoints.get(wp)?;
            match kind {
                WaypointKind::Couch => back_couch_anchor(wp_obj.pos),
                _ => waypoint_anchor(wp_obj.pos),
            }
        }
        Pose::AimlessAt { dest } => waypoint_anchor(dest),
        Pose::Walking { from, to, t_x1000, .. } => {
            walking_anchor(walking_position(from, to, t_x1000))
        }
    };
    Some(anchor)
}
```

       Every call site of `character_anchor` (`widgets/tooltip.rs`, `hit_test.rs`,
       `pixel_painter/mod.rs` orchestrator) already threads a `motion` borrow from `PixelCtx`
       after Phase 2 — forward it.

- [ ] 3. Confirm zero warnings about unused imports in `physics.rs` use sites:

```
cargo clippy -p pixtuoid --features pixtuoid-core/test-renderer -- -D warnings 2>&1 | tail -20
```

Expected: `warning: ... 0 errors`.

---

### Task 4: Commit

**Files:** `crates/pixtuoid/src/tui/pose.rs`, `crates/pixtuoid/src/tui/pixel_painter/anchors.rs` (if touched in Task 3)

- [ ] 1. Stage changed files:

```
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  add crates/pixtuoid/src/tui/pose.rs \
      crates/pixtuoid/src/tui/pixel_painter/anchors.rs
```

- [ ] 2. Commit:

```
git -C /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics \
  commit -m "feat(motion): physics-eased snap-back via WalkProfile

Replace linear t = since_state/SNAP_BACK_MS with a SnapBack-intent
WalkProfile snapshotted on the first frame of each snap-back walk.
walk_progress() drives t_x1000; walk_arrived() gates the pose flip
back to desk. The 8 px and 900 ms outer guards are unchanged.

- All 4 existing snap_back_* gate tests pass.
- New test snap_back_uses_physics_progress_not_linear: at 25% of
  profile.duration_ms the eased t_x1000 < 250 (accel ramp, not linear).
- New test snap_back_profile_stored_in_motion_state: duration_ms
  identical on frame 1 and frame 2 (snapshot-once contract)."
```



## Phase 5: Cyclic elastic wander timeline (advance_wander)

### Task 1: Export phase-fraction constants + `pick_aimless_dest` from `pixtuoid-core`

**Files:**
- Modify `crates/pixtuoid-core/src/pose.rs` (lines 32–35 — make constants `pub const`; line 236 — make `pick_aimless_dest` `pub`)
- Modify `crates/pixtuoid/src/tui/pose.rs` (line 21 — extend the `pub use` list)

- [ ] 1. Open `crates/pixtuoid-core/src/pose.rs`. Change the three `const` phase-fraction declarations at lines 32–35 to `pub const`, and change `fn pick_aimless_dest` to `pub fn pick_aimless_dest`:

```rust
// crates/pixtuoid-core/src/pose.rs  — ONLY the changed declarations

/// Phase fractions of a cycle (×1000 to stay in integer math).
pub const PHASE_SEATED_FRAC: u64 = 250;      // 0..250/1000
pub const PHASE_WALK_OUT_FRAC: u64 = 417;    // 250..417/1000
pub const PHASE_AT_WAYPOINT_FRAC: u64 = 833; // 417..833/1000
                                              // walk-back is 833..1000/1000.
```

```rust
// crates/pixtuoid-core/src/pose.rs  — pick_aimless_dest visibility

/// Weighted-zone aimless-wander destination picker. `pub` so the TUI-side
/// `advance_wander` can mirror the same destination logic without
/// duplicating zone weights. Seed is `agent_id.raw() ^ cycle_n * MUL`.
pub fn pick_aimless_dest(layout: &SceneLayout, seed: u64) -> Point {
    let window_strip = Bounds {
        x: layout.cubicle_band.x,
        y: layout.top_margin + 1,
        width: layout.cubicle_band.width,
        height: 10,
    };
    let zones: [(Bounds, u16); 5] = [
        (window_strip, 30),
        (layout.pantry_room.unwrap_or(window_strip), 25),
        (layout.corridor.unwrap_or(layout.walkway), 20),
        (layout.cubicle_band, 15),
        (layout.meeting_room.unwrap_or(window_strip), 10),
    ];
    let total: u16 = zones.iter().map(|(_, w)| *w).sum();
    let mut roll = ((seed >> 32) as u16) % total.max(1);
    let zone = zones
        .iter()
        .find_map(|(b, w)| {
            if roll < *w {
                Some(b)
            } else {
                roll -= w;
                None
            }
        })
        .unwrap_or(&zones[0].0);
    for i in 0..32u64 {
        let h = seed
            .wrapping_add(i.wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .wrapping_mul(0xc6a4_a793_5bd1_e995);
        let x = zone.x + (h as u16) % zone.width.max(1);
        let y = zone.y + ((h >> 16) as u16) % zone.height.max(1);
        if layout.is_walkable(x, y) {
            return Point { x, y };
        }
    }
    let c = layout.corridor.unwrap_or(layout.walkway);
    let x_jitter = (seed as u16) % c.width.max(1);
    Point {
        x: c.x + x_jitter,
        y: c.y + c.height / 2,
    }
}
```

- [ ] 2. In `crates/pixtuoid/src/tui/pose.rs` extend the `pub use` block to re-export the three new constants and `pick_aimless_dest`:

```rust
pub use pixtuoid_core::pose::{
    cycle_ms_for, derive, is_aimless_cycle, personality_for, pick_aimless_dest, takes_trip,
    waypoint_index_for_cycle, Personality, Pose, ENTRY_ANIMATION_MS, PHASE_AT_WAYPOINT_FRAC,
    PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC, TYPING_FRAMES, TYPING_FRAME_MS, WALKING_FRAMES,
    WALKING_FRAME_MS, WANDER_CYCLE_BASE_MS, WANDER_CYCLE_RANGE_MS,
};
```

- [ ] 3. Verify it compiles (all existing tests still green):

```
cargo test -p pixtuoid-core 2>&1 | tail -5
# expected: test result: ok. N passed
```

---

### Task 2: Write failing tests for `advance_wander` — phase transitions

**Files:**
- Modify `crates/pixtuoid/src/tui/motion.rs` (add `#[cfg(test)] mod tests` block — assumes Phase 1/2 created the struct + enum)

These tests are written BEFORE any `advance_wander` implementation; they all fail with "not found" / compile errors until Task 4.

- [ ] 1. Add the test module at the bottom of `crates/pixtuoid/src/tui/motion.rs`. The tests use `advance_wander`, which does not exist yet — this is the RED step.

```rust
// Append to crates/pixtuoid/src/tui/motion.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::layout::Layout;
    use crate::tui::pathfind::Router;
    use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};
    use pixtuoid_core::{AgentId, AgentSlot};
    use pixtuoid_core::state::ActivityState;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    // -----------------------------------------------------------------------
    // Stub router: returns a straight two-point path always
    // -----------------------------------------------------------------------
    struct Straight;
    impl Router for Straight {
        fn route(
            &mut self,
            _: &WalkableMask,
            _: &OccupancyOverlay,
            from: Point,
            to: Point,
        ) -> Vec<Point> {
            vec![from, to]
        }
        fn invalidate(&mut self) {}
        fn set_preferred_zone(&mut self, _: Option<pixtuoid_core::layout::Bounds>) {}
    }

    // A stub router that returns a multi-hop polyline of a given octile length.
    // It always routes `from → mid → to` where mid is placed so the total
    // octile length equals `target_len` (within ±10 — straight legs).
    struct FixedLen {
        octile_len: u32,
    }
    impl Router for FixedLen {
        fn route(
            &mut self,
            _: &WalkableMask,
            _: &OccupancyOverlay,
            from: Point,
            to: Point,
        ) -> Vec<Point> {
            // Use a simple straight-line path but return from + extra waypoints
            // that sum to the requested length. For test simplicity we just
            // return a horizontal segment of the right length from `from`.
            let dx = (self.octile_len / 10) as u16;
            let mid = Point { x: from.x + dx / 2, y: from.y };
            let end = Point { x: from.x + dx, y: from.y };
            // Override the canonical `to` with our synthetic end for length
            // testing — the phase-transition logic we test here snapshots
            // the *length*, not the exact destination.
            let _ = to;
            vec![from, mid, end]
        }
        fn invalidate(&mut self) {}
        fn set_preferred_zone(&mut self, _: Option<pixtuoid_core::layout::Bounds>) {}
    }

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn idle_slot(id: &str, state_started: SystemTime) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path(id),
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: Arc::from("cc"),
            state: ActivityState::Idle,
            state_started_at: state_started,
            created_at: state_started - Duration::from_secs(90),
            last_event_at: state_started - Duration::from_secs(90),
            exiting_at: None,
            pending_idle_at: None,
            desk_index: 0,
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        }
    }

    fn layout() -> Layout {
        Layout::compute(120, 96, 4).expect("fits")
    }

    // -----------------------------------------------------------------------
    // T1: Fresh idle agent initialises into Seated phase
    // -----------------------------------------------------------------------
    #[test]
    fn fresh_idle_inits_to_seated_phase() {
        let now = t0();
        let slot = idle_slot("/p/a.jsonl", now);
        let l = layout();
        let overlay = OccupancyOverlay::new();
        let mut router = Straight;
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

        let ms = motion.get(&slot.agent_id).expect("state inserted");
        assert!(
            matches!(ms.wander_phase, WanderPhase::Seated),
            "fresh idle should init to Seated, got {:?}",
            ms.wander_phase
        );
        assert_eq!(ms.wander_cycle_n, 0);
    }

    // -----------------------------------------------------------------------
    // T2: Seated phase transitions to WalkingOut after dwell_ms elapses
    //     (only on a trip cycle — use cycle_n=0 and pick an id where
    //     takes_trip(id, 0) is true).
    // -----------------------------------------------------------------------
    #[test]
    fn seated_transitions_to_walking_out_on_trip_cycle() {
        use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};
        use pixtuoid_core::AgentId;

        // Find an agent where cycle_n=0 is a trip cycle.
        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/trip_{i}.jsonl")))
            .find(|id| takes_trip(*id, 0))
            .expect("should find a trip agent quickly");

        let now = t0();
        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

        let slot = idle_slot(&format!("/p/trip_{}.jsonl", id.raw()), now);
        // Construct a slot with the target id
        let mut slot = AgentSlot {
            agent_id: id,
            ..idle_slot("/dummy", now)
        };
        slot.state_started_at = now;

        let l = layout();
        let overlay = OccupancyOverlay::new();
        let mut router = Straight;
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        // Tick once to initialise at `now` (phase_started = now).
        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

        // Advance past the seated dwell.
        let later = now + Duration::from_millis(seated_dur + 50);
        advance_wander(&slot, later, &l, &mut router, &overlay, &mut motion);

        let ms = motion.get(&id).expect("state present");
        assert!(
            matches!(ms.wander_phase, WanderPhase::WalkingOut),
            "after seated dwell on trip cycle, expected WalkingOut, got {:?}",
            ms.wander_phase
        );
        assert!(ms.wander_profile.is_some(), "walk-out profile must be snapshotted");
    }

    // -----------------------------------------------------------------------
    // T3: Non-trip cycle stays Seated even after dwell elapsed
    // -----------------------------------------------------------------------
    #[test]
    fn non_trip_cycle_stays_seated() {
        use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};

        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/stay_{i}.jsonl")))
            .find(|id| !takes_trip(*id, 0))
            .expect("should find a stay-seated agent");

        let now = t0();
        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

        let slot = AgentSlot {
            agent_id: id,
            ..idle_slot("/dummy", now)
        };

        let l = layout();
        let overlay = OccupancyOverlay::new();
        let mut router = Straight;
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
        let later = now + Duration::from_millis(seated_dur + 200);
        advance_wander(&slot, later, &l, &mut router, &overlay, &mut motion);

        let ms = motion.get(&id).expect("state present");
        assert!(
            matches!(ms.wander_phase, WanderPhase::Seated),
            "non-trip cycle must stay Seated, got {:?}",
            ms.wander_phase
        );
    }

    // -----------------------------------------------------------------------
    // T4: WalkingOut transitions to AtWaypoint when walk_arrived fires
    // -----------------------------------------------------------------------
    #[test]
    fn walking_out_transitions_to_at_waypoint_on_arrival() {
        use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};
        use pixtuoid_core::physics::{walk_profile, WalkIntent};

        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/wp_{i}.jsonl")))
            .find(|id| takes_trip(*id, 0))
            .expect("find trip agent");

        let now = t0();
        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

        let slot = AgentSlot {
            agent_id: id,
            ..idle_slot("/dummy", now)
        };

        // Short synthetic path length so we don't need a multi-second wait.
        let short_len: u32 = 200;
        let profile = walk_profile(short_len, WalkIntent::WanderOut, id);
        let total_walk_ms = profile.duration_ms + profile.pause_ms;

        let l = layout();
        let overlay = OccupancyOverlay::new();
        // Use FixedLen so the snapshot measures exactly short_len.
        let mut router = FixedLen { octile_len: short_len };
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        // Initialise.
        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

        // Past seated dwell → should transition to WalkingOut.
        let t1 = now + Duration::from_millis(seated_dur + 10);
        advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

        // Confirm WalkingOut, then get the actual snapshotted profile.
        let snap_ms = {
            let ms = motion.get(&id).expect("state");
            assert!(matches!(ms.wander_phase, WanderPhase::WalkingOut));
            ms.wander_profile
                .as_ref()
                .map(|p| p.duration_ms + p.pause_ms)
                .expect("profile snapshotted")
        };

        // Advance past the walk arrival (use the actual snapshotted duration).
        let t2 = t1 + Duration::from_millis(snap_ms + 50);
        advance_wander(&slot, t2, &l, &mut router, &overlay, &mut motion);

        let ms = motion.get(&id).expect("state");
        assert!(
            matches!(ms.wander_phase, WanderPhase::AtWaypoint),
            "expected AtWaypoint after walk-out arrival, got {:?}",
            ms.wander_phase
        );
    }

    // -----------------------------------------------------------------------
    // T5: AtWaypoint dwell transitions to WalkingBack
    // -----------------------------------------------------------------------
    #[test]
    fn at_waypoint_transitions_to_walking_back_after_dwell() {
        use crate::tui::pose::{
            cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC,
            PHASE_WALK_OUT_FRAC,
        };
        use pixtuoid_core::physics::{walk_profile, WalkIntent};

        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/dwell_{i}.jsonl")))
            .find(|id| takes_trip(*id, 0))
            .expect("find trip agent");

        let now = t0();
        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
        let dwell_dur = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

        let slot = AgentSlot {
            agent_id: id,
            ..idle_slot("/dummy", now)
        };

        let short_len: u32 = 200;
        let profile = walk_profile(short_len, WalkIntent::WanderOut, id);
        let walk_ms = profile.duration_ms + profile.pause_ms;

        let l = layout();
        let overlay = OccupancyOverlay::new();
        let mut router = FixedLen { octile_len: short_len };
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

        // → WalkingOut
        let t1 = now + Duration::from_millis(seated_dur + 10);
        advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

        // → AtWaypoint
        let t2 = t1 + Duration::from_millis(walk_ms + 10);
        advance_wander(&slot, t2, &l, &mut router, &overlay, &mut motion);

        // → WalkingBack (past dwell)
        let t3 = t2 + Duration::from_millis(dwell_dur + 10);
        advance_wander(&slot, t3, &l, &mut router, &overlay, &mut motion);

        let ms = motion.get(&id).expect("state");
        assert!(
            matches!(ms.wander_phase, WanderPhase::WalkingBack),
            "expected WalkingBack after dwell, got {:?}",
            ms.wander_phase
        );
        assert!(ms.wander_profile.is_some(), "walk-back profile must be snapshotted");
    }

    // -----------------------------------------------------------------------
    // T6: WalkingBack arrival increments cycle_n and resets to Seated
    // -----------------------------------------------------------------------
    #[test]
    fn walking_back_arrival_increments_cycle_n_and_resets_to_seated() {
        use crate::tui::pose::{
            cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC,
            PHASE_WALK_OUT_FRAC,
        };
        use pixtuoid_core::physics::{walk_profile, WalkIntent};

        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/cyc_{i}.jsonl")))
            .find(|id| takes_trip(*id, 0))
            .expect("find trip agent");

        let now = t0();
        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
        let dwell_dur = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

        let slot = AgentSlot {
            agent_id: id,
            ..idle_slot("/dummy", now)
        };

        let short_len: u32 = 200;
        let out_profile = walk_profile(short_len, WalkIntent::WanderOut, id);
        let out_ms = out_profile.duration_ms + out_profile.pause_ms;
        let back_profile = walk_profile(short_len, WalkIntent::WanderBack, id);
        let back_ms = back_profile.duration_ms + back_profile.pause_ms;

        let l = layout();
        let overlay = OccupancyOverlay::new();
        let mut router = FixedLen { octile_len: short_len };
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        let mut t = now;
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

        t += Duration::from_millis(seated_dur + 10);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

        t += Duration::from_millis(out_ms + 10);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

        t += Duration::from_millis(dwell_dur + 10);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

        t += Duration::from_millis(back_ms + 10);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

        let ms = motion.get(&id).expect("state");
        assert!(
            matches!(ms.wander_phase, WanderPhase::Seated),
            "completed cycle must reset to Seated, got {:?}",
            ms.wander_phase
        );
        assert_eq!(ms.wander_cycle_n, 1, "cycle_n must increment once");
    }

    // -----------------------------------------------------------------------
    // T7: Dwell time is independent of path length
    //     Two agents at the same desk, same cycle phase — but different
    //     walk distances — should both dwell the same wall-clock milliseconds.
    // -----------------------------------------------------------------------
    #[test]
    fn dwell_time_independent_of_path_length() {
        use crate::tui::pose::{
            cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC,
            PHASE_WALK_OUT_FRAC,
        };
        use pixtuoid_core::physics::{walk_profile, WalkIntent};

        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/dwell2_{i}.jsonl")))
            .find(|id| takes_trip(*id, 0))
            .expect("find trip agent");

        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
        let expected_dwell = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

        let slot = AgentSlot {
            agent_id: id,
            ..idle_slot("/dummy", t0())
        };

        let l = layout();
        let overlay = OccupancyOverlay::new();

        // Test with two different path lengths — dwell should equal
        // `expected_dwell` regardless of the walk distance.
        for short_len in [150u32, 800u32] {
            let now = t0();
            let out_prof = walk_profile(short_len, WalkIntent::WanderOut, id);
            let out_ms = out_prof.duration_ms + out_prof.pause_ms;

            let mut router = FixedLen { octile_len: short_len };
            let mut motion: std::collections::HashMap<AgentId, MotionState> =
                std::collections::HashMap::new();

            let mut t = now;
            advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);
            t += Duration::from_millis(seated_dur + 10);
            advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);
            t += Duration::from_millis(out_ms + 10);
            advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

            // Record when we entered AtWaypoint.
            let at_wp_started = motion.get(&id).unwrap().wander_phase_started_at;

            // One ms before dwell ends: must still be AtWaypoint.
            let before_end = at_wp_started + Duration::from_millis(expected_dwell.saturating_sub(5));
            advance_wander(&slot, before_end, &l, &mut router, &overlay, &mut motion);
            assert!(
                matches!(motion.get(&id).unwrap().wander_phase, WanderPhase::AtWaypoint),
                "short_len={short_len}: still AtWaypoint 5ms before dwell ends"
            );

            // One ms after dwell ends: must be WalkingBack.
            let after_end = at_wp_started + Duration::from_millis(expected_dwell + 50);
            advance_wander(&slot, after_end, &l, &mut router, &overlay, &mut motion);
            assert!(
                matches!(motion.get(&id).unwrap().wander_phase, WanderPhase::WalkingBack),
                "short_len={short_len}: WalkingBack after dwell, expected_dwell={expected_dwell}ms"
            );
        }
    }

    // -----------------------------------------------------------------------
    // T8: Far waypoint makes the full cycle wall-time longer
    //     Walk legs differ; seated and dwell are identical in both cases.
    // -----------------------------------------------------------------------
    #[test]
    fn far_waypoint_full_cycle_is_longer() {
        use crate::tui::pose::{
            cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC,
            PHASE_WALK_OUT_FRAC,
        };
        use pixtuoid_core::physics::{walk_profile, WalkIntent};

        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/far_{i}.jsonl")))
            .find(|id| takes_trip(*id, 0))
            .expect("find trip agent");

        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
        let dwell_dur = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

        let cycle_wall_ms = |path_len: u32| -> u64 {
            let out = walk_profile(path_len, WalkIntent::WanderOut, id);
            let back = walk_profile(path_len, WalkIntent::WanderBack, id);
            seated_dur + (out.duration_ms + out.pause_ms) + dwell_dur
                + (back.duration_ms + back.pause_ms)
        };

        let near_ms = cycle_wall_ms(100);
        let far_ms = cycle_wall_ms(1200);

        assert!(
            far_ms > near_ms,
            "far cycle ({far_ms}ms) must be longer than near cycle ({near_ms}ms)"
        );

        // seated and dwell portions must be identical regardless of distance
        let out_near = walk_profile(100, WalkIntent::WanderOut, id);
        let out_far = walk_profile(1200, WalkIntent::WanderOut, id);

        // seated_dur doesn't change
        assert_eq!(
            cycle * PHASE_SEATED_FRAC / 1000,
            cycle * PHASE_SEATED_FRAC / 1000,
        );
        // dwell_dur doesn't change
        assert_eq!(
            cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000,
            cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000,
        );
        // Walk times DO differ.
        assert!(out_far.duration_ms > out_near.duration_ms, "far walk must take longer");
    }

    // -----------------------------------------------------------------------
    // T9: Arrival pause holds the Walking pose during [T, T+pause)
    //     walk_arrived returns false mid-pause; the Pose returned from
    //     derive_with_routing is Walking (t_x1000=1000) not a desk pose.
    // -----------------------------------------------------------------------
    #[test]
    fn arrival_pause_holds_walking_pose() {
        use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};
        use pixtuoid_core::physics::{pause_ms_for, walk_profile, walk_arrived, WalkIntent};

        let id = (0u64..500)
            .map(|i| AgentId::from_transcript_path(&format!("/p/pause_{i}.jsonl")))
            .find(|id| takes_trip(*id, 0))
            .expect("find trip agent");

        let now = t0();
        let cycle = cycle_ms_for(id);
        let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

        let slot = AgentSlot {
            agent_id: id,
            ..idle_slot("/dummy", now)
        };

        let short_len: u32 = 200;
        let profile = walk_profile(short_len, WalkIntent::WanderOut, id);
        // Pause starts at `profile.duration_ms`; agent is still Walking.
        let mid_pause_elapsed = profile.duration_ms + profile.pause_ms / 2;
        // walk_arrived is false mid-pause.
        assert!(
            !walk_arrived(&profile, mid_pause_elapsed),
            "walk_arrived must be false mid-pause"
        );

        // Now drive advance_wander to WalkingOut and check the phase does NOT
        // flip to AtWaypoint mid-pause.
        let l = layout();
        let overlay = OccupancyOverlay::new();
        let mut router = FixedLen { octile_len: short_len };
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

        let t1 = now + Duration::from_millis(seated_dur + 10);
        advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

        // Snapshot walk-out phase start.
        let out_started = motion.get(&id).unwrap().wander_phase_started_at;

        // Mid-pause: still WalkingOut (walk_arrived returns false).
        let mid = out_started + Duration::from_millis(mid_pause_elapsed);
        advance_wander(&slot, mid, &l, &mut router, &overlay, &mut motion);
        assert!(
            matches!(motion.get(&id).unwrap().wander_phase, WanderPhase::WalkingOut),
            "must stay WalkingOut during arrival pause"
        );
    }

    // -----------------------------------------------------------------------
    // T10: Bootstrap catch-up — agent idle for a long time before first render
    //      skips zero-walk seated/dwell cycles so cycle_n is approximately
    //      correct (within 2 of ideal for the given elapsed time).
    // -----------------------------------------------------------------------
    #[test]
    fn bootstrap_fast_forwards_cycle_n() {
        use crate::tui::pose::{
            cycle_ms_for, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC,
        };

        let id = AgentId::from_transcript_path("/p/bootstrap.jsonl");
        let now = t0();
        // Agent has been Idle for 10 full cycles before we first render.
        let cycle = cycle_ms_for(id);
        let state_started = now - Duration::from_millis(10 * cycle);
        let slot = idle_slot("/p/bootstrap.jsonl", state_started);

        let l = layout();
        let overlay = OccupancyOverlay::new();
        let mut router = Straight;
        let mut motion: std::collections::HashMap<AgentId, MotionState> =
            std::collections::HashMap::new();

        advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

        let ms = motion.get(&id).expect("state present");
        // The cycle_n should be approximately 10 (the jump approximation
        // accounts for seated+dwell fractions only, so exact equality isn't
        // guaranteed — allow ±2).
        let approx_cycles = ms.wander_cycle_n;
        assert!(
            (8..=12).contains(&approx_cycles),
            "bootstrap cycle_n={approx_cycles}, expected ~10 for 10-cycle idle"
        );
    }
}
```

- [ ] 2. Run tests (expect compile errors — `advance_wander` not yet defined):

```
cargo test -p pixtuoid --test-options '' 2>&1 | head -30
# expected: error[E0425]: cannot find function `advance_wander`
```

---

### Task 3: Verify `physics.rs` exports needed by tests are present

**Files:**
- Read `crates/pixtuoid-core/src/physics.rs` (created in Phase 0)

This is a verification-only step — Phase 0 should have created `physics.rs`. Confirm the exports `walk_profile`, `walk_arrived`, `pause_ms_for`, `WalkIntent`, `WalkProfile` exist.

- [ ] 1. Confirm `physics.rs` is registered in `lib.rs` (Phase 0 work):

```
grep -n "pub mod physics" crates/pixtuoid-core/src/lib.rs
# expected:  pub mod physics;
```

- [ ] 2. Confirm `walk_arrived` is exported:

```
grep -n "pub fn walk_arrived" crates/pixtuoid-core/src/physics.rs
# expected: match
```

If either check fails, the Phase 0 task did not complete — stop and fix Phase 0 first.

---

### Task 4: Implement `advance_wander` in `motion.rs`

**Files:**
- Modify `crates/pixtuoid/src/tui/motion.rs`

The function is added in `motion.rs` (Phase 1 created the file with `MotionState` / `WanderPhase` structs; Phase 2 threaded the param). This task adds the business logic.

- [ ] 1. Add imports and the `advance_wander` function to `motion.rs` (before the `#[cfg(test)]` block):

```rust
// Add to the top-level imports section of crates/pixtuoid/src/tui/motion.rs

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use pixtuoid_core::physics::{
    walk_arrived, walk_profile, WalkIntent,
};
use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::AgentId;

use crate::tui::layout::{Layout, Point, WaypointKind};
use crate::tui::pathfind::Router;
use crate::tui::pose::{
    cycle_ms_for, is_aimless_cycle, pick_aimless_dest, takes_trip, waypoint_index_for_cycle,
    PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC,
};
use pixtuoid_core::walkable::OccupancyOverlay;
```

```rust
// crates/pixtuoid/src/tui/motion.rs — advance_wander function

/// Advance the wander state machine by one frame for the given idle agent.
///
/// On first call for a fresh Idle agent (`wander_phase_started_at <
/// slot.state_started_at`), seeds the Seated phase anchored to
/// `state_started_at` and applies a bootstrap catch-up to compute an
/// approximate `cycle_n` so destination selection (takes_trip /
/// waypoint_index_for_cycle / is_aimless_cycle) is consistent with what
/// `core::derive` would have computed for an agent that was Idle before
/// the first render.
///
/// Returns the `t_x1000` progress value (0..=1000) for the current
/// walk leg (meaningful only in WalkingOut / WalkingBack phases), and
/// which phase the agent is in after the advance.
pub fn advance_wander(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    motion: &mut HashMap<AgentId, MotionState>,
) -> (WanderPhase, u16) {
    let id = slot.agent_id;
    let ms = motion.entry(id).or_insert_with(|| MotionState::new(id));

    // --- INIT / BOOTSTRAP ---------------------------------------------------
    // Detect a fresh Idle slot: either the MotionState was just created (cycle_n
    // == 0 AND phase_started is the epoch) OR the stored phase_started predates
    // the slot's state_started_at (the slot transitioned to Idle after we last
    // saw it in a different state).
    let is_fresh = ms.wander_phase_started_at
        < slot.state_started_at.checked_sub(Duration::from_millis(1)).unwrap_or(slot.state_started_at);

    if is_fresh {
        // Seed Seated, anchored to state_started_at.
        ms.wander_phase = WanderPhase::Seated;
        ms.wander_phase_started_at = slot.state_started_at;
        ms.wander_profile = None;
        ms.wander_cycle_n = 0;

        // Bootstrap catch-up: estimate how many full cycles elapsed between
        // state_started_at and now, assuming each cycle ≈ cycle_ms_for(id)
        // plus two short walk legs (approximated as zero since we only skip
        // seated+dwell phases — nil visual impact).
        let elapsed_idle = now
            .duration_since(slot.state_started_at)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        let cycle = cycle_ms_for(id);
        if elapsed_idle > cycle {
            // Jump approximation: count how many full seated+dwell periods fit.
            let seated_frac = PHASE_SEATED_FRAC; // per-1000 fraction
            let dwell_frac = PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC;
            let fixed_ms = cycle * (seated_frac + dwell_frac) / 1000;
            if fixed_ms > 0 {
                ms.wander_cycle_n = (elapsed_idle / cycle).saturating_sub(0);
            }
            // Re-anchor the phase start so the current frame is consistent.
            let cycles_elapsed = elapsed_idle / cycle;
            ms.wander_cycle_n = cycles_elapsed;
            // Place the phase start at the beginning of this partial cycle.
            let partial_ms = elapsed_idle % cycle;
            let phase_start = now
                .checked_sub(Duration::from_millis(partial_ms))
                .unwrap_or(slot.state_started_at);
            ms.wander_phase_started_at = phase_start;
        }
    }

    // --- PHASE MACHINE ------------------------------------------------------
    let elapsed_phase = now
        .duration_since(ms.wander_phase_started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    let cycle = cycle_ms_for(id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
    let dwell_dur = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

    match ms.wander_phase.clone() {
        // -----------------------------------------------------------------
        WanderPhase::Seated => {
            if elapsed_phase >= seated_dur {
                // Check if this cycle is a trip.
                if !takes_trip(id, ms.wander_cycle_n) || layout.waypoints.is_empty() {
                    // Non-trip: skip to next cycle's Seated without walking.
                    ms.wander_cycle_n += 1;
                    ms.wander_phase_started_at = ms
                        .wander_phase_started_at
                        .checked_add(Duration::from_millis(seated_dur))
                        .unwrap_or(now);
                    return (WanderPhase::Seated, 0);
                }

                // Trip: pick destination and snapshot walk-out profile.
                let (dest, dest_kind, wp_idx) = pick_wander_dest(id, ms.wander_cycle_n, layout);
                ms.wander_dest = dest;
                ms.wander_dest_kind = dest_kind;
                ms.wander_dest_wp_idx = wp_idx;

                // Route from desk to destination; snapshot octile length.
                let desk = layout.home_desks.get(slot.desk_index).copied().unwrap_or(dest);
                let path = router.route(&layout.walkable, overlay, desk, dest);
                let len = octile_path_len(&path).max(1);
                ms.wander_profile = Some(walk_profile(len, WalkIntent::WanderOut, id));

                ms.wander_phase = WanderPhase::WalkingOut;
                ms.wander_phase_started_at = ms
                    .wander_phase_started_at
                    .checked_add(Duration::from_millis(seated_dur))
                    .unwrap_or(now);
            }
            (ms.wander_phase.clone(), 0)
        }

        // -----------------------------------------------------------------
        WanderPhase::WalkingOut => {
            let profile = match &ms.wander_profile {
                Some(p) => p,
                None => return (WanderPhase::WalkingOut, 0),
            };
            let t_x1000 = pixtuoid_core::physics::walk_progress(profile, elapsed_phase);
            if walk_arrived(profile, elapsed_phase) {
                let walk_total = profile.duration_ms + profile.pause_ms;
                ms.wander_phase = WanderPhase::AtWaypoint;
                ms.wander_phase_started_at = ms
                    .wander_phase_started_at
                    .checked_add(Duration::from_millis(walk_total))
                    .unwrap_or(now);
                ms.wander_profile = None;
                return (WanderPhase::AtWaypoint, 1000);
            }
            (WanderPhase::WalkingOut, t_x1000)
        }

        // -----------------------------------------------------------------
        WanderPhase::AtWaypoint => {
            if elapsed_phase >= dwell_dur {
                // Snapshot walk-back profile (destination → desk).
                let desk = layout.home_desks.get(slot.desk_index).copied().unwrap_or(ms.wander_dest);
                let snap_to = Point { x: desk.x + 6, y: desk.y + 4 };
                let path = router.route(&layout.walkable, overlay, ms.wander_dest, snap_to);
                let len = octile_path_len(&path).max(1);
                ms.wander_profile = Some(walk_profile(len, WalkIntent::WanderBack, id));

                ms.wander_phase = WanderPhase::WalkingBack;
                ms.wander_phase_started_at = ms
                    .wander_phase_started_at
                    .checked_add(Duration::from_millis(dwell_dur))
                    .unwrap_or(now);
            }
            (ms.wander_phase.clone(), 0)
        }

        // -----------------------------------------------------------------
        WanderPhase::WalkingBack => {
            let profile = match &ms.wander_profile {
                Some(p) => p,
                None => return (WanderPhase::WalkingBack, 0),
            };
            let t_x1000 = pixtuoid_core::physics::walk_progress(profile, elapsed_phase);
            if walk_arrived(profile, elapsed_phase) {
                let walk_total = profile.duration_ms + profile.pause_ms;
                ms.wander_cycle_n += 1;
                ms.wander_phase = WanderPhase::Seated;
                ms.wander_phase_started_at = ms
                    .wander_phase_started_at
                    .checked_add(Duration::from_millis(walk_total))
                    .unwrap_or(now);
                ms.wander_profile = None;
                ms.wander_dest_kind = None;
                ms.wander_dest_wp_idx = None;
                return (WanderPhase::Seated, 0);
            }
            (WanderPhase::WalkingBack, t_x1000)
        }
    }
}

/// Pick the wander destination for a given agent and cycle.  Mirrors the
/// same logic as `core::pose::idle_pose` so `cycle_n` produces identical
/// destination choices in both the stateless core path and the stateful
/// tui path.
///
/// Returns `(dest_point, waypoint_kind, waypoint_index)`.
fn pick_wander_dest(
    id: AgentId,
    cycle_n: u64,
    layout: &Layout,
) -> (Point, Option<WaypointKind>, Option<usize>) {
    if is_aimless_cycle(id, cycle_n) {
        let seed = id.raw() ^ cycle_n.wrapping_mul(0xd1b5_4a32_d192_ed03);
        let p = pick_aimless_dest(&layout.inner, seed);
        (p, None, None)
    } else {
        let wp_idx = waypoint_index_for_cycle(id, cycle_n, layout.waypoints.len());
        let wp = layout.waypoints[wp_idx];
        (wp.pos, Some(wp.kind), Some(wp_idx))
    }
}
```

- [ ] 2. Ensure `MotionState::new` exists (it must be added to the struct impl if Phase 1 did not include it). Add to the struct impl block:

```rust
// In the MotionState impl block in motion.rs:

impl MotionState {
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            entry: None,
            exit: None,
            snap_back: None,
            wander_cycle_n: 0,
            wander_phase: WanderPhase::Seated,
            // Epoch sentinel — guaranteed < any real slot.state_started_at.
            wander_phase_started_at: SystemTime::UNIX_EPOCH,
            wander_profile: None,
            wander_dest: Point { x: 0, y: 0 },
            wander_dest_kind: None,
            wander_dest_wp_idx: None,
        }
    }
}
```

- [ ] 3. The `Layout` in the tui layer wraps a `SceneLayout`. Confirm that `layout.inner` (or the appropriate field) gives the `SceneLayout` needed by `pick_aimless_dest`. Read `crates/pixtuoid/src/tui/layout.rs` to find the field name; if it is `layout.scene` or a Deref, adjust the call. The call must pass the `SceneLayout` ref to `pick_aimless_dest` (defined in `pixtuoid_core::pose`).

```
grep -n "pub struct Layout" crates/pixtuoid/src/tui/layout.rs
grep -n "SceneLayout" crates/pixtuoid/src/tui/layout.rs | head -10
```

Adjust `pick_wander_dest` accordingly — if `Layout` newtype-wraps `SceneLayout`, use `&layout.0`; if it has a named field `scene_layout`, use `&layout.scene_layout`.

- [ ] 4. Run the tests added in Task 2 (expect RED — compile should succeed now but logic tests may fail):

```
cargo test -p pixtuoid tui::motion::tests 2>&1 | tail -30
# expected: some FAIL (tests are now finding the function but logic may not pass)
```

---

### Task 5: Fix test failures one by one (RED → GREEN)

**Files:**
- Modify `crates/pixtuoid/src/tui/motion.rs` (logic fixes only — no new tests)

Work through any failing tests from Task 4 in order. Common failure modes and their fixes:

- [ ] 1. **T1 `fresh_idle_inits_to_seated_phase` fails:** Bootstrap logic sets `wander_cycle_n = cycles_elapsed` even when it's 0 for a brand-new agent. Ensure the bootstrap path only fires when `elapsed_idle > cycle`. Check the `is_fresh` guard: the `wander_phase_started_at < state_started_at - 1ms` predicate on a brand-new `MotionState` (epoch) must be `true`, and `elapsed_idle` is effectively 0, so the bootstrap jump should leave `wander_cycle_n = 0`.

If the test fails because `wander_cycle_n != 0`, the fix is:

```rust
// Inside the is_fresh branch, replace the unconditional assignment:
if elapsed_idle > cycle {
    let cycles_elapsed = elapsed_idle / cycle;
    ms.wander_cycle_n = cycles_elapsed;
    let partial_ms = elapsed_idle % cycle;
    let phase_start = now
        .checked_sub(Duration::from_millis(partial_ms))
        .unwrap_or(slot.state_started_at);
    ms.wander_phase_started_at = phase_start;
} else {
    // elapsed < one cycle: start at Seated anchored to state_started_at.
    ms.wander_phase_started_at = slot.state_started_at;
    ms.wander_cycle_n = 0;
}
```

- [ ] 2. **T2 `seated_transitions_to_walking_out_on_trip_cycle` fails with wrong `agent_id`:** The test constructs an `AgentSlot` with `idle_slot("/dummy", now)` and then overwrites `agent_id`. Confirm the slot's `agent_id` matches `id`. Fix: build the slot directly with the right path matching `id.raw()`:

The test code in Task 2 already does `..idle_slot("/dummy", now)` with an override of `agent_id`. If `idle_slot` is a helper that re-derives the `AgentId` from its string argument, the override field wins — verify the struct update syntax is used, not a function that ignores the override. The `idle_slot` helper in Task 2 creates a slot via `AgentSlot { agent_id: id, .. }` which is correct — `agent_id` field is explicitly set.

- [ ] 3. **T4/T5/T6 fail because FixedLen router ignores `to`:** `advance_wander` calls `router.route(…, desk, dest)`. `FixedLen::route` ignores `to` and synthesises a horizontal path of length `octile_len` starting at `from`. This is intentional — the tests are probing phase transitions, not routing fidelity. Confirm the snapshotted profile in the motion state uses the path returned by `FixedLen`. If the length stored is ~`short_len` in the tests, the test should pass. If it's 0 or mismatched, trace to `octile_path_len` on the returned `[from, mid, end]` path.

- [ ] 4. **T9 `arrival_pause_holds_walking_pose` fails:** This test only calls `advance_wander` (not `derive_with_routing`), so it can only verify the phase stays `WalkingOut` — it cannot check the `Pose` variant directly. Confirm the assertion in the test already checks `WalkingOut` phase (not a `Pose`). If the test checks `Pose::Walking`, adjust to check `ms.wander_phase == WanderPhase::WalkingOut`.

- [ ] 5. Run the full test suite after fixes:

```
cargo test -p pixtuoid tui::motion::tests 2>&1 | tail -20
# expected: test result: ok. 10 passed; 0 failed
```

---

### Task 6: Wire `advance_wander` into `derive_with_routing` for idle wander

**Files:**
- Modify `crates/pixtuoid/src/tui/pose.rs` (the `derive_with_routing` body — Idle idle_pose branch)

This wires the physics-driven `t_x1000` from `advance_wander` into the existing polyline segment-mapper, replacing the fixed-fraction `t` from `idle_pose`. The `motion` parameter is already threaded (Phase 2).

- [ ] 1. In `derive_with_routing`, locate the call path for an Idle agent that `core::derive` mapped to a `Pose::Walking` from `idle_pose`. After the snap-back guard (which only fires for desk-bound poses), add the wander dispatch. The key insertion point is: when `raw` is `Pose::Walking { from, to, … }` AND the from/to indicate a wander leg (not an entry/exit leg), call `advance_wander` and substitute its `t_x1000`.

The cleanest approach: call `advance_wander` BEFORE `derive`, so we have the physics `t_x1000` ready when we compose the Walking pose. In `derive_with_routing`:

```rust
// crates/pixtuoid/src/tui/pose.rs — derive_with_routing body
// This is the NEW Idle wander dispatch inserted into the existing fn.
// Place this after the desk-guard and before the snap-back block.

use crate::tui::motion::{advance_wander, WanderPhase};
use pixtuoid_core::physics::WALKING_FRAME_MS; // already in scope via pub use

// Only for Idle agents that are past the entry window.
let since_spawn = now
    .duration_since(slot.created_at)
    .unwrap_or(Duration::ZERO)
    .as_millis() as u64;
let entry_done = since_spawn >= ENTRY_ANIMATION_MS;
let is_idle = matches!(slot.state, pixtuoid_core::state::ActivityState::Idle);

if is_idle && entry_done && slot.exiting_at.is_none() {
    // Advance per-phase clock; get physics-driven t_x1000.
    let (phase, t_phys) = advance_wander(slot, now, layout, router, overlay, motion);
    match phase {
        WanderPhase::WalkingOut => {
            let ms = motion.get(&slot.agent_id)?;
            let desk = *layout.home_desks.get(slot.desk_index)?;
            let dest = ms.wander_dest;
            let frame = (now
                .duration_since(ms.wander_phase_started_at)
                .unwrap_or(Duration::ZERO)
                .as_millis() as u64
                / WALKING_FRAME_MS as u64) as usize
                % WALKING_FRAMES;
            let pose = Pose::Walking {
                from: desk,
                to: dest,
                t_x1000: t_phys,
                frame,
                carrying_coffee: false,
            };
            // Feed through the polyline segment-mapper (re-use existing code
            // by falling through to the polyline block below with the
            // physics-derived t_x1000).
            return route_walking_pose(slot, now, layout, router, overlay, history, pose);
        }
        WanderPhase::AtWaypoint => {
            let ms = motion.get(&slot.agent_id)?;
            let pose = if let (Some(wp_idx), Some(kind)) =
                (ms.wander_dest_wp_idx, ms.wander_dest_kind)
            {
                Pose::AtWaypoint { wp: wp_idx, kind }
            } else {
                Pose::AimlessAt { dest: ms.wander_dest }
            };
            let pt = ms.wander_dest;
            history.record(slot.agent_id, pt, now);
            return Some(pose);
        }
        WanderPhase::WalkingBack => {
            let ms = motion.get(&slot.agent_id)?;
            let desk = *layout.home_desks.get(slot.desk_index)?;
            let snap_target = Point { x: desk.x + 6, y: desk.y + 4 };
            let elapsed_phase = now
                .duration_since(ms.wander_phase_started_at)
                .unwrap_or(Duration::ZERO)
                .as_millis() as u64;
            let frame = (elapsed_phase / WALKING_FRAME_MS as u64) as usize % WALKING_FRAMES;
            let carrying_coffee = ms.wander_dest_kind == Some(WaypointKind::Pantry);
            let pose = Pose::Walking {
                from: ms.wander_dest,
                to: snap_target,
                t_x1000: t_phys,
                frame,
                carrying_coffee,
            };
            return route_walking_pose(slot, now, layout, router, overlay, history, pose);
        }
        WanderPhase::Seated => {
            // Fall through to normal `derive` — it will return SeatedIdle
            // (or SeatedThinking) for this agent.
        }
    }
}
```

- [ ] 2. Extract the polyline segment-mapper into a named helper `route_walking_pose` to avoid duplication (the same mapper runs for entry/exit/snap-back in the existing code). This avoids a copy-paste of ~40 lines. The helper signature:

```rust
// Helper extracted from the bottom half of derive_with_routing.
// Runs the jitter → route → segment-map → history-record pipeline
// on an already-constructed Walking pose.
fn route_walking_pose(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut PoseHistory,
    pose: Pose,
) -> Option<Pose> {
    let Pose::Walking { from, to, t_x1000, frame, carrying_coffee } = pose else {
        return Some(pose);
    };
    let h = slot.agent_id.raw();
    let jx = ((h % 9) as i32 - 4) as i16;
    let jy = (((h >> 16) % 9) as i32 - 4) as i16;
    let to_jittered = Point {
        x: to.x.saturating_add_signed(jx),
        y: to.y.saturating_add_signed(jy),
    };
    let mut path = router.route(&layout.walkable, overlay, from, to_jittered);
    if let Some(last) = path.last_mut() {
        *last = to;
    }
    if path.len() <= 2 {
        history.record(slot.agent_id, walking_position(from, to, t_x1000), now);
        return Some(Pose::Walking { from, to, t_x1000, frame, carrying_coffee });
    }
    let mut leg_lens: Vec<u32> = Vec::with_capacity(path.len() - 1);
    for w in path.windows(2) {
        leg_lens.push(octile_distance(w[0], w[1]));
    }
    let total: u32 = leg_lens.iter().sum();
    if total == 0 {
        return Some(Pose::Walking { from, to, t_x1000, frame, carrying_coffee });
    }
    let traveled = (t_x1000 as u32 * total) / 1000;
    let mut acc: u32 = 0;
    for (i, &leg) in leg_lens.iter().enumerate() {
        if acc + leg >= traveled {
            let into_leg = traveled - acc;
            let seg_t = (into_leg * 1000)
                .checked_div(leg)
                .map(|t| t.min(1000) as u16)
                .unwrap_or(1000);
            let cur_pos = walking_position(path[i], path[i + 1], seg_t);
            history.record(slot.agent_id, cur_pos, now);
            return Some(Pose::Walking {
                from: path[i],
                to: path[i + 1],
                t_x1000: seg_t,
                frame,
                carrying_coffee,
            });
        }
        acc += leg;
    }
    let last = path.len() - 1;
    history.record(slot.agent_id, path[last], now);
    Some(Pose::Walking { from: path[last - 1], to: path[last], t_x1000: 1000, frame, carrying_coffee })
}
```

- [ ] 3. Update the existing bottom of `derive_with_routing` to call `route_walking_pose` instead of repeating the mapper inline (refactor, no behavior change for entry/snap-back paths):

```rust
// Replace the inline polyline block (lines ~171-231 in the original pose.rs)
// with a single call:
return route_walking_pose(slot, now, layout, router, overlay, history, pose);
```

- [ ] 4. Run all four existing snap-back tests to confirm no regression:

```
cargo test -p pixtuoid snap_back 2>&1 | tail -10
# expected: test result: ok. 4 passed; 0 failed
```

- [ ] 5. Run the full workspace test suite:

```
cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tail -20
# expected: test result: ok. (all pass)
```

---

### Task 7: Commit

**Files:** (all files touched in this phase)

- [ ] 1. Run `scripts/preflight.sh` (mirrors CI: fmt + machete + deny + clippy + tests):

```
./scripts/preflight.sh 2>&1 | tail -20
# expected: all checks pass
```

- [ ] 2. If `clippy` complains about unused variables in the bootstrap path or the `FixedLen` stub, fix them:

```rust
// Silence the let _ pattern on the stub's unused `to` arg:
fn route(&mut self, _: &WalkableMask, _: &OccupancyOverlay, from: Point, _to: Point) -> Vec<Point> {
```

- [ ] 3. Commit:

```
git add crates/pixtuoid-core/src/pose.rs \
        crates/pixtuoid/src/tui/pose.rs \
        crates/pixtuoid/src/tui/motion.rs

git commit -m "feat(motion): cyclic elastic wander timeline (advance_wander)

- Export PHASE_*_FRAC constants and pick_aimless_dest from core::pose
  so tui::motion can mirror identical destination selection logic.
- Implement advance_wander() with per-phase clock in MotionState:
  Seated + AtWaypoint use fixed-fraction dwell (cycle_ms_for);
  WalkingOut + WalkingBack snapshot A* path length at transition and
  drive t_x1000 via walk_profile / walk_progress / walk_arrived.
- Bootstrap catch-up: cycle_n jump approximation for agents idle
  before first render (nil visual impact, correct destination sync).
- Wire advance_wander into derive_with_routing: Idle wander phases
  route through the existing polyline segment-mapper with physics t.
- TDD: 10 tests cover all 4 phase transitions, dwell independence,
  far-waypoint duration ordering, arrival-pause hold, and bootstrap."
```



## Phase 6: Integration + visual verification

### Task 1: Full workspace test suite — green gate

**Files:** (none created or modified — read-only verification)

- [ ] 1. Run the full workspace test suite (with the `test-renderer` feature required by the `e2e.rs` integration test):

    ```
    cargo test --workspace --features pixtuoid-core/test-renderer 2>&1 | tee /tmp/walk-physics-tests.log
    ```

    Expected outcome: every test passes. The output ends with a line like:

    ```
    test result: ok. N passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
    ```

    If any test **fails** here, the failure is a regression introduced in an earlier phase. Stop, fix the failing test (in the phase that owns the touched file), and re-run before continuing.

- [ ] 2. Confirm that the two key physics tests from Phase 0 are among the passing tests — look for lines matching:

    ```
    test physics::tests::trapezoidal_duration_ms ... ok
    test physics::tests::triangular_duration_ms ... ok
    test physics::tests::cruise_plateau_constant_velocity ... ok
    test physics::tests::walk_arrived_false_during_pause_true_after ... ok
    ```

- [ ] 3. Confirm that the four existing snap-back tests from `tui/pose.rs` still pass — look for:

    ```
    test tui::pose::tests::snap_back_walks_from_history_when_state_just_flipped ... ok
    test tui::pose::tests::snap_back_skipped_when_prev_within_min_distance ... ok
    test tui::pose::tests::snap_back_skipped_after_900ms_window ... ok
    test tui::pose::tests::snap_back_skipped_without_recent_history ... ok
    ```

- [ ] 4. Confirm the motion-layer tui tests from Phase 5 are present and passing — look for lines containing `motion::tests::`.

---

### Task 2: Build release binary + snapshot example

**Files:** (build artifacts only — no source changes)

- [ ] 1. Build the full release workspace including the `snapshot` example:

    ```
    cargo build --release --workspace --example snapshot 2>&1 | tail -5
    ```

    Expected: finishes without errors. The last non-progress line should be:

    ```
    Compiling pixtuoid v0.x.x (...)
    Finished `release` profile [optimized] target(s) in ...
    ```

    The snapshot binary lands at `/Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/target/release/examples/snapshot`.

- [ ] 2. Render the standard verification snapshot (192×80, default 12-agent sample scene):

    ```
    /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/target/release/examples/snapshot \
        --cols 192 --rows 80 /tmp/snap-walk-physics.png
    ```

    Expected: exits 0, prints `wrote /tmp/snap-walk-physics.png` followed by the text-preview grid. The binary must not panic.

- [ ] 3. Render a second snapshot with the entry-walk stagger scenario — 5 agents all sharing the same `created_at` offset of 2000 ms (they are mid-entry-walk). Confirm the binary exits 0:

    ```
    /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/target/release/examples/snapshot \
        --cols 192 --rows 80 --max-desks 8 /tmp/snap-stagger.png
    ```

    Expected: exits 0 and writes `/tmp/snap-stagger.png`.

---

### Task 3: Visual inspection — confirm staggered arrivals + kinematics read

**Files:** (Python venv — must exist from prior work; if absent run the setup commands)

- [ ] 1. Ensure the Python venv is ready:

    ```
    cd /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics
    python3 -m venv .venv
    .venv/bin/pip install -r requirements-dev.txt --quiet
    ```

- [ ] 2. Crop the main snapshot into quadrants for inspection:

    ```
    .venv/bin/python3 /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/scripts/crop-snapshot.py \
        /tmp/snap-walk-physics.png --scale 3
    ```

    Expected output lists four files, e.g.:

    ```
      /tmp/snap-walk-physics_meeting.png  (NNN×NNN)
      /tmp/snap-walk-physics_pantry.png   (NNN×NNN)
      /tmp/snap-walk-physics_cubicle.png  (NNN×NNN)
      /tmp/snap-walk-physics_corridor.png (NNN×NNN)
    ```

- [ ] 3. Read each cropped PNG (use the Read tool on each path) and apply the self-critique checklist:

    **Stagger criterion (MUST PASS):** In the cubicle quadrant, agents that are mid-entry-walk should be visually at different distances from the door — not all clustered at the same position and not all seated at the same time. The sample scene sets varied `created_at` offsets (0 ms, 10 s, 5 s, 300 s, etc.), so agents in their entry window should appear at physics-correct positions.

    **Kinematics criterion (MUST PASS):** A freshly-spawned agent (< 200 ms into entry) should be close to the door/elevator; one 1-2 s in should be further along the corridor. The gap between them should be proportional to distance (near-desk agent may already be seated while far-desk agent is still walking).

    **No regression criterion (MUST PASS):** Seated agents at their desks, the pantry counter, couch, wall clock, elevator door, plants, and background lighting must all render normally — no missing sprites, no blank areas, no panic artifacts.

    **Acceleration criterion (QUALITATIVE):** If you render a GIF (optional, below), walkers should visibly start slow, speed up, and slow down again — not slide at constant speed across the floor.

    If any criterion fails, identify the root cause (likely a threading bug in an earlier phase's `motion` map integration) and fix it before continuing to Task 4.

- [ ] 4. (Optional but recommended) Render a 5-second GIF to confirm accel/decel reads in motion:

    ```
    /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/target/release/examples/snapshot \
        --cols 192 --rows 80 --gif --gif-duration 5 --gif-fps 10 /tmp/walk-physics.gif
    ```

    Open `/tmp/walk-physics.gif` in a viewer and observe:
    - Agents near the door reach their desks first (stagger visible).
    - Walk motion shows easing at start and end of each walk.
    - No agent teleports mid-walk.

---

### Task 4: Preflight + regenerate visual baseline + commit

**Files:**
- Modify `docs/images/screenshot.png` (replace with updated baseline)
- Modify `docs/images/gallery-cubicle.png` (update cubicle quadrant)

- [ ] 1. Run the full preflight script:

    ```
    cd /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics
    ./scripts/preflight.sh
    ```

    Expected: all three phase-1 checks pass (fmt, machete, deny); clippy passes with `-D warnings`; all workspace tests pass. Final line:

    ```
    [preflight] all checks passed
    ```

    If `cargo fmt --check` fails, run `cargo fmt --all` first. If clippy fails, fix the warning in the indicated file before continuing.

- [ ] 2. If the preflight revealed any `clippy` warnings from new code introduced in earlier phases (e.g. unused variable in `motion.rs`, dead_code on `WanderPhase`), fix them now. The pattern for suppressing intentional dead code during rollout:

    ```rust
    // In crates/pixtuoid/src/tui/motion.rs, if needed:
    #[allow(dead_code)]  // populated by advance_wander in phase 5; used by derive_with_routing
    pub wander_profile: Option<WalkProfile>,
    ```

    Re-run `./scripts/preflight.sh` after any fix until it exits 0.

- [ ] 3. Regenerate the visual baseline screenshots that are tracked in `docs/images/`:

    ```
    /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/target/release/examples/snapshot \
        --cols 192 --rows 80 /tmp/baseline.png

    .venv/bin/python3 /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/scripts/crop-snapshot.py \
        /tmp/baseline.png --scale 3

    cp /tmp/baseline_cubicle.png \
       /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/docs/images/gallery-cubicle.png

    cp /tmp/baseline.png \
       /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics/docs/images/screenshot.png
    ```

    Read the updated `docs/images/gallery-cubicle.png` and `docs/images/screenshot.png` with the Read tool to confirm they render correctly (non-empty, shows the office scene).

- [ ] 4. Stage all changes and commit:

    ```
    cd /Users/navepnow/Desktop/ascii-agent.nosync/.claude/worktrees/feat+walk-pace-physics
    git add \
      crates/pixtuoid-core/src/physics.rs \
      crates/pixtuoid-core/src/lib.rs \
      crates/pixtuoid/src/tui/motion.rs \
      crates/pixtuoid/src/tui/mod.rs \
      crates/pixtuoid/src/tui/pose.rs \
      crates/pixtuoid/src/tui/floor.rs \
      crates/pixtuoid/src/tui/renderer.rs \
      crates/pixtuoid/src/tui/tui_renderer.rs \
      crates/pixtuoid/src/tui/pixel_painter/mod.rs \
      crates/pixtuoid/src/tui/pixel_painter/anchors.rs \
      crates/pixtuoid/src/tui/hit_test.rs \
      crates/pixtuoid/src/tui/widgets/tooltip.rs \
      crates/pixtuoid/examples/snapshot.rs \
      docs/images/gallery-cubicle.png \
      docs/images/screenshot.png \
      CLAUDE.md

    git status
    ```

    Inspect `git status` — verify there are no unexpected untracked or modified files. Add any additional files from earlier phases that were missed.

- [ ] 5. Create the final integration commit:

    ```
    git commit -m "feat(physics): walk-pace physics — constant-velocity trapezoidal profile

    - pixtuoid-core: new physics.rs with WalkProfile, walk_profile/progress/arrived,
      speed_mult, pause_ms_for, calibrated constants (V_CRUISE_COMMUTE=0.213,
      V_CRUISE_WANDER=0.146, WALK_ACCEL=3.7e-4)
    - tui: motion.rs with MotionState, WanderPhase, octile_path_len
    - tui: FloorCtx gains motion HashMap + door_anim_max_ms
    - tui: derive_with_routing is now the motion-timing authority; entry/exit/
      snap-back/wander all use physics-driven t_x1000 off frozen WalkProfile
    - tui: advance_wander() elastic per-phase clock; wander_cycle_n preserved
    - tui: anchors.rs compute_door_frame_idx reads door_anim_max_ms
    - visual baseline regenerated: staggered arrivals + accel/decel visible

    Closes: walk-pace-physics feature
    Invariant check: pixtuoid-core has zero router/terminal deps (physics.rs
    imports only crate::AgentId). All 200+ tests green. Preflight clean."
    ```

    Do NOT `git push` — wait for explicit user confirmation per workflow rules.



## Phase 7: Docs — CLAUDE.md "Where to look" + module layout + ENTRY_ANIMATION_MS demotion comment

### Task 1: Add `ENTRY_ANIMATION_MS` demotion doc comment in `core/pose.rs`

**Files:**
- Modify `crates/pixtuoid-core/src/pose.rs` (line 45–46)

- [ ] 1. Open `crates/pixtuoid-core/src/pose.rs` and replace the existing doc comment on `ENTRY_ANIMATION_MS` (currently at line 44–46) with the updated version that demotes it to a non-routing fallback:

```rust
/// Spawn-window guard for entry routing in `tui::pose::derive_with_routing`.
/// After `physics::walk_profile` took over motion timing this constant is no
/// longer used to compute walk duration — it is only the *upper bound* on the
/// time window during which the tui layer will attempt to route an entry walk
/// and (via `FloorCtx::door_anim_max_ms`) drive door-open cosmetics. The
/// actual walk completes when `physics::walk_arrived` returns true.
pub const ENTRY_ANIMATION_MS: u64 = 4000;
```

- [ ] 2. Verify the workspace still compiles:

```
cargo build --workspace 2>&1 | tail -5
```

Expected: `Finished` with no errors.

---

### Task 2: Update `CLAUDE.md` — module layout + "Where to look" entries

**Files:**
- Modify `CLAUDE.md` (the project root copy, checked into the repo)

- [ ] 1. In the `## Layout` section, add `physics.rs` to the `pixtuoid-core` tree and `motion.rs` + the `FloorCtx` field note to the `pixtuoid` tui tree. Replace the existing block:

```
│   ├── pose.rs             pure state→pose derivation + wander state machine (no terminal deps)
```

with:

```
│   ├── physics.rs          pure walk-pace physics (no terminal/router deps): WalkIntent, WalkProfile,
│   │                       walk_profile (trapezoidal/triangular kinematics), walk_progress (t_x1000),
│   │                       walk_arrived, speed_mult, pause_ms_for; all constants (V_CRUISE_COMMUTE /
│   │                       V_CRUISE_WANDER / WALK_ACCEL / SPEED_MULT_MIN/MAX / PAUSE_MS_MIN/MAX)
│   ├── pose.rs             pure state→pose derivation + wander state machine (no terminal deps).
│   │                       ENTRY_ANIMATION_MS is demoted: not a duration knob, only the spawn-window
│   │                       upper bound for tui entry-routing and door-cosmetic gating.
```

- [ ] 2. In the same Layout block, update the `tui/` subtree. Replace:

```
│       ├── pose.rs         routed pose layer (PoseHistory, derive_with_routing, snap-back) — re-exports core::pose
│       ├── pathfind.rs     Router trait + AStarRouter with selective cache invalidation
```

with:

```
│       ├── motion.rs       per-agent walk-timing state: MotionState (entry/exit/snap_back/wander_* fields),
│       │                   WanderPhase enum, octile_path_len (reuses promoted octile_distance); owned
│       │                   as HashMap<AgentId, MotionState> on FloorCtx.motion; evicted alongside agent GC.
│       ├── pose.rs         routed pose + motion authority (PoseHistory, derive_with_routing, snap-back);
│       │                   derive_with_routing signature gains motion: &mut HashMap<AgentId, MotionState>;
│       │                   octile_distance promoted to pub(in crate::tui) for motion.rs reuse — re-exports core::pose
│       ├── pathfind.rs     Router trait + AStarRouter with selective cache invalidation
```

- [ ] 3. In the same Layout block, update the `tui/floor.rs` line. Replace:

```
│   └── tui/                ratatui App + TuiRenderer (Renderer trait impl)
```
(that line stays, but update the `FloorCtx` description). Find the line:

```
│       ├── hit_test.rs     mouse hit-test: agent hover, coffee machine click, furniture tooltips
│       ├── tui_renderer.rs Renderer trait impl — owns cross-frame state (RgbBuffer, FrameCache, Router, PoseHistory, TickerQueue, Theme, cached Layout)
```

and replace the `tui_renderer.rs` line with:

```
│       ├── hit_test.rs     mouse hit-test: agent hover, coffee machine click, furniture tooltips
│       ├── tui_renderer.rs Renderer trait impl — owns cross-frame state (RgbBuffer, FrameCache, Router, PoseHistory, TickerQueue, Theme, cached Layout); Vec<FloorCtx> each now carries .motion HashMap + .door_anim_max_ms
```

- [ ] 4. In the `## Where to look` section, update the existing entry for the pose system:

Find the line starting:
```
- "How is the office laid out?" → `core::layout::SceneLayout::compute_with_seed`...
```

and update the fragment `; \`tui::pose::derive_with_routing\` for the routed variant (A*-routed polylines + snap-back walks)` to:

```
; `tui::pose::derive_with_routing` for the routed variant — this is now the **motion-timing authority**: it snapshots A* path length once per walk-start into `tui::motion::MotionState`, freezes a `physics::WalkProfile`, and drives `t_x1000` per-frame via `physics::walk_progress` while re-routing path *shape* per frame; snap-back is similarly driven through a `WalkIntent::SnapBack` profile capped at `SNAP_BACK_MS`
```

- [ ] 5. After the existing "How do multi-floor offices work?" bullet, add a new "Where to look" bullet for the physics module and motion state:

```markdown
- "How does walk-pace physics work?" → `pixtuoid_core::physics` (pure, no terminal deps) — `WalkIntent` enum tags the walk kind (Entry/Exit/WanderOut/WanderBack/SnapBack); `walk_profile(path_len_octile, intent, agent_id)` returns a frozen `WalkProfile` with trapezoidal/triangular kinematics (triangular when path < `v²/a`); `walk_progress(p, elapsed_ms)` emits `t_x1000 ∈ [0,1000]`; `walk_arrived` gates the Walking→Seated/AtWaypoint flip including the per-agent arrival pause (`pause_ms_for`). Constants: `V_CRUISE_COMMUTE = 0.213` octile/ms (brisk, Entry/Exit/SnapBack), `V_CRUISE_WANDER = 0.146` octile/ms (amble, Wander legs), `WALK_ACCEL = 3.7e-4` octile/ms². Per-agent personality: `speed_mult` (bits 24..34, range 0.85–1.20), `pause_ms_for` (bits 40..52, range 200–400ms) — disjoint bit ranges from `cycle_ms_for`/`personality_for`.
- "How does per-agent motion state work?" → `tui::motion::MotionState` (in `crates/pixtuoid/src/tui/motion.rs`) holds all per-agent walk-timing: `entry`/`exit` optional `(SystemTime, WalkProfile)` pairs (snapshotted once at walk-start; shape re-routes per frame but duration is frozen); `snap_back` optional `(SystemTime, WalkProfile, Point)`; and the elastic wander timeline — `wander_phase` (`WanderPhase` enum: Seated/WalkingOut/AtWaypoint/WalkingBack`), `wander_phase_started_at`, `wander_profile`, `wander_cycle_n`. Each `FloorCtx` owns `pub motion: HashMap<AgentId, MotionState>` and `pub door_anim_max_ms: u64` (replaces the hardcoded `ENTRY_ANIMATION_MS` in door cosmetics). Evicted via `fctx.motion.retain(|id,_| scene.agents.contains_key(id))` in the GC block in `tui_renderer.rs`. `octile_path_len(&[Point]) -> u32` in `motion.rs` sums per-segment `octile_distance` for the snapshotted A* path.
- "What is the elastic wander timeline?" → `advance_wander()` inside `tui/pose.rs` drives the cyclic wander via explicit per-phase clocks anchored to `wander_phase_started_at` rather than a global cycle modulo. Walk legs (WalkingOut / WalkingBack) use `physics::walk_profile` and are variable-length; Seated / AtWaypoint dwell phases keep the fixed-fraction knobs from `core::pose`. The cycle is **elastic** — total length varies with path length, but each phase is self-contained. Destination selection (`takes_trip` / `is_aimless_cycle` / `waypoint_index_for_cycle` / `pick_aimless_dest`) is unchanged. Bootstrap: fresh Idle seeded at Seated anchored to `state_started_at`; long-idle agents fast-forward `cycle_n` (only seated/dwell phases skipped, zero visual impact).
```

- [ ] 6. Verify no typos broke the Markdown — check the file compiles (it's CLAUDE.md, not Rust, so just visually inspect the bullet list indentation):

```
grep -n "How does walk-pace physics" CLAUDE.md
grep -n "motion.rs" CLAUDE.md
grep -n "physics.rs" CLAUDE.md
```

Expected: each grep returns at least one hit.

---

### Task 3: Commit the docs

**Files:**
- `crates/pixtuoid-core/src/pose.rs`
- `CLAUDE.md`

- [ ] 1. Stage both files and confirm only the expected files are in the diff:

```
git diff --stat HEAD
```

Expected output includes `CLAUDE.md` and `crates/pixtuoid-core/src/pose.rs`, nothing else.

- [ ] 2. Run the format check to make sure the Rust file is clean:

```
cargo fmt --all --check
```

Expected: exits 0 (no output).

- [ ] 3. Commit:

```
git add CLAUDE.md crates/pixtuoid-core/src/pose.rs
git commit -m "docs: document walk-pace physics module, MotionState, elastic wander timeline

- physics.rs added to core layout map (pure, no terminal deps)
- motion.rs + FloorCtx.motion / door_anim_max_ms added to tui layout map
- derive_with_routing described as motion-timing authority (frozen WalkProfile,
  live path shape, physics-driven t_x1000)
- Three new 'Where to look' bullets: physics constants, MotionState lifecycle,
  elastic wander timeline
- ENTRY_ANIMATION_MS doc comment demoted: spawn-window gate only, not a
  duration knob — actual timing now owned by walk_profile / walk_arrived"
```

Expected: commit succeeds, no pre-commit hook failures.

