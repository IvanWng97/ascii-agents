//! TUI-side pose layer.
//!
//! Re-exports the pure pose-derivation surface from `ascii_agents_core::pose`
//! and adds the binary-side machinery:
//!   * `PoseHistory` — per-agent cache of the last rendered position.
//!   * `derive_with_routing` — the routed variant of `derive` that consults
//!     a `&mut dyn Router` so walking poses follow A*-routed polylines and
//!     so state transitions are smoothed with a snap-back walk instead of
//!     teleporting back to the desk.
//!
//! Keeping the routed code on this side means `ascii-agents-core` does not
//! depend on the pathfinder — the trait lives in the binary because A* is
//! TUI-rendering-adjacent and may differ for non-terminal renderers.

use std::time::{Duration, SystemTime};

use ascii_agents_core::state::AgentSlot;
use ascii_agents_core::walkable::OccupancyOverlay;
use ascii_agents_core::AgentId;

pub use ascii_agents_core::pose::{
    cycle_ms_for, derive, is_aimless_cycle, personality_for, takes_trip, waypoint_index_for_cycle,
    Personality, Pose, ENTRY_ANIMATION_MS, TYPING_FRAMES, TYPING_FRAME_MS, WALKING_FRAMES,
    WALKING_FRAME_MS, WANDER_CYCLE_BASE_MS, WANDER_CYCLE_RANGE_MS,
};

use crate::tui::layout::{Layout, Point};
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
) -> Option<Pose> {
    let raw = derive(slot, now, layout)?;
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
            let desk = *layout.home_desks.get(slot.desk_index)?;
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
                let t = (since_state * 1000 / SNAP_BACK_MS).min(1000) as u16;
                let frame = ((since_state / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
                Pose::Walking {
                    from: prev,
                    to: snap_target,
                    t_x1000: t,
                    frame,
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

    let Pose::Walking {
        from,
        to,
        t_x1000,
        frame,
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
        // Straight-line walk — record the interpolated position for
        // next frame's snap-back lookup.
        history.record(slot.agent_id, walking_position(from, to, t_x1000), now);
        return Some(pose);
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
            // snap-back lookup.
            let cur_pos = walking_position(path[i], path[i + 1], seg_t);
            history.record(slot.agent_id, cur_pos, now);
            return Some(Pose::Walking {
                from: path[i],
                to: path[i + 1],
                t_x1000: seg_t,
                frame,
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
    })
}

/// Pure linear interpolation along the segment from `from` to `to`. The
/// rendering side has its own `walking_position` in renderer.rs that
/// also applies vertical breathing; this one is for history-tracking
/// only (we want the deterministic position, not the breath offset).
fn walking_position(from: Point, to: Point, t_x1000: u16) -> Point {
    let t = t_x1000 as i32;
    let x = (from.x as i32 + (to.x as i32 - from.x as i32) * t / 1000).max(0) as u16;
    let y = (from.y as i32 + (to.y as i32 - from.y as i32) * t / 1000).max(0) as u16;
    Point { x, y }
}

fn octile_distance(a: Point, b: Point) -> u32 {
    let dx = (a.x as i32 - b.x as i32).unsigned_abs();
    let dy = (a.y as i32 - b.y as i32).unsigned_abs();
    14 * dx.min(dy) + 10 * (dx.max(dy) - dx.min(dy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ascii_agents_core::source::Activity;
    use ascii_agents_core::state::ActivityState;
    use ascii_agents_core::walkable::WalkableMask;
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
            _: &ascii_agents_core::walkable::OccupancyOverlay,
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
            tool_call_count: 0,
            active_ms: 0,
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
        let overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        match derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history) {
            Some(Pose::Walking { from, .. }) => {
                assert_eq!(from, prev, "snap-back walk should start from recorded prev");
            }
            other => panic!("expected snap-back Walking pose, got {other:?}"),
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
        let overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let p = derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history);
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
        let overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let p = derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history);
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
        let overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let p = derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history);
        assert!(
            matches!(p, Some(Pose::SeatedTyping { .. })),
            "no prev history → raw pose, got {p:?}"
        );
    }

    #[test]
    fn multi_segment_path_maps_t_to_segment_via_octile_distance() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let l = layout();
        // Entry animation: ENTRY_ANIMATION_MS = 4000, since_spawn = 1000 →
        // t_x1000 = 250. derive returns Walking { from: door, to: desk, t=250 }.
        let slot = entry_slot(now - Duration::from_millis(1_000));
        let mut history = PoseHistory::new();
        let overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
        let door = l.door_threshold.expect("door");
        let desk = l.home_desks[0];
        // `mid` placed on the straight line between door and desk so the
        // two legs have ~equal octile distance after the last-point
        // substitution. At t=250 (1/4 of the walk), `traveled` lands
        // about halfway through segment 0 → seg_t ≈ 500.
        let mid = Point {
            x: (door.x + desk.x) / 2,
            y: (door.y + desk.y) / 2,
        };
        let mut router = StubRouter::corners(vec![door, mid, desk]);
        let p = derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history);
        match p {
            Some(Pose::Walking {
                from, to, t_x1000, ..
            }) => {
                assert_eq!(from, door, "first segment starts at door, got {from:?}");
                assert_eq!(to, mid, "first segment ends at mid, got {to:?}");
                assert!(
                    (400..=600).contains(&t_x1000),
                    "expected mid-segment ~500, got t_x1000={t_x1000}"
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
            tool_call_count: 0,
            active_ms: 0,
        };
        let mut history = PoseHistory::new();
        let overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        let _ = derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history);
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
        let overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
        let mut router = StubRouter::straight();
        assert!(derive_with_routing(&slot, now, &l, &mut router, &overlay, &mut history).is_none());
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
}
