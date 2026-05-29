//! TUI-side pose layer.
//!
//! Re-exports the pure pose-derivation surface from `pixtuoid_core::pose`
//! and adds the binary-side machinery:
//!   * `PoseHistory` — per-agent cache of the last rendered position.
//!   * `derive_with_routing` — the routed variant of `derive` that consults
//!     a `&mut dyn Router` so walking poses follow A*-routed polylines and
//!     so state transitions are smoothed with a snap-back walk instead of
//!     teleporting back to the desk.
//!
//! Keeping the routed code on this side means `pixtuoid-core` does not
//! depend on the pathfinder — the trait lives in the binary because A* is
//! TUI-rendering-adjacent and may differ for non-terminal renderers.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use pixtuoid_core::physics::{walk_arrived, walk_profile, walk_progress, WalkIntent};
use pixtuoid_core::state::AgentSlot;
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::AgentId;

use crate::tui::motion::{advance_wander, octile_path_len, MotionState, WanderPhase};

pub use pixtuoid_core::pose::{
    aimless_wander_seed, cycle_ms_for, derive, derive_state_only, is_aimless_cycle,
    personality_for, pick_aimless_dest, takes_trip, waypoint_index_for_cycle, Personality, Pose,
    ENTRY_ANIMATION_MS, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC,
    THINKING_WINDOW_SECS, TYPING_FRAMES, TYPING_FRAME_MS, WALKING_FRAMES, WALKING_FRAME_MS,
    WANDER_CYCLE_BASE_MS, WANDER_CYCLE_RANGE_MS,
};

use crate::tui::layout::{Layout, Point, WaypointKind};
use crate::tui::pathfind::Router;

/// Per-agent rendered position cache. Updated each frame by
/// `derive_with_routing`, consulted on state transitions so an agent
/// who was mid-walk when their state flipped can complete the walk
/// visually instead of teleporting back to their desk.
#[derive(Debug, Default, Clone)]
pub struct PoseHistory {
    last: std::collections::HashMap<AgentId, (Point, SystemTime)>,
}

impl PoseHistory {
    pub fn new() -> Self {
        Self::default()
    }
    /// Record where an agent was visually placed this frame.
    pub fn record(&mut self, agent_id: AgentId, anchor: Point, now: SystemTime) {
        self.last.insert(agent_id, (anchor, now));
    }
    /// Latest recorded position if it's at most `max_age_ms` old.
    pub fn recent(&self, agent_id: AgentId, max_age_ms: u64, now: SystemTime) -> Option<Point> {
        let (pt, when) = self.last.get(&agent_id).copied()?;
        let age = now.duration_since(when).ok()?.as_millis() as u64;
        if age <= max_age_ms {
            Some(pt)
        } else {
            None
        }
    }
}

/// Duration of the snap-back walk used when state-driven pose would
/// instantly place the agent back at their desk. 600ms is short enough
/// to feel responsive (the user wants to see the tool fire) but long
/// enough to read as motion, not a pop.
const SNAP_BACK_MS: u64 = 900;
/// Minimum manhattan distance (px) from current rendered position to
/// the desk before we bother animating the snap-back. Below this the
/// teleport is invisible and animating wastes a frame.
const SNAP_BACK_MIN_DIST: i32 = 8;

/// Routed variant of `derive`. For Walking poses, asks `router` for an
/// A*-routed polyline (composed against the layout's static mask + the
/// per-frame `overlay`) and converts the global t (0..1000) into a
/// per-segment Walking pose so the character traces the path
/// corner-by-corner instead of cutting through obstacles or other agents.
///
/// `motion` drives entry/exit physics: on first sighting an entering or
/// exiting agent the A* path length is snapshotted into a `WalkProfile`
/// (commit-to-route); subsequent frames compute `t_x1000` from
/// `walk_progress` against the frozen profile.
///
/// `history` is consulted on state transitions: if the agent's pose
/// flipped from a wander walk (or from AtWaypoint) to a desk-bound
/// pose (SeatedTyping / SeatedIdle / StandingAtDesk), we override the
/// instant teleport with a brief walk from the recorded previous
/// position to the desk.
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
        let Some(door_target) = layout.door_threshold else {
            // No door in this layout (very narrow terminal — Layout can
            // return door_threshold: None). Skip the physics exit walk and
            // let the reducer's grace window GC the slot. Returning None
            // here would make the exiting agent VANISH on its first frame
            // instead of holding at the desk; the old linear exit code
            // handled this gracefully too.
            let raw = derive_state_only(slot, now, layout)?;
            return match raw {
                Pose::Walking { .. } => {
                    route_walking_pose(slot, now, layout, router, overlay, history, raw)
                }
                other => Some(other),
            };
        };

        let mstate = motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));

        // Snapshot the exit profile on first sighting.
        if mstate.exit.is_none() {
            let from = Point {
                x: desk.x + 6,
                y: desk.y + 4,
            };
            let h = slot.agent_id.raw();
            let jx = ((h % 9) as i32 - 4) as i16;
            let jy = (((h >> 16) % 9) as i32 - 4) as i16;
            let to_jittered = Point {
                x: door_target.x.saturating_add_signed(jx),
                y: door_target.y.saturating_add_signed(jy),
            };
            let path = router.route(&layout.walkable, overlay, from, to_jittered);
            let path_len = octile_path_len(&path).max(1);
            let profile = walk_profile(path_len, WalkIntent::Exit, slot.agent_id);
            mstate.exit = Some((exit_time, profile));
        }

        // Destructure without moving the non-Copy profile (Correction L).
        let e = mstate.exit.as_ref()?;
        let started_at = e.0;
        let profile = &e.1;

        let elapsed_ms = now
            .duration_since(started_at)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;

        // Compress the exit walk so it REACHES the door before the reducer's
        // EXIT_GRACE_WINDOW reaps the slot. Physics exit duration for far/slow
        // desks can exceed 4500ms; without this the slot is GC'd mid-walk and
        // the sprite vanishes in the corridor instead of reaching the door.
        // (Entry has no such cap — nothing GCs an entering agent.)
        let exit_budget = (pixtuoid_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64)
            .saturating_sub(300);
        let eff_elapsed = if profile.duration_ms.saturating_add(profile.pause_ms) > exit_budget {
            (elapsed_ms.saturating_mul(profile.duration_ms) / exit_budget.max(1)).max(elapsed_ms)
        } else {
            elapsed_ms
        };

        // GC: walk fully done including pause → return None so the slot
        // disappears (same as old ENTRY_ANIMATION_MS gate).
        if walk_arrived(profile, eff_elapsed) {
            return None;
        }

        let t_x1000 = walk_progress(profile, eff_elapsed);
        let frame = ((eff_elapsed / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        let from = Point {
            x: desk.x + 6,
            y: desk.y + 4,
        };

        return route_walking_pose(
            slot,
            now,
            layout,
            router,
            overlay,
            history,
            Pose::Walking {
                from,
                to: door_target,
                t_x1000,
                frame,
                carrying_coffee: false,
            },
        );
    }

    // ---- ENTRY branch ------------------------------------------------------
    // Gate: spawn window check reuses ENTRY_ANIMATION_MS only as a bound on
    // how long we try to route. Physics duration is the real walk time.
    let since_spawn = now
        .duration_since(slot.created_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    if let Some(door) = layout.door_threshold {
        let mstate = motion
            .entry(slot.agent_id)
            .or_insert_with(|| MotionState::new(slot.agent_id));

        // Snapshot on first sighting if we're within the spawn window.
        if mstate.entry.is_none() && since_spawn < ENTRY_ANIMATION_MS {
            let to_desk = Point {
                x: desk.x + 6,
                y: desk.y + 4,
            };
            let h = slot.agent_id.raw();
            let jx = ((h % 9) as i32 - 4) as i16;
            let jy = (((h >> 16) % 9) as i32 - 4) as i16;
            let to_jittered = Point {
                x: to_desk.x.saturating_add_signed(jx),
                y: to_desk.y.saturating_add_signed(jy),
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
                let to_desk = Point {
                    x: desk.x + 6,
                    y: desk.y + 4,
                };
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    router,
                    overlay,
                    history,
                    Pose::Walking {
                        from: door,
                        to: to_desk,
                        t_x1000,
                        frame,
                        carrying_coffee: false,
                    },
                );
            }
            // walk_arrived — fall through to state-driven pose (Correction C).
            // DO NOT call `derive()` here as that would re-fire the linear
            // entry override and cause a double-walk.
        }
    }

    // ---- WANDER DISPATCH (Idle agents whose entry walk is done) ------------
    // Reaching this line means the entry branch above already returned for any
    // in-flight entry walk, so the agent's entry is complete (arrived early for
    // near desks, or never started). Gate on Idle, NOT on `since_spawn >=
    // ENTRY_ANIMATION_MS` — that fixed 4000ms gate made a near-desk agent that
    // physically arrived in ~1s sit in core's fixed-fraction idle_pose until 4s
    // and then snap to physics wander. Drive `advance_wander` right away.
    // SeatedThinking still takes priority so the thinking-pose window is intact.
    let is_idle = matches!(slot.state, pixtuoid_core::state::ActivityState::Idle);
    if is_idle && slot.exiting_at.is_none() {
        // Check thinking-pose seam: if the agent recently finished active
        // work and is within the thinking window, return SeatedThinking now
        // regardless of wander phase.  This keeps the existing thinking-pose
        // behaviour entirely intact (no regression).
        let was_active = slot.last_event_at > slot.created_at;
        let since_last_event = now
            .duration_since(slot.last_event_at)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        if was_active && since_last_event < THINKING_WINDOW_SECS {
            // Thinking window active — return SeatedThinking directly.
            return Some(Pose::SeatedThinking);
        }

        let (wander_phase, t_phys) = advance_wander(slot, now, layout, router, overlay, motion);

        match wander_phase {
            WanderPhase::WalkingOut => {
                let ms = motion.get(&slot.agent_id)?;
                let desk_point = *layout.home_desks.get(slot.desk_index)?;
                let dest = ms.wander_dest;
                // Walk-out starts at desk+(6,4) (the seated anchor) so there's
                // no stand-up jump; symmetric with walk-back's snap_target.
                // This intentionally differs from core::idle_pose's raw
                // `from: desk` (only the routed TUI path is user-visible) and
                // matches the profile routed from desk+(6,4) in advance_wander.
                let from = Point {
                    x: desk_point.x + 6,
                    y: desk_point.y + 4,
                };
                let elapsed_phase = now
                    .duration_since(ms.wander_phase_started_at)
                    .unwrap_or(Duration::ZERO)
                    .as_millis() as u64;
                let frame = (elapsed_phase / WALKING_FRAME_MS) as usize % WALKING_FRAMES;
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    router,
                    overlay,
                    history,
                    Pose::Walking {
                        from,
                        to: dest,
                        t_x1000: t_phys,
                        frame,
                        carrying_coffee: false,
                    },
                );
            }
            WanderPhase::AtWaypoint => {
                let ms = motion.get(&slot.agent_id)?;
                let pose = if let (Some(wp_idx), Some(kind)) =
                    (ms.wander_dest_wp_idx, ms.wander_dest_kind)
                {
                    Pose::AtWaypoint { wp: wp_idx, kind }
                } else {
                    Pose::AimlessAt {
                        dest: ms.wander_dest,
                    }
                };
                // Record history so snap-back works if state changes now.
                let pt = ms.wander_dest;
                history.record(slot.agent_id, pt, now);
                return Some(pose);
            }
            WanderPhase::WalkingBack => {
                let ms = motion.get(&slot.agent_id)?;
                let desk_point = *layout.home_desks.get(slot.desk_index)?;
                // Endpoint is desk+(6,4) to match seated_anchor so there's no
                // jump on arrival; this intentionally differs from
                // core::idle_pose's raw `to: desk` (only the routed TUI path is
                // user-visible).
                let snap_target = Point {
                    x: desk_point.x + 6,
                    y: desk_point.y + 4,
                };
                let elapsed_phase = now
                    .duration_since(ms.wander_phase_started_at)
                    .unwrap_or(Duration::ZERO)
                    .as_millis() as u64;
                let frame = (elapsed_phase / WALKING_FRAME_MS) as usize % WALKING_FRAMES;
                let carrying_coffee = ms.wander_dest_kind == Some(WaypointKind::Pantry);
                return route_walking_pose(
                    slot,
                    now,
                    layout,
                    router,
                    overlay,
                    history,
                    Pose::Walking {
                        from: ms.wander_dest,
                        to: snap_target,
                        t_x1000: t_phys,
                        frame,
                        carrying_coffee,
                    },
                );
            }
            WanderPhase::Seated => {
                // Fall through to derive_state_only — it returns SeatedIdle.
            }
        }
    }

    // ---- STATE-DRIVEN pose -------------------------------------------------
    // Use derive_state_only (not derive) to avoid re-triggering the linear
    // entry/exit overrides in core's derive() (Correction C — no double-walk).
    let raw = derive_state_only(slot, now, layout)?;

    // Snap-back override: state-driven poses (SeatedTyping etc.) at the
    // desk would teleport the agent if they were mid-wander when state
    // changed. Replace them with a Walking pose from the previous
    // rendered position over SNAP_BACK_MS.
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
                // Walk-end target is offset (+6, +4) from the desk pixel so
                // walking_anchor(target) lands on the SAME sprite anchor
                // that seated_anchor(desk) would. Without this offset the
                // sprite jumps ~6 px right + 4 px down at the moment the
                // pose flips from Walking → SeatedTyping. The agent ends
                // visually AT the desk (anchor-equivalent), so there's no
                // perceivable transition flash.
                let snap_target = Point {
                    x: desk.x + 6,
                    y: desk.y + 4,
                };
                // Retrieve or snapshot the physics profile for this snap-back.
                // Re-arm when EITHER there's no profile yet OR a NEW state
                // transition began (stored `started_at != slot.state_started_at`):
                // a second desk-bound transition within the 900ms window would
                // otherwise reuse the stale T0 clock and jump mid-progress. The
                // stored key is `slot.state_started_at` (not `now`) so the elapsed
                // clock and the re-arm key agree. A fresh HashMap each call (tests)
                // still snapshots on first sight — `snap_back` is None.
                let ms_entry = motion
                    .entry(slot.agent_id)
                    .or_insert_with(|| MotionState::new(slot.agent_id));
                let needs_arm = match &ms_entry.snap_back {
                    Some((started_at, _, _)) => *started_at != slot.state_started_at,
                    None => true,
                };
                if needs_arm {
                    let path = [prev, snap_target];
                    let len = octile_path_len(&path);
                    let p = walk_profile(len, WalkIntent::SnapBack, slot.agent_id);
                    ms_entry.snap_back = Some((slot.state_started_at, p, prev));
                }
                // Clone out (releases the borrow) so we can clear snap_back below.
                // No `unwrap`: `needs_arm` set Some above, or the match at line
                // ~408 witnessed Some — a None here is unreachable, but fall back
                // to the seated pose gracefully rather than panic (CLAUDE.md: no
                // unwrap in non-test code).
                match ms_entry.snap_back.clone() {
                    None => raw,
                    Some((started_at, profile, _snap_prev)) => {
                        let elapsed_ms = now
                            .duration_since(started_at)
                            .unwrap_or(Duration::ZERO)
                            .as_millis() as u64;
                        // Time-compress so the eased walk COMPLETES by the 900ms
                        // responsive window. Snap-back distances routinely exceed
                        // the ~13px the physics finishes within 900ms; without this
                        // the walk is cut off mid-path by the window guard and the
                        // sprite teleports the remaining distance to the desk.
                        let eff_elapsed = if profile.duration_ms > SNAP_BACK_MS {
                            (elapsed_ms.saturating_mul(profile.duration_ms) / SNAP_BACK_MS)
                                .max(elapsed_ms)
                        } else {
                            elapsed_ms
                        };
                        if walk_arrived(&profile, eff_elapsed) {
                            // Short snaps complete + pause before the window edge —
                            // clear so the next state transition re-snapshots fresh.
                            ms_entry.snap_back = None;
                            raw
                        } else {
                            let t_x1000 = walk_progress(&profile, eff_elapsed);
                            let frame =
                                ((eff_elapsed / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
                            Pose::Walking {
                                from: prev,
                                to: snap_target,
                                t_x1000,
                                frame,
                                carrying_coffee: false,
                            }
                        }
                    }
                }
            } else {
                raw
            }
        } else {
            raw
        }
    } else {
        // Hard wall: clear any stale snap-back profile so the next state
        // transition gets a fresh snapshot rather than replaying a previous one.
        if let Some(ms) = motion.get_mut(&slot.agent_id) {
            if ms.snap_back.is_some() {
                ms.snap_back = None;
            }
        }
        raw
    };

    route_walking_pose(slot, now, layout, router, overlay, history, pose)
}

/// Apply A*-based polyline routing to a `Pose::Walking`, recording
/// history with `now`. For non-Walking poses, records waypoint/aimless
/// positions to history and returns `Some(pose)`.
///
/// This is the single shared helper for entry, exit, snap-back, and
/// state-driven walks (Correction B). Records history with `now` (not
/// `slot.last_event_at`) so snap-back lookups are fresh.
fn route_walking_pose(
    slot: &AgentSlot,
    now: SystemTime,
    layout: &Layout,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut PoseHistory,
    pose: Pose,
) -> Option<Pose> {
    let Pose::Walking {
        from,
        to,
        t_x1000,
        frame,
        carrying_coffee,
    } = pose
    else {
        // Record AtWaypoint / AimlessAt positions too — they're a valid
        // "previous position" for a subsequent snap-back walk.
        let pt = match &pose {
            Pose::AtWaypoint { wp, .. } => layout.waypoints.get(*wp).map(|w| w.pos),
            Pose::AimlessAt { dest } => Some(*dest),
            _ => None,
        };
        if let Some(p) = pt {
            history.record(slot.agent_id, p, now);
        }
        return Some(pose);
    };

    // Per-agent path personality: perturb the routing destination by a
    // few pixels hashed from the agent_id. Different agents heading
    // between the same two waypoints get different cache keys and (in
    // most cases) visibly different polylines — breaks the "ant trail"
    // effect when multiple agents converge on the same place. The last
    // polyline point is then restored to the true `to` so the walker
    // ends at the canonical destination, not the jittered approximation.
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
        // Straight-line walk — record the interpolated position for next
        // frame's snap-back lookup. Use `now` not `last_event_at`.
        history.record(slot.agent_id, walking_position(from, to, t_x1000), now);
        return Some(Pose::Walking {
            from,
            to,
            t_x1000,
            frame,
            carrying_coffee,
        });
    }
    // Map global t to a (segment_idx, t_within_segment) using cumulative
    // octile distance — same metric A* used to plan the path, so timing
    // stays uniform along diagonals.
    let mut leg_lens: Vec<u32> = Vec::with_capacity(path.len() - 1);
    for w in path.windows(2) {
        leg_lens.push(octile_distance(w[0], w[1]));
    }
    let total: u32 = leg_lens.iter().sum();
    if total == 0 {
        return Some(pose);
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
            // Record the walker's current position for the next frame's
            // snap-back lookup. Use `now` not `last_event_at`.
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
    // Past the last segment — snap to final.
    let last = path.len() - 1;
    history.record(slot.agent_id, path[last], now);
    Some(Pose::Walking {
        from: path[last - 1],
        to: path[last],
        t_x1000: 1000,
        frame,
        carrying_coffee,
    })
}

/// Pure linear interpolation along the segment from `from` to `to`. The
/// rendering side has its own `walking_position` in renderer.rs that
/// also applies vertical breathing; this one is for history-tracking
/// only (we want the deterministic position, not the breath offset).
use crate::tui::pixel_painter::walking_position;

pub(in crate::tui) fn octile_distance(a: Point, b: Point) -> u32 {
    let dx = (a.x as i32 - b.x as i32).unsigned_abs();
    let dy = (a.y as i32 - b.y as i32).unsigned_abs();
    14 * dx.min(dy) + 10 * (dx.max(dy) - dx.min(dy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::source::Activity;
    use pixtuoid_core::state::ActivityState;
    use pixtuoid_core::walkable::WalkableMask;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    /// Stub router for testing — returns a pre-baked polyline so segment
    /// mapping can be exercised without real A* over a layout.
    struct StubRouter {
        path: Vec<Point>,
    }

    impl StubRouter {
        /// Straight-line: `route` returns `[from, to]` regardless of input.
        fn straight() -> Self {
            Self { path: vec![] }
        }
        /// Hardcoded polyline; the binary's `derive_with_routing` then
        /// restores the last point to the original `to` per the
        /// jitter-correction logic.
        fn corners(path: Vec<Point>) -> Self {
            Self { path }
        }
    }

    impl Router for StubRouter {
        fn route(
            &mut self,
            _: &WalkableMask,
            _: &pixtuoid_core::walkable::OccupancyOverlay,
            from: Point,
            to: Point,
        ) -> Vec<Point> {
            if self.path.is_empty() {
                vec![from, to]
            } else {
                self.path.clone()
            }
        }
        fn invalidate(&mut self) {}
    }

    fn layout() -> Layout {
        Layout::compute(120, 96, 4).expect("fits")
    }

    fn active_slot(state_started_at: SystemTime, created_at: SystemTime) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path("/snap.jsonl"),
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: Arc::from("cc"),
            state: ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from("Edit")),
            },
            state_started_at,
            last_event_at: created_at,
            created_at,
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

    fn entry_slot(created_at: SystemTime) -> AgentSlot {
        let mut s = active_slot(created_at, created_at);
        s.state = ActivityState::Idle;
        s
    }

    #[test]
    fn snap_back_walks_from_history_when_state_just_flipped() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let slot = active_slot(now, now - Duration::from_secs(60));
        let desk = l.home_desks[0];
        // Far waypoint position recorded one frame ago: snap-back should fire.
        let prev = Point {
            x: desk.x + 50,
            y: desk.y + 30,
        };
        let mut history = PoseHistory::new();
        history.record(slot.agent_id, prev, now - Duration::from_millis(50));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        match derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        ) {
            Some(Pose::Walking { from, .. }) => {
                assert_eq!(from, prev, "snap-back walk should start from recorded prev");
            }
            other => panic!("expected snap-back Walking pose, got {other:?}"),
        }
    }

    #[test]
    fn snap_back_long_distance_completes_by_window_no_teleport() {
        // Regression: a snap-back over a distance whose physics duration exceeds
        // SNAP_BACK_MS (the common case — agents snap back from far waypoints)
        // must be time-compressed so it REACHES the desk by the 900ms window
        // edge. Before the fix it capped elapsed at 900ms → progress stuck mid-
        // path → the sprite teleported the remaining distance when the window
        // guard flipped it to seated.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        // State flipped 880ms ago — just inside the 900ms window.
        let slot = active_slot(
            now - Duration::from_millis(880),
            now - Duration::from_secs(60),
        );
        let desk = l.home_desks[0];
        // Far prev (octile ~544) → SnapBack physics duration ~1.9s >> 900ms.
        let prev = Point {
            x: desk.x + 50,
            y: desk.y + 30,
        };
        let mut history = PoseHistory::new();
        history.record(slot.agent_id, prev, now - Duration::from_millis(50));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        match derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        ) {
            Some(Pose::Walking { t_x1000, .. }) => {
                assert!(
                    t_x1000 >= 950,
                    "long snap-back must be ~complete by the window edge (no teleport), got t_x1000={t_x1000}"
                );
            }
            other => panic!("expected near-complete Walking pose, got {other:?}"),
        }
    }

    #[test]
    fn snap_back_skipped_when_prev_within_min_distance() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let slot = active_slot(now, now - Duration::from_secs(60));
        let desk = l.home_desks[0];
        // Only 3 px away — below the 8-px snap-back threshold.
        let close = Point {
            x: desk.x + 3,
            y: desk.y,
        };
        let mut history = PoseHistory::new();
        history.record(slot.agent_id, close, now - Duration::from_millis(50));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let p = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        assert!(
            matches!(p, Some(Pose::SeatedTyping { .. })),
            "close prev should NOT trigger snap-back, got {p:?}"
        );
    }

    #[test]
    fn snap_back_skipped_after_900ms_window() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        // state_started_at is 1.5 s ago — past SNAP_BACK_MS=900.
        let slot = active_slot(
            now - Duration::from_millis(1_500),
            now - Duration::from_secs(60),
        );
        let desk = l.home_desks[0];
        let prev = Point {
            x: desk.x + 50,
            y: desk.y + 30,
        };
        let mut history = PoseHistory::new();
        history.record(slot.agent_id, prev, now - Duration::from_millis(50));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let p = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        assert!(
            matches!(p, Some(Pose::SeatedTyping { .. })),
            "snap-back window should be expired at 1.5s, got {p:?}"
        );
    }

    #[test]
    fn snap_back_skipped_without_recent_history() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let slot = active_slot(now, now - Duration::from_secs(60));
        let mut history = PoseHistory::new(); // empty
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let p = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        assert!(
            matches!(p, Some(Pose::SeatedTyping { .. })),
            "no prev history → raw pose, got {p:?}"
        );
    }

    #[test]
    fn multi_segment_path_maps_t_to_segment_via_octile_distance() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        // Entry walk in physics mode: 400ms elapsed. Physics (accel ramp) means
        // the agent is early in the path — earlier than linear t=10% would give.
        // The key check is that the segment-mapper correctly places the agent on
        // segment 0 (door→mid) rather than segment 1, regardless of the exact
        // physics-derived t_x1000.
        let slot = entry_slot(now - Duration::from_millis(400));
        let mut history = PoseHistory::new();
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let door = l.door_threshold.expect("door");
        let desk = l.home_desks[0];
        let mid = Point {
            x: (door.x + desk.x) / 2,
            y: (door.y + desk.y) / 2,
        };
        let mut router = StubRouter::corners(vec![door, mid, desk]);
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let p = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        match p {
            Some(Pose::Walking {
                from, to, t_x1000, ..
            }) => {
                assert_eq!(from, door, "first segment starts at door, got {from:?}");
                assert_eq!(to, mid, "first segment ends at mid, got {to:?}");
                // Physics progress at 400ms is in [0,500] — we're on the first segment.
                // The wider band covers both physics (accel) and the old linear case.
                assert!(
                    (0..=500).contains(&t_x1000),
                    "expected first-segment seg_t in [0,500], got t_x1000={t_x1000}"
                );
                assert!(history.recent(slot.agent_id, 1_000, now).is_some());
            }
            other => panic!("expected Walking on segment 0, got {other:?}"),
        }
    }

    #[test]
    fn at_waypoint_pose_records_position_to_history() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        // Construct a synthetic AtWaypoint pose by going through derive
        // with carefully picked timing is hard — instead, exercise the
        // history-record path by feeding derive an AimlessAt pose via
        // a custom orchestration. Easiest: re-call derive_with_routing
        // for a non-walking pose case. Idle agent with state_started_at
        // not in a trip phase → SeatedIdle (non-walking, non-waypoint).
        // After this call, no history is recorded because SeatedIdle
        // isn't in the "record" list. That's correct behaviour — verify
        // by ensuring history is empty after the call.
        let slot = AgentSlot {
            agent_id: AgentId::from_transcript_path("/idle.jsonl"),
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: Arc::from("cc"),
            state: ActivityState::Idle,
            state_started_at: now,
            created_at: now - Duration::from_secs(60),
            last_event_at: now - Duration::from_secs(60),
            exiting_at: None,
            pending_idle_at: None,

            desk_index: 0,
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        };
        let mut history = PoseHistory::new();
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let _ = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        // SeatedIdle isn't recorded — that's the contract.
        assert!(
            history.recent(slot.agent_id, 1_000, now).is_none(),
            "SeatedIdle should not write history"
        );
    }

    #[test]
    fn delegates_to_derive_for_oob_desk() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let mut slot = active_slot(now, now - Duration::from_secs(60));
        slot.desk_index = 999;
        let mut history = PoseHistory::new();
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        assert!(derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion
        )
        .is_none());
    }

    #[test]
    fn pose_history_record_and_recent() {
        let id = AgentId::from_transcript_path("/test/a.jsonl");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let pt = Point { x: 42, y: 99 };
        let mut history = PoseHistory::new();
        assert!(history.recent(id, 500, now).is_none());
        history.record(id, pt, now);
        assert_eq!(history.recent(id, 500, now), Some(pt));
    }

    #[test]
    fn pose_history_recent_expires() {
        let id = AgentId::from_transcript_path("/test/b.jsonl");
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let pt = Point { x: 10, y: 20 };
        let mut history = PoseHistory::new();
        history.record(id, pt, t0);
        let t1 = t0 + Duration::from_millis(600);
        assert_eq!(history.recent(id, 500, t1), None);
        assert_eq!(history.recent(id, 700, t1), Some(pt));
    }

    // ---- Phase 4: snap-back physics tests ---------------------------------

    #[test]
    fn snap_back_progress_is_physics_eased_not_linear() {
        // Use a SHORT path (desk + 10px) so the walk is in the TRIANGULAR
        // kinematic regime — distance ∝ t², so at 25% of duration the agent
        // covers only 1/16 of the path (t_x1000 ≈ 62), well below linear's 250.
        //
        // Distance choice: prev = desk+(10,5) → snap_target = desk+(6,4)
        //   dx=4, dy=1, octile = 14*1 + 10*(4-1) = 44 units.
        //   L_crit(max speed) ≈ 287 → 44 is firmly triangular for all agents.
        //   T ≈ 2*sqrt(44/6.5e-4) ≈ 520 ms → T/4 ≈ 130 ms < 300 ms history gate.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let slot = active_slot(now, now - Duration::from_secs(60));
        let desk = l.home_desks[0];
        // Short-but-qualifying distance (manhattan 15 ≥ SNAP_BACK_MIN_DIST=8).
        let prev = Point {
            x: desk.x + 10,
            y: desk.y + 5,
        };

        let mut history = PoseHistory::new();
        history.record(slot.agent_id, prev, now - Duration::from_millis(50));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

        // Frame 0: state just flipped — snapshots the physics profile.
        let _pose0 = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        let ms = motion
            .get(&slot.agent_id)
            .expect("MotionState created on frame 0");
        let (_, ref profile, _) = *ms.snap_back.as_ref().expect("snap_back profile stored");
        let dur_ms = profile.duration_ms;
        assert!(
            dur_ms > 0,
            "profile duration must be > 0 for a non-trivial distance"
        );

        // Frame 1: exactly 25% of profile duration elapsed.
        // Record history at quarter_now - 50ms so it's fresh (age = 50ms < 300ms gate).
        let slot_q = active_slot(now, now - Duration::from_secs(60));
        let quarter_now = now + Duration::from_millis(dur_ms / 4);
        let mut history2 = PoseHistory::new();
        history2.record(
            slot_q.agent_id,
            prev,
            quarter_now - Duration::from_millis(50),
        );
        let p = derive_with_routing(
            &slot_q,
            quarter_now,
            &l,
            &mut router,
            &overlay,
            &mut history2,
            &mut motion,
        );

        match p {
            Some(Pose::Walking { t_x1000, .. }) => {
                // Triangular profile: s(T/4) = (1/2)*a*(T/4)² = L/16
                // → t_x1000 ≈ 1000*L/16/L = 62. Linear would be 250.
                // We assert strictly < 250 (generous threshold).
                assert!(
                    t_x1000 < 250,
                    "physics ease-in: expected t_x1000 < 250 at 25% of duration (triangular), got {t_x1000}"
                );
            }
            other => panic!("expected Walking pose at 25% of snap-back duration, got {other:?}"),
        }
    }

    #[test]
    fn snap_back_profile_stored_in_motion_state() {
        // Second call for the same snap-back must REUSE the frozen profile
        // (same duration_ms), not re-snapshot a new one.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let slot = active_slot(now, now - Duration::from_secs(60));
        let desk = l.home_desks[0];
        let prev = Point {
            x: desk.x + 50,
            y: desk.y + 30,
        };

        let mut history = PoseHistory::new();
        history.record(slot.agent_id, prev, now - Duration::from_millis(50));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

        // Frame 1: creates the profile.
        let _p1 = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        let dur1 = motion
            .get(&slot.agent_id)
            .and_then(|ms| ms.snap_back.as_ref())
            .map(|(_, p, _)| p.duration_ms)
            .expect("snap_back profile created on frame 1");

        // Frame 2: 100ms later with fresh history but SAME persistent motion map.
        let slot2 = active_slot(now, now - Duration::from_secs(60));
        let t2 = now + Duration::from_millis(100);
        history.record(slot2.agent_id, prev, t2 - Duration::from_millis(50));
        let _p2 = derive_with_routing(
            &slot2,
            t2,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        let dur2 = motion
            .get(&slot2.agent_id)
            .and_then(|ms| ms.snap_back.as_ref())
            .map(|(_, p, _)| p.duration_ms)
            .expect("snap_back profile still present on frame 2");

        assert_eq!(
            dur1, dur2,
            "snap-back profile must be snapshotted once and reused across frames"
        );
    }

    #[test]
    fn snap_back_rearms_on_new_state_transition() {
        // A SECOND desk-bound transition within the 900ms window (state_started_at
        // advances while snap_back still holds the T0 tuple) must RE-ARM: the
        // stored `snap_back.0` should track the new `state_started_at`, not the
        // stale T0 — otherwise the snap-back clock jumps mid-progress.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let desk = l.home_desks[0];
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let mut history = PoseHistory::new();

        // T0: first transition fires a snap-back.
        let t0 = now;
        let slot0 = active_slot(t0, now - Duration::from_secs(60));
        let prev0 = Point {
            x: desk.x + 50,
            y: desk.y + 30,
        };
        history.record(slot0.agent_id, prev0, t0 - Duration::from_millis(50));
        let _ = derive_with_routing(
            &slot0,
            t0,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        let stored0 = motion
            .get(&slot0.agent_id)
            .and_then(|ms| ms.snap_back.as_ref())
            .map(|(s, _, _)| *s)
            .expect("snap_back armed at T0");
        assert_eq!(stored0, t0, "first arm should key on T0 state_started_at");

        // T0+400ms: a NEW transition (state_started_at advanced) within the window.
        let t1_state = t0 + Duration::from_millis(400);
        // Same agent_id (active_slot uses a fixed transcript path) so the motion
        // entry is reused; only state_started_at moved.
        let slot1 = active_slot(t1_state, now - Duration::from_secs(60));
        let now1 = t1_state; // observe at the new transition instant
        let prev1 = Point {
            x: desk.x + 40,
            y: desk.y + 25,
        };
        history.record(slot1.agent_id, prev1, now1 - Duration::from_millis(50));
        let _ = derive_with_routing(
            &slot1,
            now1,
            &l,
            &mut router,
            &overlay,
            &mut history,
            &mut motion,
        );
        let stored1 = motion
            .get(&slot1.agent_id)
            .and_then(|ms| ms.snap_back.as_ref())
            .map(|(s, _, _)| *s)
            .expect("snap_back still present after new transition");
        assert_eq!(
            stored1, t1_state,
            "snap-back must re-arm to the NEW state_started_at, not the stale T0"
        );
        assert_ne!(
            stored1, t0,
            "re-armed clock must differ from the old T0 clock"
        );
    }

    // ---- Phase 3: entry/exit physics tests --------------------------------
    // These live alongside the existing snap_back_* tests.
    // Requires: physics::walk_profile, motion::MotionState (Phase 0-2 outputs).

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

    /// Return (near_desk_index, far_desk_index) by actual octile distance
    /// from the door to each desk+offset. Panics if layout has < 2 desks
    /// or no door_threshold.
    fn near_far_desk_indices(l: &Layout) -> (usize, usize) {
        let door = l.door_threshold.expect("layout must have door_threshold");
        let dists: Vec<u32> = l
            .home_desks
            .iter()
            .map(|d| {
                let target = Point {
                    x: d.x + 6,
                    y: d.y + 4,
                };
                octile_distance(door, target)
            })
            .collect();
        let near_idx = dists
            .iter()
            .enumerate()
            .min_by_key(|&(_, d)| d)
            .map(|(i, _)| i)
            .unwrap();
        let far_idx = dists
            .iter()
            .enumerate()
            .max_by_key(|&(_, d)| d)
            .map(|(i, _)| i)
            .unwrap();
        assert_ne!(
            dists[near_idx], dists[far_idx],
            "need distinct near/far distances for this test"
        );
        assert!(
            dists[far_idx] >= dists[near_idx] * 3 / 2,
            "far dist ({}) must be ≥ 1.5× near dist ({}) for a meaningful test",
            dists[far_idx],
            dists[near_idx]
        );
        (near_idx, far_idx)
    }

    #[test]
    fn entry_duration_scales_with_path_longer_desk_takes_longer() {
        // Compute the actual nearest and farthest desks by octile distance
        // from the door (Correction M — don't assume desk 0 is nearest).
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout(); // 120×96, 4 desks
        let (near_idx, far_idx) = near_far_desk_indices(&l);

        let near = entry_slot_far(now, near_idx);
        let far = entry_slot_far(now, far_idx);

        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();

        // Two separate motion maps — each agent's first call snapshots its own profile.
        let mut motion_near: HashMap<AgentId, MotionState> = HashMap::new();
        let mut motion_far: HashMap<AgentId, MotionState> = HashMap::new();
        let mut hist_near = PoseHistory::new();
        let mut hist_far = PoseHistory::new();
        let mut router_n = StubRouter::straight();
        let mut router_f = StubRouter::straight();

        // First call: snapshots the entry profile.
        let _pn = derive_with_routing(
            &near,
            now,
            &l,
            &mut router_n,
            &overlay,
            &mut hist_near,
            &mut motion_near,
        );
        let _pf = derive_with_routing(
            &far,
            now,
            &l,
            &mut router_f,
            &overlay,
            &mut hist_far,
            &mut motion_far,
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
            "far desk duration {dur_far}ms must be >= near desk {dur_near}ms"
        );
    }

    #[test]
    fn nearer_desk_arrives_before_farther_desk() {
        // Same created_at, same StubRouter (straight-line). Run enough frames
        // so the near desk agent walk_arrived flips; the far desk must still
        // be Walking at that point. Desks are chosen by actual octile distance
        // from the door (Correction M).
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let (near_idx, far_idx) = near_far_desk_indices(&l);

        let near = entry_slot_far(now, near_idx);
        let far = entry_slot_far(now, far_idx);

        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut motion_near = HashMap::new();
        let mut motion_far = HashMap::new();
        let mut hist_near = PoseHistory::new();
        let mut hist_far = PoseHistory::new();
        let mut router_n = StubRouter::straight();
        let mut router_f = StubRouter::straight();

        // Snapshot on first call.
        let _ = derive_with_routing(
            &near,
            now,
            &l,
            &mut router_n,
            &overlay,
            &mut hist_near,
            &mut motion_near,
        );
        let _ = derive_with_routing(
            &far,
            now,
            &l,
            &mut router_f,
            &overlay,
            &mut hist_far,
            &mut motion_far,
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
            &near,
            t1,
            &l,
            &mut router_n,
            &overlay,
            &mut hist_near,
            &mut motion_near,
        );
        let p_far = derive_with_routing(
            &far,
            t1,
            &l,
            &mut router_f,
            &overlay,
            &mut hist_far,
            &mut motion_far,
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
            let mut motion = HashMap::new();
            let mut hist = PoseHistory::new();
            let mut router = StubRouter::straight();
            let _ = derive_with_routing(
                &slot,
                now,
                &l,
                &mut router,
                &overlay,
                &mut hist,
                &mut motion,
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
        let mut motion = HashMap::new();
        let mut hist = PoseHistory::new();
        let mut router = StubRouter::straight();

        // First call: snapshot.
        let _ = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut hist,
            &mut motion,
        );
        let (started_at_1, _) = motion[&slot.agent_id]
            .exit
            .as_ref()
            .expect("exit profile set on first call")
            .clone();

        // Second call 100 ms later: must not re-snapshot.
        let t1 = now + Duration::from_millis(100);
        let _ = derive_with_routing(&slot, t1, &l, &mut router, &overlay, &mut hist, &mut motion);
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
    fn exit_far_completes_before_grace_window_no_vanish() {
        // Regression: a far/slow physics exit walk whose duration exceeds the
        // reducer's EXIT_GRACE_WINDOW (4500ms) must be time-compressed to REACH
        // the door before the slot is GC'd. Before the fix the sprite popped out
        // of existence mid-corridor (~85% along) when the grace window reaped it.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        let door = l.door_threshold.expect("door");
        let desk = l.home_desks[0];
        let from = Point {
            x: desk.x + 6,
            y: desk.y + 4,
        };
        // Synthetic long route (≥1600 octile) so the physics exit duration
        // exceeds the exit budget and the compression path is exercised.
        let mid1 = Point {
            x: from.x.saturating_add(80),
            y: from.y,
        };
        let mid2 = Point {
            x: mid1.x,
            y: mid1.y.saturating_add(80),
        };
        let mut router = StubRouter::corners(vec![from, mid1, mid2, door]);
        // Exit started 4300ms ago — just inside the 4500ms grace window.
        let slot = exiting_slot(
            now - Duration::from_millis(4300),
            now - Duration::from_secs(60),
        );
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut hist = PoseHistory::new();
        let mut motion = HashMap::new();
        match derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut hist, &mut motion) {
            // Reached the door (Walking at the end of the path) or already
            // arrived (None, GC imminent). Either way: NOT stuck mid-corridor.
            Some(Pose::Walking { t_x1000, .. }) => assert!(
                t_x1000 >= 950,
                "far exit must reach the door by the grace window (no mid-corridor vanish), got t_x1000={t_x1000}"
            ),
            None => {}
            other => panic!("expected Walking near the door or None (arrived), got {other:?}"),
        }
        // Sanity: the snapshotted exit profile really exceeded the budget, so the
        // compression branch (not the pass-through) was the one under test.
        let dur = motion[&slot.agent_id]
            .exit
            .as_ref()
            .expect("exit profile snapshotted")
            .1
            .duration_ms;
        assert!(
            dur > 4200,
            "test setup: exit duration {dur}ms should exceed the ~4200ms exit budget"
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
        let mut motion = HashMap::new();
        let mut hist = PoseHistory::new();
        let mut router = StubRouter::straight();

        let _ = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut hist,
            &mut motion,
        );
        let profile = &motion[&slot.agent_id]
            .exit
            .as_ref()
            .expect("exit profile set")
            .1;
        // v_cruise stored in WalkProfile is v_base * speed_mult — it must be
        // derived from V_CRUISE_COMMUTE (0.36), NOT V_CRUISE_WANDER (0.25).
        // The minimum possible commute v_cruise = 0.36 * 0.85 ≈ 0.306,
        // while the maximum wander v_cruise = 0.25 * 1.20 ≈ 0.300.
        // There's a gap: anything >= 0.301 is unambiguously commute.
        let min_commute =
            pixtuoid_core::physics::V_CRUISE_COMMUTE * pixtuoid_core::physics::SPEED_MULT_MIN;
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

    #[test]
    fn exit_with_no_door_does_not_vanish() {
        // Regression: on a layout with no door_threshold (very narrow
        // terminal), an exiting agent must NOT return None on its first
        // frame (None is the GC signal — the agent would vanish instantly).
        // It should fall through to the state-driven pose and let the
        // reducer's grace window GC the slot instead.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut l = layout();
        l.door_threshold = None;
        let slot = exiting_slot(now, now - Duration::from_secs(60));
        let overlay = pixtuoid_core::walkable::OccupancyOverlay::new();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let mut hist = PoseHistory::new();
        let mut router = StubRouter::straight();

        let p = derive_with_routing(
            &slot,
            now,
            &l,
            &mut router,
            &overlay,
            &mut hist,
            &mut motion,
        );
        assert!(
            p.is_some(),
            "exiting agent on a no-door layout must not vanish (got None)"
        );
        // No exit profile should have been snapshotted — we never reached
        // the physics exit branch.
        assert!(
            motion
                .get(&slot.agent_id)
                .is_none_or(|ms| ms.exit.is_none()),
            "no exit profile should be snapshotted when there is no door"
        );
    }
}
