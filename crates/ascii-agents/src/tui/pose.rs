//! State → pose derivation for the coworking-lounge renderer.
//!
//! Pure function: given an `AgentSlot`, current `SystemTime`, and `Layout`,
//! returns which `Pose` the agent should appear in this frame. Includes the
//! wander state machine for Idle agents (cycles between desk and waypoints).
//!
//! Variation knobs:
//!  * `cycle_ms_for(agent_id)` — per-agent wander cycle length so the office
//!    stops feeling clockwork-synchronized. Range 7..13s.
//!  * Waypoint choice XORs `agent_id` with the current cycle number, so each
//!    cycle the same agent picks a (likely) different waypoint.

use std::time::{Duration, SystemTime};

use ascii_agents_core::state::{ActivityState, AgentSlot};
use ascii_agents_core::AgentId;

use crate::tui::layout::{Layout, Point};

/// Base cycle length. Each agent's actual cycle = base + per-agent jitter.
pub const WANDER_CYCLE_BASE_MS: u64 = 7_000;
/// Maximum extra time added per agent — jitter range is `[0, RANGE)`.
pub const WANDER_CYCLE_RANGE_MS: u64 = 6_000;
/// Phase fractions of a cycle (×1000 to stay in integer math).
const PHASE_SEATED_FRAC: u64 = 389;        // 0..389/1000
const PHASE_WALK_OUT_FRAC: u64 = 556;      // 389..556/1000
const PHASE_AT_WAYPOINT_FRAC: u64 = 833;   // 556..833/1000
// walk-back is 833..1000/1000.

/// Frame-cycle period for animated poses.
pub const TYPING_FRAME_MS: u64 = 140;
pub const WALKING_FRAME_MS: u64 = 220;
pub const TYPING_FRAMES: usize = 2;
pub const WALKING_FRAMES: usize = 2;

/// Deterministic wander-cycle length for one agent. Each agent picks a
/// different speed so walkers don't move in lockstep.
pub fn cycle_ms_for(agent_id: AgentId) -> u64 {
    WANDER_CYCLE_BASE_MS + (agent_id.raw() >> 16) % WANDER_CYCLE_RANGE_MS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    SeatedIdle,
    SeatedTyping { frame: usize },
    StandingAtDesk,
    StandingAtWaypoint { wp: usize },
    Walking { from: Point, to: Point, t_x1000: u16, frame: usize },
}

/// Returns `None` if the slot's desk_index is out of range for `layout`.
pub fn derive(slot: &AgentSlot, now: SystemTime, layout: &Layout) -> Option<Pose> {
    let desk = *layout.home_desks.get(slot.desk_index)?;

    let elapsed = now
        .duration_since(slot.state_started_at)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    match &slot.state {
        ActivityState::Active { .. } => {
            let frame = ((elapsed / TYPING_FRAME_MS) as usize) % TYPING_FRAMES;
            Some(Pose::SeatedTyping { frame })
        }
        ActivityState::Waiting { .. } => Some(Pose::StandingAtDesk),
        ActivityState::Idle => Some(idle_pose(slot, desk, layout, elapsed)),
    }
}

fn idle_pose(slot: &AgentSlot, desk: Point, layout: &Layout, elapsed_ms: u64) -> Pose {
    let cycle_ms = cycle_ms_for(slot.agent_id);
    let cycle_n = elapsed_ms / cycle_ms;
    let phase_t = elapsed_ms % cycle_ms;

    // XOR cycle_n so the same agent picks a (typically) different waypoint
    // each loop. Identical agents on identical cycles still match — only the
    // cycle number changes the choice.
    let wp_idx =
        ((slot.agent_id.raw() ^ cycle_n) as usize) % layout.waypoints.len();
    let wp = layout.waypoints[wp_idx];

    let seated_end = cycle_ms * PHASE_SEATED_FRAC / 1000;
    let walk_out_end = cycle_ms * PHASE_WALK_OUT_FRAC / 1000;
    let at_wp_end = cycle_ms * PHASE_AT_WAYPOINT_FRAC / 1000;

    if phase_t < seated_end {
        Pose::SeatedIdle
    } else if phase_t < walk_out_end {
        let span = walk_out_end - seated_end;
        let t = ((phase_t - seated_end) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        Pose::Walking { from: desk, to: wp, t_x1000: t, frame }
    } else if phase_t < at_wp_end {
        Pose::StandingAtWaypoint { wp: wp_idx }
    } else {
        let span = cycle_ms - at_wp_end;
        let t = ((phase_t - at_wp_end) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        Pose::Walking { from: wp, to: desk, t_x1000: t, frame }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;
    use ascii_agents_core::source::Activity;

    fn slot(state: ActivityState, age_ms: u64) -> (AgentSlot, SystemTime) {
        let id = AgentId::from_transcript_path("/p/a.jsonl");
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let now = started + Duration::from_millis(age_ms);
        let s = AgentSlot {
            agent_id: id,
            source: "claude-code".into(),
            session_id: "abc".into(),
            cwd: PathBuf::from("/repo"),
            label: "cc".into(),
            state,
            state_started_at: started,
            desk_index: 0,
        };
        (s, now)
    }

    fn layout() -> Layout {
        Layout::compute(120, 96, 4).expect("fits")
    }

    fn typing() -> ActivityState {
        ActivityState::Active {
            activity: Activity::Typing,
            tool_use_id: Some("t".into()),
            detail: Some("Edit".into()),
        }
    }

    /// Phase boundary helper using the agent's actual cycle length.
    fn phases(agent_id: AgentId) -> (u64, u64, u64, u64) {
        let c = cycle_ms_for(agent_id);
        (
            c * PHASE_SEATED_FRAC / 1000,
            c * PHASE_WALK_OUT_FRAC / 1000,
            c * PHASE_AT_WAYPOINT_FRAC / 1000,
            c,
        )
    }

    #[test]
    fn active_state_is_seated_typing_with_cycling_frame() {
        let (s, now) = slot(typing(), 0);
        let l = layout();
        assert_eq!(derive(&s, now, &l), Some(Pose::SeatedTyping { frame: 0 }));
        let (s, now) = slot(typing(), TYPING_FRAME_MS);
        assert_eq!(derive(&s, now, &l), Some(Pose::SeatedTyping { frame: 1 }));
        let (s, now) = slot(typing(), TYPING_FRAME_MS * 2);
        assert_eq!(derive(&s, now, &l), Some(Pose::SeatedTyping { frame: 0 }));
    }

    #[test]
    fn waiting_state_is_standing_at_desk() {
        let (s, now) = slot(
            ActivityState::Waiting { reason: "perm".into() },
            5_000,
        );
        let l = layout();
        assert_eq!(derive(&s, now, &l), Some(Pose::StandingAtDesk));
    }

    #[test]
    fn idle_phase_0_is_seated_idle() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let (seated_end, _, _, _) = phases(test_slot.agent_id);
        let (s, now) = slot(ActivityState::Idle, seated_end - 1);
        let l = layout();
        assert_eq!(derive(&s, now, &l), Some(Pose::SeatedIdle));
    }

    #[test]
    fn idle_phase_1_is_walking_out() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let (seated_end, walk_out_end, _, _) = phases(test_slot.agent_id);
        let midpoint = seated_end + (walk_out_end - seated_end) / 2;
        let (s, now) = slot(ActivityState::Idle, midpoint);
        let l = layout();
        match derive(&s, now, &l).expect("pose") {
            Pose::Walking { t_x1000, frame, .. } => {
                assert!((400..=600).contains(&t_x1000), "t_x1000={t_x1000}");
                assert!(frame < WALKING_FRAMES);
            }
            other => panic!("expected Walking, got {other:?}"),
        }
    }

    #[test]
    fn idle_phase_2_is_standing_at_waypoint() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let (_, walk_out_end, at_wp_end, _) = phases(test_slot.agent_id);
        let midpoint = walk_out_end + (at_wp_end - walk_out_end) / 2;
        let (s, now) = slot(ActivityState::Idle, midpoint);
        let l = layout();
        match derive(&s, now, &l).expect("pose") {
            Pose::StandingAtWaypoint { wp } => assert!(wp < l.waypoints.len()),
            other => panic!("expected StandingAtWaypoint, got {other:?}"),
        }
    }

    #[test]
    fn idle_phase_3_is_walking_back() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let (_, _, at_wp_end, cycle) = phases(test_slot.agent_id);
        let midpoint = at_wp_end + (cycle - at_wp_end) / 2;
        let (s, now) = slot(ActivityState::Idle, midpoint);
        let l = layout();
        match derive(&s, now, &l).expect("pose") {
            Pose::Walking { t_x1000, .. } => {
                assert!((400..=600).contains(&t_x1000));
            }
            other => panic!("expected Walking, got {other:?}"),
        }
    }

    #[test]
    fn idle_cycle_loops_after_one_cycle() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let cycle = cycle_ms_for(test_slot.agent_id);
        let (s_early, now_early) = slot(ActivityState::Idle, 1_000);
        let (s_loop, now_loop) = slot(ActivityState::Idle, 1_000 + cycle);
        let l = layout();
        // Same phase within cycle, BUT waypoint differs because cycle_n changed.
        // Only the destination changes — the phase itself is the same kind.
        let e = derive(&s_early, now_early, &l).expect("e");
        let lp = derive(&s_loop, now_loop, &l).expect("loop");
        assert!(
            matches!((e, lp), (Pose::SeatedIdle, Pose::SeatedIdle)),
            "1s into any cycle should be SeatedIdle. got early={e:?} loop={lp:?}"
        );
    }

    #[test]
    fn derive_returns_none_when_desk_index_out_of_range() {
        let (mut s, now) = slot(ActivityState::Idle, 0);
        s.desk_index = 999;
        assert!(derive(&s, now, &layout()).is_none());
    }

    #[test]
    fn cycle_ms_for_varies_across_agents() {
        // Sanity: a handful of different agent ids should not all map to the
        // same cycle length.
        let ids: Vec<AgentId> = (0..10)
            .map(|i| AgentId::from_transcript_path(&format!("/p/{i}.jsonl")))
            .collect();
        let cycles: std::collections::HashSet<u64> =
            ids.iter().map(|id| cycle_ms_for(*id)).collect();
        assert!(
            cycles.len() >= 3,
            "expected multiple distinct cycle lengths, got {cycles:?}"
        );
        for c in &cycles {
            assert!(
                *c >= WANDER_CYCLE_BASE_MS
                    && *c < WANDER_CYCLE_BASE_MS + WANDER_CYCLE_RANGE_MS
            );
        }
    }

    #[test]
    fn waypoint_choice_changes_across_cycles_for_same_agent() {
        let l = layout();
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let cycle = cycle_ms_for(test_slot.agent_id);
        let (_, walk_out_end, at_wp_end, _) = phases(test_slot.agent_id);
        let mid_at_wp = walk_out_end + (at_wp_end - walk_out_end) / 2;

        // Capture the waypoint chosen over the first 4 cycles.
        let mut chosen = std::collections::HashSet::new();
        for n in 0..4u64 {
            let t = n * cycle + mid_at_wp;
            let (s, now) = slot(ActivityState::Idle, t);
            if let Some(Pose::StandingAtWaypoint { wp }) = derive(&s, now, &l) {
                chosen.insert(wp);
            }
        }
        assert!(
            chosen.len() >= 2,
            "waypoint should vary across cycles, got {chosen:?}"
        );
    }
}
