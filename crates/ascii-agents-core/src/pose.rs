//! Pure state → pose derivation. Lives in core so non-TUI renderers
//! (snapshot tooling, future PNG/GIF capture, web canvas) get identical
//! pose semantics without depending on the binary crate.
//!
//! `derive(slot, now, layout)` is a function of the snapshot inputs only —
//! no routing, no per-frame history. The routed variant (which composes
//! against a `Router` and a `PoseHistory` cache) is `tui::pose` on the
//! binary side, since the router is terminal-rendering-adjacent.
//!
//! Variation knobs:
//!  * `cycle_ms_for(agent_id)` — per-agent wander cycle length so the office
//!    stops feeling clockwork-synchronized. Range 7..13s.
//!  * Waypoint choice XORs `agent_id` with the current cycle number, so each
//!    cycle the same agent picks a (likely) different waypoint.

use std::time::{Duration, SystemTime};

use crate::layout::{Bounds, Point, SceneLayout, WaypointKind};
use crate::state::{ActivityState, AgentSlot};
use crate::AgentId;

/// How long after the last event an Idle agent stays in the "thinking"
/// pose (seated, awake, no z's) before entering the wander/sleep cycle.
/// 20s covers typical CC thinking pauses between tool bursts.
const THINKING_WINDOW_SECS: u64 = 20;

/// Base cycle length. Each agent's actual cycle = base + per-agent jitter.
pub const WANDER_CYCLE_BASE_MS: u64 = 7_000;
/// Maximum extra time added per agent — jitter range is `[0, RANGE)`.
pub const WANDER_CYCLE_RANGE_MS: u64 = 6_000;
/// Phase fractions of a cycle (×1000 to stay in integer math).
const PHASE_SEATED_FRAC: u64 = 389; // 0..389/1000
const PHASE_WALK_OUT_FRAC: u64 = 556; // 389..556/1000
const PHASE_AT_WAYPOINT_FRAC: u64 = 833; // 556..833/1000
                                         // walk-back is 833..1000/1000.

/// Frame-cycle period for animated poses.
pub const TYPING_FRAME_MS: u64 = 140;
pub const WALKING_FRAME_MS: u64 = 220;
pub const TYPING_FRAMES: usize = 2;
pub const WALKING_FRAMES: usize = 2;

/// Milliseconds of one-shot walk-from-door entry animation. Overrides the
/// normal state→pose mapping for any newly-spawned agent so SessionStart
/// reads as "someone just walked into the office".
pub const ENTRY_ANIMATION_MS: u64 = 4000;

/// Deterministic wander-cycle length for one agent. Each agent picks a
/// different speed so walkers don't move in lockstep.
pub fn cycle_ms_for(agent_id: AgentId) -> u64 {
    WANDER_CYCLE_BASE_MS + (agent_id.raw() >> 16) % WANDER_CYCLE_RANGE_MS
}

/// Per-agent wander personality derived from the agent's id hash.
/// Controls how often the agent leaves their desk and whether they prefer
/// aimless wandering vs heading to a named lounge waypoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Personality {
    /// Probability (in percent) that this agent takes a trip on any given
    /// cycle. Range: 10..=50. Restless agents wander more.
    pub trip_chance_pct: u8,
    /// Probability (in percent) that a trip is aimless wander vs heading to
    /// a lounge waypoint. Range: 0..=70.
    pub aimless_pref_pct: u8,
}

pub fn personality_for(agent_id: AgentId) -> Personality {
    let h = agent_id.raw();
    Personality {
        trip_chance_pct: (10 + (h % 41)) as u8,  // 10..=50
        aimless_pref_pct: ((h >> 8) % 71) as u8, // 0..=70
    }
}

/// Deterministic per-(agent, cycle) decision: does this agent take a
/// wander trip on this cycle, or stay seated? Trip frequency is driven by
/// per-agent Personality so different agents wander at different rates.
pub fn takes_trip(agent_id: AgentId, cycle_n: u64) -> bool {
    let p = personality_for(agent_id);
    let mix = agent_id.raw() ^ cycle_n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (mix % 100) < p.trip_chance_pct as u64
}

/// Per-(agent, cycle) decision: when the agent takes a trip, is it an
/// aimless wander (random walkway point) or a directed visit to a named
/// waypoint? Used by `idle_pose` AND by the snapshot example to find
/// agent_ids whose cycle deterministically lands at a target waypoint.
pub fn is_aimless_cycle(agent_id: AgentId, cycle_n: u64) -> bool {
    let p = personality_for(agent_id);
    let type_mix = agent_id.raw() ^ cycle_n.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    (type_mix % 100) < p.aimless_pref_pct as u64
}

/// Per-(agent, cycle) waypoint index. Only meaningful when `takes_trip` is
/// true AND `is_aimless_cycle` is false. Returns 0 if `num_waypoints` is 0.
pub fn waypoint_index_for_cycle(agent_id: AgentId, cycle_n: u64, num_waypoints: usize) -> usize {
    if num_waypoints == 0 {
        return 0;
    }
    ((agent_id.raw() ^ cycle_n) as usize) % num_waypoints
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    SeatedIdle,
    /// Seated at desk, awake but not typing. Used when the agent
    /// recently finished a tool call and the LLM is likely thinking.
    SeatedThinking,
    SeatedTyping {
        frame: usize,
    },
    StandingAtDesk,
    /// At a lounge waypoint. Concrete render depends on the kind:
    ///   Couch    → sit on couch sprite
    ///   Coffee   → standing + holding-coffee sprite
    ///   Others   → plain standing
    AtWaypoint {
        wp: usize,
        kind: WaypointKind,
    },
    Walking {
        from: Point,
        to: Point,
        t_x1000: u16,
        frame: usize,
    },
    /// Standing at a random walkway point (not at any waypoint). The dest field
    /// is the buf-pixel target the agent walked to. Used by aimless wander.
    AimlessAt {
        dest: Point,
    },
}

/// Returns `None` if the slot's desk_index is out of range for `layout`.
///
/// Priority chain (first match wins, walks down):
///   1. **Exit override** — slot has `exiting_at` set → Walking to door.
///      Once the exit window passes, returns `None` (slot will be GC'd).
///   2. **Entry override** — `now - created_at < ENTRY_ANIMATION_MS` →
///      Walking from door to desk. Plays for the first 4 s of the slot's
///      life regardless of state changes.
///   3. **State-driven pose** — Active → SeatedTyping, Waiting →
///      StandingAtDesk, Idle → idle_pose (which itself decides between
///      SeatedIdle / Walking / AtWaypoint / AimlessAt based on the
///      wander state machine).
pub fn derive(slot: &AgentSlot, now: SystemTime, layout: &SceneLayout) -> Option<Pose> {
    let desk = *layout.home_desks.get(slot.desk_index)?;

    // Exit takes priority — once SessionEnd fires we always walk to the
    // door regardless of entry-window or normal state. Use door_threshold
    // (on-floor point below the door) as the walk target so the character
    // doesn't paint through the wall trim.
    if let (Some(exit_time), Some(target)) = (slot.exiting_at, layout.door_threshold) {
        let since_exit = now
            .duration_since(exit_time)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        if since_exit < ENTRY_ANIMATION_MS {
            let t = (since_exit * 1000 / ENTRY_ANIMATION_MS).min(1000) as u16;
            let frame = ((since_exit / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
            return Some(Pose::Walking {
                from: Point {
                    x: desk.x + 6,
                    y: desk.y + 4,
                },
                to: target,
                t_x1000: t,
                frame,
            });
        }
        // Past exit window: nothing to render, slot will be GC'd shortly.
        return None;
    }

    // Entry animation overrides everything for the first ENTRY_ANIMATION_MS
    // after creation — agent walks in from the door threshold to their desk.
    // Target is offset (+6, +4) from the desk top-left so the walk ends at
    // the seated anchor position, not inside the desk obstacle. Without this
    // the A* router detours around the desk and always approaches from one side.
    if let Some(from) = layout.door_threshold {
        let since_spawn = now
            .duration_since(slot.created_at)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        if since_spawn < ENTRY_ANIMATION_MS {
            let t = (since_spawn * 1000 / ENTRY_ANIMATION_MS).min(1000) as u16;
            let frame = ((since_spawn / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
            return Some(Pose::Walking {
                from,
                to: Point {
                    x: desk.x + 6,
                    y: desk.y + 4,
                },
                t_x1000: t,
                frame,
            });
        }
    }

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
        ActivityState::Idle => {
            let was_active = slot.last_event_at > slot.created_at;
            let since_last_event = now
                .duration_since(slot.last_event_at)
                .unwrap_or(Duration::ZERO)
                .as_secs();
            if was_active && since_last_event < THINKING_WINDOW_SECS {
                Some(Pose::SeatedThinking)
            } else {
                Some(idle_pose(slot, desk, layout, elapsed))
            }
        }
    }
}

/// Pick an aimless wander destination using weighted zones. Each zone
/// gets a "vibe weight" — window-viewing strip + pantry are highest
/// because that's where people naturally drift during breaks; corridor
/// and cubicle aisles are incidental; meeting room is rare. After
/// picking a zone (weighted random), rejection-sample 32 points
/// within the zone for a walkable pixel. Falls back to a randomised
/// point along the corridor if every probe fails.
fn pick_aimless_dest(layout: &SceneLayout, seed: u64) -> Point {
    // Build the zone list. Use small rectangles for "window strip"
    // (top of cubicle band, where viewing-the-city makes sense) and
    // larger bounding boxes for the rooms / corridor. Zones can
    // overlap — the walkable mask filters out non-walkable picks
    // either way.
    let window_strip = Bounds {
        x: layout.cubicle_band.x,
        y: layout.top_margin + 1,
        width: layout.cubicle_band.width,
        height: 10,
    };
    let zones: [(Bounds, u16); 5] = [
        // Stretch + look-at-the-view at the top of the cubicle band.
        (window_strip, 30),
        // Pantry interior — snack break, coffee, chat.
        (layout.pantry_room.unwrap_or(window_strip), 25),
        // Main corridor — incidental traffic.
        (layout.corridor.unwrap_or(layout.walkway), 20),
        // Cubicle band (pod aisles) — within own area, stretching.
        (layout.cubicle_band, 15),
        // Meeting room — occasional drift-in.
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
    // Fallback — randomised point along the corridor's x-range so
    // multiple fallback agents spread out instead of clustering.
    let c = layout.corridor.unwrap_or(layout.walkway);
    let x_jitter = (seed as u16) % c.width.max(1);
    Point {
        x: c.x + x_jitter,
        y: c.y + c.height / 2,
    }
}

fn idle_pose(slot: &AgentSlot, desk: Point, layout: &SceneLayout, elapsed_ms: u64) -> Pose {
    let cycle_ms = cycle_ms_for(slot.agent_id);
    let cycle_n = elapsed_ms / cycle_ms;
    let phase_t = elapsed_ms % cycle_ms;

    if !takes_trip(slot.agent_id, cycle_n) || layout.waypoints.is_empty() {
        return Pose::SeatedIdle;
    }

    // Per-cycle "trip type" roll. Personality.aimless_pref_pct shifts the
    // mix between lounge waypoint and aimless wander.
    let aimless = is_aimless_cycle(slot.agent_id, cycle_n);

    // Phase boundaries.
    let seated_end = cycle_ms * PHASE_SEATED_FRAC / 1000;
    let walk_out_end = cycle_ms * PHASE_WALK_OUT_FRAC / 1000;
    let at_wp_end = cycle_ms * PHASE_AT_WAYPOINT_FRAC / 1000;

    // Destination: lounge waypoint OR aimless point.
    let (dest, at_dest_pose): (Point, Pose) = if aimless {
        // Weighted-zone aimless wander. Instead of uniformly sampling
        // anywhere in the buffer (which clusters at the fallback
        // because most cubicle pixels are obstacles), pick a ZONE by
        // weight first — window-viewing strip, pantry, corridor,
        // meeting room — then rejection-sample within that zone.
        // Weights tune the "vibe" of where agents drift: window
        // strip and pantry get the highest weight so the office
        // feels alive (people stretching at windows, grabbing
        // coffee), corridor/cubicle/meeting are more incidental.
        let seed = slot.agent_id.raw() ^ cycle_n.wrapping_mul(0xd1b5_4a32_d192_ed03);
        let p = pick_aimless_dest(layout, seed);
        (p, Pose::AimlessAt { dest: p })
    } else {
        let wp_idx = waypoint_index_for_cycle(slot.agent_id, cycle_n, layout.waypoints.len());
        let wp = layout.waypoints[wp_idx];
        (
            wp.pos,
            Pose::AtWaypoint {
                wp: wp_idx,
                kind: wp.kind,
            },
        )
    };

    if phase_t < seated_end {
        Pose::SeatedIdle
    } else if phase_t < walk_out_end {
        let span = walk_out_end - seated_end;
        let t = ((phase_t - seated_end) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        Pose::Walking {
            from: desk,
            to: dest,
            t_x1000: t,
            frame,
        }
    } else if phase_t < at_wp_end {
        at_dest_pose
    } else {
        let span = cycle_ms - at_wp_end;
        let t = ((phase_t - at_wp_end) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        Pose::Walking {
            from: dest,
            to: desk,
            t_x1000: t,
            frame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Activity;
    use std::path::PathBuf;
    use std::time::Duration;

    fn slot(state: ActivityState, age_ms: u64) -> (AgentSlot, SystemTime) {
        let id = AgentId::from_transcript_path("/p/a.jsonl");
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let now = started + Duration::from_millis(age_ms);
        // created_at well before `started` so the entry-animation override
        // doesn't fire in tests that probe regular state→pose mapping.
        let created = started - Duration::from_secs(60);
        let s = AgentSlot {
            agent_id: id,
            source: std::sync::Arc::from("claude-code"),
            session_id: std::sync::Arc::from("abc"),
            cwd: std::sync::Arc::from(PathBuf::from("/repo").as_path()),
            label: std::sync::Arc::from("cc"),
            state,
            state_started_at: started,
            created_at: created,
            last_event_at: created,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: 0,
            tool_call_count: 0,
            active_ms: 0,
        };
        (s, now)
    }

    fn layout() -> SceneLayout {
        SceneLayout::compute(120, 96, 4).expect("fits")
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

    /// Find the lowest cycle index where the agent decides to take a trip.
    /// Pose tests that probe walking/waypoint phases need a known trip cycle
    /// to drive the elapsed offset off of.
    fn first_trip_cycle(agent_id: AgentId) -> u64 {
        (0u64..1000)
            .find(|n| takes_trip(agent_id, *n))
            .expect("agent should trip within first 1000 cycles")
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
            ActivityState::Waiting {
                reason: "perm".into(),
            },
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
        let cycle = cycle_ms_for(test_slot.agent_id);
        let trip_n = first_trip_cycle(test_slot.agent_id);
        let midpoint = trip_n * cycle + seated_end + (walk_out_end - seated_end) / 2;
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
    fn idle_phase_2_is_at_waypoint() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let (_, walk_out_end, at_wp_end, _) = phases(test_slot.agent_id);
        let cycle = cycle_ms_for(test_slot.agent_id);
        let trip_n = first_trip_cycle(test_slot.agent_id);
        let midpoint = trip_n * cycle + walk_out_end + (at_wp_end - walk_out_end) / 2;
        let (s, now) = slot(ActivityState::Idle, midpoint);
        let l = layout();
        // Trip cycles land at either a named waypoint or an aimless point,
        // depending on per-agent personality.
        match derive(&s, now, &l).expect("pose") {
            Pose::AtWaypoint { wp, .. } => assert!(wp < l.waypoints.len()),
            Pose::AimlessAt { .. } => {}
            other => panic!("expected AtWaypoint or AimlessAt, got {other:?}"),
        }
    }

    #[test]
    fn idle_phase_3_is_walking_back() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let (_, _, at_wp_end, cycle) = phases(test_slot.agent_id);
        let trip_n = first_trip_cycle(test_slot.agent_id);
        let midpoint = trip_n * cycle + at_wp_end + (cycle - at_wp_end) / 2;
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
    fn takes_trip_fires_roughly_30_percent_of_cycles() {
        let id = AgentId::from_transcript_path("/p/sample.jsonl");
        let trips = (0u64..1000).filter(|n| takes_trip(id, *n)).count();
        // Per-agent trip chance varies 10..=50%, so across 1000 cycles the
        // count is bounded by those extremes with reasonable tolerance.
        assert!(
            (50..=550).contains(&trips),
            "expected 50..=550 trips out of 1000 (personality-driven), got {trips}"
        );
    }

    #[test]
    fn personality_varies_across_agents() {
        let ps: Vec<Personality> = (0..20)
            .map(|i| personality_for(AgentId::from_transcript_path(&format!("/p/{i}.jsonl"))))
            .collect();
        let trip_chances: std::collections::HashSet<u8> =
            ps.iter().map(|p| p.trip_chance_pct).collect();
        assert!(
            trip_chances.len() >= 5,
            "expected variance in trip_chance_pct"
        );
        for p in &ps {
            assert!((10..=50).contains(&p.trip_chance_pct));
            assert!(p.aimless_pref_pct <= 70);
        }
    }

    #[test]
    fn non_trip_cycle_is_seated_idle_throughout() {
        let (test_slot, _) = slot(ActivityState::Idle, 0);
        let id = test_slot.agent_id;
        let cycle = cycle_ms_for(id);
        // Find a cycle where the agent does NOT trip.
        let stay_n = (0u64..100)
            .find(|n| !takes_trip(id, *n))
            .expect("agent should have a non-trip cycle");
        // Sample 10 points across that cycle; all should be SeatedIdle.
        for k in 0..10 {
            let t = stay_n * cycle + (k * cycle / 10);
            let (s, now) = slot(ActivityState::Idle, t);
            let l = layout();
            assert_eq!(
                derive(&s, now, &l),
                Some(Pose::SeatedIdle),
                "t={t} should be SeatedIdle on non-trip cycle"
            );
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
    fn entry_animation_overrides_normal_pose_for_first_4s() {
        let id = AgentId::from_transcript_path("/p/entry.jsonl");
        let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // created_at == now0, so since_spawn = 1500ms at probe time
        let s = AgentSlot {
            agent_id: id,
            source: std::sync::Arc::from("claude-code"),
            session_id: std::sync::Arc::from("abc"),
            cwd: std::sync::Arc::from(PathBuf::from("/repo").as_path()),
            label: std::sync::Arc::from("cc"),
            state: ActivityState::Idle,
            state_started_at: now0,
            created_at: now0,
            last_event_at: now0,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: 0,
            tool_call_count: 0,
            active_ms: 0,
        };
        let probe = now0 + Duration::from_millis(1500);
        let l = layout();
        match derive(&s, probe, &l).expect("pose") {
            Pose::Walking { t_x1000, .. } => {
                // 1500/4000 = 0.375 → t_x1000 ~= 375
                assert!((300..=450).contains(&t_x1000), "t_x1000={t_x1000}");
            }
            other => panic!("expected Walking entry, got {other:?}"),
        }
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
                *c >= WANDER_CYCLE_BASE_MS && *c < WANDER_CYCLE_BASE_MS + WANDER_CYCLE_RANGE_MS
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

        // Capture destinations chosen across many cycles. Trip cycles produce
        // either AtWaypoint or AimlessAt — both contribute to destination
        // variety, so we count distinct destination x coords.
        let mut dest_xs = std::collections::HashSet::new();
        for n in 0..50u64 {
            let t = n * cycle + mid_at_wp;
            let (s, now) = slot(ActivityState::Idle, t);
            match derive(&s, now, &l) {
                Some(Pose::AtWaypoint { wp, .. }) => {
                    dest_xs.insert(l.waypoints[wp].pos.x);
                }
                Some(Pose::AimlessAt { dest }) => {
                    dest_xs.insert(dest.x);
                }
                _ => {}
            }
        }
        assert!(
            dest_xs.len() >= 2,
            "destination should vary across cycles, got {dest_xs:?}"
        );
    }

    #[test]
    fn idle_within_thinking_window_returns_seated_thinking() {
        let (mut s, now) = slot(ActivityState::Idle, 5_000);
        s.last_event_at = now - Duration::from_secs(5);
        let l = layout();
        let p = derive(&s, now, &l).unwrap();
        assert_eq!(p, Pose::SeatedThinking);
    }

    #[test]
    fn idle_past_thinking_window_returns_idle_pose() {
        let (mut s, now) = slot(ActivityState::Idle, 25_000);
        s.last_event_at = now - Duration::from_secs(25);
        let l = layout();
        let p = derive(&s, now, &l).unwrap();
        assert_ne!(p, Pose::SeatedThinking);
    }

    #[test]
    fn freshly_spawned_idle_skips_thinking() {
        let (s, now) = slot(ActivityState::Idle, 5_000);
        assert_eq!(s.last_event_at, s.created_at);
        let l = layout();
        let p = derive(&s, now, &l).unwrap();
        assert_ne!(p, Pose::SeatedThinking);
    }
}
