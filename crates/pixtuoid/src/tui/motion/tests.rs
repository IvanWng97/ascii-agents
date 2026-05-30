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
    assert!(ms.walk_path.is_none());
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

// =========================================================================
// advance_wander tests
// =========================================================================

use crate::tui::layout::Layout;
use crate::tui::pathfind::Router;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::walkable::{OccupancyOverlay, WalkableMask};
use std::path::PathBuf;
use std::sync::Arc;

// -----------------------------------------------------------------------
// Stub routers
// -----------------------------------------------------------------------

/// Straight-line stub: always returns `[from, to]`.
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
}

/// Fixed-octile-length stub: synthesises a horizontal path of the requested
/// octile length starting at `from`, ignoring `to`. Used to test phase
/// transitions with predictable walk durations.
struct FixedLen {
    octile_len: u32,
}
impl Router for FixedLen {
    fn route(
        &mut self,
        _: &WalkableMask,
        _: &OccupancyOverlay,
        from: Point,
        _to: Point,
    ) -> Vec<Point> {
        // Horizontal path: each step is 10 octile units (1 px orthogonal).
        // octile_len / 10 px ≈ requested length.
        let steps = (self.octile_len / 10) as u16;
        let mid = Point {
            x: from.x + steps / 2,
            y: from.y,
        };
        let end = Point {
            x: from.x + steps,
            y: from.y,
        };
        vec![from, mid, end]
    }
    fn invalidate(&mut self) {}
}

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

fn idle_slot(path: &str, state_started: SystemTime) -> AgentSlot {
    AgentSlot {
        agent_id: AgentId::from_transcript_path(path),
        source: Arc::from("claude-code"),
        session_id: Arc::from("s"),
        cwd: Arc::from(PathBuf::from("/p").as_path()),
        label: Arc::from("cc"),
        state: ActivityState::Idle,
        state_started_at: state_started,
        created_at: state_started
            .checked_sub(Duration::from_secs(90))
            .unwrap_or(state_started),
        last_event_at: state_started
            .checked_sub(Duration::from_secs(90))
            .unwrap_or(state_started),
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
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

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
//     on a trip cycle.
// -----------------------------------------------------------------------
#[test]
fn seated_transitions_to_walking_out_on_trip_cycle() {
    use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};

    // Find an agent where cycle_n=0 is a trip cycle.
    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/trip_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("should find a trip agent quickly");

    let now = t0();
    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Tick once to initialise.
    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    // Advance past the seated dwell.
    let later = now + Duration::from_millis(seated_dur + 50);
    advance_wander(&slot, later, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).expect("state present");
    assert!(
        matches!(ms.wander_phase, WanderPhase::WalkingOut),
        "after seated dwell on trip cycle, expected WalkingOut, got {:?}",
        ms.wander_phase
    );
    assert!(
        ms.wander_profile.is_some(),
        "walk-out profile must be snapshotted"
    );
}

// -----------------------------------------------------------------------
// T3: Non-trip cycle stays Seated even after dwell elapsed
// -----------------------------------------------------------------------
#[test]
fn non_trip_cycle_stays_seated() {
    use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};

    let stay_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/stay_{i}.jsonl")))
        .find(|id| !takes_trip(*id, 0))
        .expect("should find a stay-seated agent");

    let now = t0();
    let cycle = cycle_ms_for(stay_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

    let slot = AgentSlot {
        agent_id: stay_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let later = now + Duration::from_millis(seated_dur + 200);
    advance_wander(&slot, later, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&stay_id).expect("state present");
    // Non-trip should bump cycle_n and stay Seated.
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

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/wp_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let now = t0();
    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let profile = walk_profile(short_len, WalkIntent::WanderOut, trip_id);
    let total_walk_ms = profile.duration_ms + profile.pause_ms;

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Initialise.
    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    // Past seated dwell → WalkingOut.
    let t1 = now + Duration::from_millis(seated_dur + 10);
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

    // Get the actual snapshotted profile.
    let snap_ms = {
        let ms = motion.get(&trip_id).expect("state");
        assert!(
            matches!(ms.wander_phase, WanderPhase::WalkingOut),
            "expected WalkingOut, got {:?}",
            ms.wander_phase
        );
        ms.wander_profile
            .as_ref()
            .map(|p| p.duration_ms + p.pause_ms)
            .expect("profile snapshotted")
    };

    // Advance past the walk arrival.
    let t2 = t1 + Duration::from_millis(snap_ms + 50);
    advance_wander(&slot, t2, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander_phase, WanderPhase::AtWaypoint),
        "expected AtWaypoint after walk-out arrival, got {:?}",
        ms.wander_phase
    );
    let _ = total_walk_ms; // used for documentation
}

// -----------------------------------------------------------------------
// T5: AtWaypoint dwell transitions to WalkingBack
// -----------------------------------------------------------------------
#[test]
fn at_waypoint_transitions_to_walking_back_after_dwell() {
    use crate::tui::pose::{
        cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC,
    };
    use pixtuoid_core::physics::{walk_profile, WalkIntent};

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/dwell_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let now = t0();
    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
    let dwell_dur = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let out_profile = walk_profile(short_len, WalkIntent::WanderOut, trip_id);
    let walk_ms = out_profile.duration_ms + out_profile.pause_ms;

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    // → WalkingOut
    let t1 = now + Duration::from_millis(seated_dur + 10);
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

    // Get snapshotted walk_ms (may differ from the theoretical).
    let actual_walk_ms = motion
        .get(&trip_id)
        .and_then(|ms| ms.wander_profile.as_ref())
        .map(|p| p.duration_ms + p.pause_ms)
        .unwrap_or(walk_ms);

    // → AtWaypoint
    let t2 = t1 + Duration::from_millis(actual_walk_ms + 10);
    advance_wander(&slot, t2, &l, &mut router, &overlay, &mut motion);

    // → WalkingBack (past dwell)
    let t3 = t2 + Duration::from_millis(dwell_dur + 10);
    advance_wander(&slot, t3, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander_phase, WanderPhase::WalkingBack),
        "expected WalkingBack after dwell, got {:?}",
        ms.wander_phase
    );
    assert!(
        ms.wander_profile.is_some(),
        "walk-back profile must be snapshotted"
    );
}

// -----------------------------------------------------------------------
// T6: WalkingBack arrival increments cycle_n and resets to Seated
// -----------------------------------------------------------------------
#[test]
fn walking_back_arrival_increments_cycle_n_and_resets_to_seated() {
    use crate::tui::pose::{
        cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC,
    };
    use pixtuoid_core::physics::{walk_profile, WalkIntent};

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/cyc_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let now = t0();
    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
    let dwell_dur = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let out_profile = walk_profile(short_len, WalkIntent::WanderOut, trip_id);
    let out_ms = out_profile.duration_ms + out_profile.pause_ms;
    let back_profile = walk_profile(short_len, WalkIntent::WanderBack, trip_id);
    let back_ms = back_profile.duration_ms + back_profile.pause_ms;

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    let mut t = now;
    advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

    t += Duration::from_millis(seated_dur + 10);
    advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

    // Get actual snapshotted walk-out ms.
    let actual_out_ms = motion
        .get(&trip_id)
        .and_then(|ms| ms.wander_profile.as_ref())
        .map(|p| p.duration_ms + p.pause_ms)
        .unwrap_or(out_ms);

    t += Duration::from_millis(actual_out_ms + 10);
    advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

    t += Duration::from_millis(dwell_dur + 10);
    advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

    // Get actual snapshotted walk-back ms.
    let actual_back_ms = motion
        .get(&trip_id)
        .and_then(|ms| ms.wander_profile.as_ref())
        .map(|p| p.duration_ms + p.pause_ms)
        .unwrap_or(back_ms);

    t += Duration::from_millis(actual_back_ms + 10);
    advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).expect("state");
    assert!(
        matches!(ms.wander_phase, WanderPhase::Seated),
        "completed cycle must reset to Seated, got {:?}",
        ms.wander_phase
    );
    assert_eq!(ms.wander_cycle_n, 1, "cycle_n must increment once");
}

// -----------------------------------------------------------------------
// T7: Dwell time is independent of path length
// -----------------------------------------------------------------------
#[test]
fn dwell_time_independent_of_path_length() {
    use crate::tui::pose::{
        cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC,
    };
    use pixtuoid_core::physics::{walk_profile, WalkIntent};

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/dwell2_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
    let expected_dwell = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", t0())
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();

    for short_len in [150u32, 800u32] {
        let now = t0();
        let out_prof = walk_profile(short_len, WalkIntent::WanderOut, trip_id);
        let out_ms = out_prof.duration_ms + out_prof.pause_ms;

        let mut router = FixedLen {
            octile_len: short_len,
        };
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

        let mut t = now;
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);
        t += Duration::from_millis(seated_dur + 10);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

        // Get actual snapshotted walk-out ms.
        let actual_out_ms = motion
            .get(&trip_id)
            .and_then(|ms| ms.wander_profile.as_ref())
            .map(|p| p.duration_ms + p.pause_ms)
            .unwrap_or(out_ms);

        t += Duration::from_millis(actual_out_ms + 10);
        advance_wander(&slot, t, &l, &mut router, &overlay, &mut motion);

        // Record when we entered AtWaypoint.
        let at_wp_started = motion.get(&trip_id).unwrap().wander_phase_started_at;

        // One ms before dwell ends: must still be AtWaypoint.
        let before_end = at_wp_started + Duration::from_millis(expected_dwell.saturating_sub(5));
        advance_wander(&slot, before_end, &l, &mut router, &overlay, &mut motion);
        assert!(
            matches!(
                motion.get(&trip_id).unwrap().wander_phase,
                WanderPhase::AtWaypoint
            ),
            "short_len={short_len}: still AtWaypoint 5ms before dwell ends"
        );

        // After dwell ends: must be WalkingBack.
        let after_end = at_wp_started + Duration::from_millis(expected_dwell + 50);
        advance_wander(&slot, after_end, &l, &mut router, &overlay, &mut motion);
        assert!(
            matches!(
                motion.get(&trip_id).unwrap().wander_phase,
                WanderPhase::WalkingBack
            ),
            "short_len={short_len}: WalkingBack after dwell, expected_dwell={expected_dwell}ms"
        );
    }
}

// -----------------------------------------------------------------------
// T8: Far waypoint full-cycle wall-time longer than near
// -----------------------------------------------------------------------
#[test]
fn far_waypoint_full_cycle_is_longer() {
    use crate::tui::pose::{
        cycle_ms_for, takes_trip, PHASE_AT_WAYPOINT_FRAC, PHASE_SEATED_FRAC, PHASE_WALK_OUT_FRAC,
    };
    use pixtuoid_core::physics::{walk_profile, WalkIntent};

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/far_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;
    let dwell_dur = cycle * (PHASE_AT_WAYPOINT_FRAC - PHASE_WALK_OUT_FRAC) / 1000;

    let cycle_wall_ms = |path_len: u32| -> u64 {
        let out = walk_profile(path_len, WalkIntent::WanderOut, trip_id);
        let back = walk_profile(path_len, WalkIntent::WanderBack, trip_id);
        seated_dur
            + (out.duration_ms + out.pause_ms)
            + dwell_dur
            + (back.duration_ms + back.pause_ms)
    };

    let near_ms = cycle_wall_ms(100);
    let far_ms = cycle_wall_ms(1200);

    assert!(
        far_ms > near_ms,
        "far cycle ({far_ms}ms) must be longer than near cycle ({near_ms}ms)"
    );

    // Walk times DO differ.
    let out_near = walk_profile(100, WalkIntent::WanderOut, trip_id);
    let out_far = walk_profile(1200, WalkIntent::WanderOut, trip_id);
    assert!(
        out_far.duration_ms > out_near.duration_ms,
        "far walk must take longer"
    );
}

// -----------------------------------------------------------------------
// T9: Arrival pause holds WalkingOut phase during [T, T+pause)
// -----------------------------------------------------------------------
#[test]
fn arrival_pause_holds_walking_out_phase() {
    use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};
    use pixtuoid_core::physics::{walk_arrived, walk_profile, WalkIntent};

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/pause_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let now = t0();
    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let short_len: u32 = 200;
    let profile = walk_profile(short_len, WalkIntent::WanderOut, trip_id);
    let mid_pause_elapsed = profile.duration_ms + profile.pause_ms / 2;

    // walk_arrived must be false mid-pause.
    assert!(
        !walk_arrived(&profile, mid_pause_elapsed),
        "walk_arrived must be false mid-pause"
    );

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = FixedLen {
        octile_len: short_len,
    };
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    let t1 = now + Duration::from_millis(seated_dur + 10);
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

    // Snapshot walk-out phase start.
    let out_started = motion.get(&trip_id).unwrap().wander_phase_started_at;

    // Get the actual snapshotted mid-pause elapsed.
    let actual_profile = motion
        .get(&trip_id)
        .and_then(|ms| ms.wander_profile.as_ref())
        .expect("profile snapshotted");
    let actual_mid_elapsed = actual_profile.duration_ms + actual_profile.pause_ms / 2;

    // Mid-pause: still WalkingOut (walk_arrived returns false).
    let mid = out_started + Duration::from_millis(actual_mid_elapsed);
    advance_wander(&slot, mid, &l, &mut router, &overlay, &mut motion);
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander_phase,
            WanderPhase::WalkingOut
        ),
        "must stay WalkingOut during arrival pause"
    );
}

// -----------------------------------------------------------------------
// T10: Idempotency — advance_wander twice same `now` leaves state unchanged
// -----------------------------------------------------------------------
#[test]
fn idempotent_same_now_does_not_mutate_state() {
    use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/idem_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let now = t0();
    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Init at `now`.
    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    // Advance past seated dwell to trigger WalkingOut.
    let t1 = now + Duration::from_millis(seated_dur + 100);
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

    let (phase_before, cycle_before) = {
        let ms = motion.get(&trip_id).unwrap();
        (ms.wander_phase, ms.wander_cycle_n)
    };

    // Call again with the SAME `now` (t1) — must NOT mutate.
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).unwrap();
    assert_eq!(
        ms.wander_phase, phase_before,
        "2nd call with same now must not change phase"
    );
    assert_eq!(
        ms.wander_cycle_n, cycle_before,
        "2nd call with same now must not change cycle_n"
    );
}

// -----------------------------------------------------------------------
// T11: Bootstrap — agent idle for N cycles before first render
// -----------------------------------------------------------------------
#[test]
fn bootstrap_fast_forwards_cycle_n() {
    use crate::tui::pose::cycle_ms_for;

    let id = AgentId::from_transcript_path("/p/bootstrap.jsonl");
    let now = t0();
    let cycle = cycle_ms_for(id);
    // Agent has been Idle for exactly 10 full cycles before first render.
    let state_started = now
        .checked_sub(Duration::from_millis(10 * cycle))
        .expect("time arithmetic ok");
    let slot = idle_slot("/p/bootstrap.jsonl", state_started);

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&id).expect("state present");
    // Bootstrap jump is integer `elapsed_idle / cycle_ms`. Elapsed is set
    // to exactly 10*cycle, so cycle_n must equal EXACTLY 10 (Correction M:
    // no guessed tolerance).
    let approx_cycles = ms.wander_cycle_n;
    assert_eq!(
        approx_cycles, 10,
        "bootstrap: elapsed = 10*cycle_ms => cycle_n must equal exactly 10 (integer elapsed/cycle)"
    );
}

// -----------------------------------------------------------------------
// T12: Stale resume — a floor that was off-screen (motion frozen) must
//      resync analytically on return instead of replaying the backlog one
//      phase per frame. Mirrors the bootstrap fast-forward.
// -----------------------------------------------------------------------
#[test]
fn stale_resume_resyncs_without_replay() {
    use crate::tui::pose::{cycle_ms_for, takes_trip, PHASE_SEATED_FRAC};

    let trip_id = (0u64..500)
        .map(|i| AgentId::from_transcript_path(&format!("/p/stale_{i}.jsonl")))
        .find(|id| takes_trip(*id, 0))
        .expect("find trip agent");

    let now = t0();
    let cycle = cycle_ms_for(trip_id);
    let seated_dur = cycle * PHASE_SEATED_FRAC / 1000;

    let slot = AgentSlot {
        agent_id: trip_id,
        ..idle_slot("/dummy", now)
    };

    let l = layout();
    let overlay = OccupancyOverlay::new();
    let mut router = Straight;
    let mut motion: HashMap<AgentId, MotionState> = HashMap::new();

    // Init, then advance into a walk leg so the pre-gap phase is mid-cycle.
    advance_wander(&slot, now, &l, &mut router, &overlay, &mut motion);
    let t1 = now + Duration::from_millis(seated_dur + 50);
    advance_wander(&slot, t1, &l, &mut router, &overlay, &mut motion);
    assert!(
        matches!(
            motion.get(&trip_id).unwrap().wander_phase,
            WanderPhase::WalkingOut
        ),
        "precondition: agent should be WalkingOut before the gap"
    );

    // Floor goes off-screen for ~20 cycles; advance_wander is NOT called.
    // On return, a SINGLE call must resync (not step one phase forward).
    let resume = t1 + Duration::from_millis(20 * cycle);
    advance_wander(&slot, resume, &l, &mut router, &overlay, &mut motion);

    let ms = motion.get(&trip_id).unwrap();
    assert!(
        matches!(ms.wander_phase, WanderPhase::Seated),
        "stale resume must resync to Seated (no per-frame replay), got {:?}",
        ms.wander_phase
    );
    assert!(
        ms.wander_cycle_n >= 18,
        "stale resume must fast-forward cycle_n across the gap, got {}",
        ms.wander_cycle_n
    );
}
