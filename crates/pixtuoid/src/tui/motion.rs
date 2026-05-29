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

use pixtuoid_core::physics::WalkProfile;
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
    /// Sentinel `UNIX_EPOCH` signals a fresh agent; `advance_wander`
    /// detects this to bootstrap the wander clock.
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
    /// Last `now` at which `advance_wander` performed a transition. Used for
    /// idempotency: when `now <= last_advanced_at`, the call is a no-op on
    /// mutable state (computes pose from existing phase state only).
    /// Sentinel `UNIX_EPOCH` means the agent has never been advanced.
    pub last_advanced_at: SystemTime,
}

impl MotionState {
    /// Construct a fresh `MotionState` for `agent_id`.
    ///
    /// All optional fields are `None`; wander starts in `Seated` phase with
    /// both `wander_phase_started_at` and `last_advanced_at` set to
    /// `SystemTime::UNIX_EPOCH` so `advance_wander` can detect a bootstrap
    /// agent on the first call via the epoch sentinel.
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            entry: None,
            exit: None,
            snap_back: None,
            wander_cycle_n: 0,
            wander_phase: WanderPhase::Seated,
            wander_phase_started_at: SystemTime::UNIX_EPOCH,
            wander_profile: None,
            // Placeholder — replaced on first WalkingOut transition.
            wander_dest: Point { x: 0, y: 0 },
            wander_dest_kind: None,
            wander_dest_wp_idx: None,
            last_advanced_at: SystemTime::UNIX_EPOCH,
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
    path.windows(2).map(|w| octile_distance(w[0], w[1])).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::AgentId;

    fn id() -> AgentId {
        AgentId::from_parts("test", "motion-test-agent")
    }

    // --- MotionState::new -------------------------------------------------

    #[test]
    fn motion_state_new_default_fields() {
        let ms = MotionState::new(id());
        assert!(ms.entry.is_none());
        assert!(ms.exit.is_none());
        assert!(ms.snap_back.is_none());
        assert_eq!(ms.wander_cycle_n, 0);
        assert_eq!(ms.wander_phase, WanderPhase::Seated);
        assert_eq!(ms.wander_phase_started_at, SystemTime::UNIX_EPOCH);
        assert_eq!(ms.last_advanced_at, SystemTime::UNIX_EPOCH);
        assert!(ms.wander_profile.is_none());
        assert!(ms.wander_dest_kind.is_none());
        assert!(ms.wander_dest_wp_idx.is_none());
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
}
