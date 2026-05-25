//! Multi-floor office partitioning.
//!
//! When more agents are active than `max_desks` can seat on a single floor,
//! the scene is split into multiple floors. This module provides the pure
//! arithmetic (which floor does desk N belong to? how many floors exist?)
//! and the per-floor rendering context (`FloorCtx`) so each floor owns its
//! own router, overlay, pose history, and frame cache.

use std::time::{Duration, SystemTime};

use ascii_agents_core::state::{AgentSlot, SceneState};
use ascii_agents_core::walkable::OccupancyOverlay;

use crate::tui::frame_cache::FrameCache;
use crate::tui::pathfind::{AStarRouter, Router};
use crate::tui::pose::PoseHistory;

/// Hard ceiling on navigable floors — keeps memory bounded even if
/// hundreds of agents show up.
pub const MAX_FLOORS: usize = 5;

/// Per-floor rendering state. Each floor gets its own pathfinder,
/// occupancy overlay, pose history, and recolored-frame cache so floors
/// are fully independent.
pub struct FloorCtx {
    pub router: AStarRouter,
    pub overlay: OccupancyOverlay,
    pub history: PoseHistory,
    pub cache: FrameCache,
}

impl Default for FloorCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl FloorCtx {
    pub fn new() -> Self {
        Self {
            router: AStarRouter::new(),
            overlay: OccupancyOverlay::new(),
            history: PoseHistory::new(),
            cache: FrameCache::new(),
        }
    }

    /// Drop all cached state — call on terminal resize or layout change.
    pub fn flush_caches(&mut self) {
        self.router.invalidate();
        self.overlay.clear();
        self.history = PoseHistory::new();
        self.cache = FrameCache::new();
    }
}

/// Animated floor-switch transition.
pub struct FloorTransition {
    pub from_floor: usize,
    pub to_floor: usize,
    pub started_at: SystemTime,
    pub duration_ms: u64,
}

const TRANSITION_DURATION_MS: u64 = 900;

impl FloorTransition {
    pub fn new(from: usize, to: usize, now: SystemTime) -> Self {
        Self {
            from_floor: from,
            to_floor: to,
            started_at: now,
            duration_ms: TRANSITION_DURATION_MS,
        }
    }

    /// Progress ratio 0.0 → 1.0 with ease-in-out curve.
    pub fn t(&self, now: SystemTime) -> f32 {
        let elapsed = now
            .duration_since(self.started_at)
            .unwrap_or(Duration::ZERO)
            .as_millis() as f32;
        let linear = (elapsed / self.duration_ms as f32).clamp(0.0, 1.0);
        ease_out(linear)
    }

    pub fn is_done(&self, now: SystemTime) -> bool {
        self.t(now) >= 1.0
    }
}

// ---------------------------------------------------------------------------
// Pure arithmetic helpers
// ---------------------------------------------------------------------------

/// Which floor does `desk_index` belong to?
/// Quadratic ease-out: fast start, smooth deceleration.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}

pub fn floor_of(desk_index: usize, desks_per_floor: usize) -> usize {
    if desks_per_floor == 0 {
        return 0;
    }
    desk_index / desks_per_floor
}

/// Local desk offset within the floor (for layout remapping).
pub fn floor_local_desk(desk_index: usize, desks_per_floor: usize) -> usize {
    if desks_per_floor == 0 {
        return 0;
    }
    desk_index % desks_per_floor
}

/// How many floors are needed to seat all agents?
pub fn num_floors(scene: &SceneState) -> usize {
    if scene.agents.is_empty() || scene.max_desks == 0 {
        return 1;
    }
    let max_idx = scene
        .agents
        .values()
        .map(|a| a.desk_index)
        .max()
        .unwrap_or(0);
    max_idx / scene.max_desks + 1
}

/// Extract agents belonging to `floor_idx`, remapping their `desk_index`
/// into the `[0..desks_per_floor)` range so the layout engine sees a
/// self-contained floor. Returns `(agents, desks_per_floor)`.
pub fn build_floor_scene(scene: &SceneState, floor_idx: usize) -> (Vec<AgentSlot>, usize) {
    let dpf = scene.max_desks;
    let lo = floor_idx * dpf;
    let hi = lo + dpf;
    let agents: Vec<AgentSlot> = scene
        .agents
        .values()
        .filter(|a| a.desk_index >= lo && a.desk_index < hi)
        .map(|a| {
            let mut slot = a.clone();
            slot.desk_index = a.desk_index - lo;
            slot
        })
        .collect();
    (agents, dpf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ascii_agents_core::id::AgentId;
    use ascii_agents_core::state::ActivityState;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    fn make_scene(n: usize, max_desks: usize) -> SceneState {
        let mut s = SceneState::new(max_desks);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        for i in 0..n {
            let id = AgentId::from_transcript_path(&format!("/p/{i}.jsonl"));
            s.agents.insert(
                id,
                AgentSlot {
                    agent_id: id,
                    source: Arc::from("cc"),
                    session_id: Arc::from(format!("s{i}").as_str()),
                    cwd: Arc::from(Path::new("/repo")),
                    label: Arc::from(format!("a{i}").as_str()),
                    state: ActivityState::Idle,
                    state_started_at: now,
                    created_at: now,
                    last_event_at: now,
                    exiting_at: None,
                    pending_idle_at: None,
                    desk_index: i,
                    tool_call_count: 0,
                    active_ms: 0,
                    unknown_cwd: false,
                    parent_id: None,
                },
            );
        }
        s
    }

    #[test]
    fn floor_of_maps_desk_to_floor() {
        assert_eq!(floor_of(0, 16), 0);
        assert_eq!(floor_of(15, 16), 0);
        assert_eq!(floor_of(16, 16), 1);
        assert_eq!(floor_of(31, 16), 1);
        assert_eq!(floor_of(32, 16), 2);
    }

    #[test]
    fn floor_local_desk_remaps_to_floor_range() {
        assert_eq!(floor_local_desk(0, 16), 0);
        assert_eq!(floor_local_desk(16, 16), 0);
        assert_eq!(floor_local_desk(17, 16), 1);
        assert_eq!(floor_local_desk(31, 16), 15);
    }

    #[test]
    fn num_floors_with_overflow() {
        let scene = make_scene(20, 16);
        assert_eq!(num_floors(&scene), 2);
    }

    #[test]
    fn num_floors_exact_fit() {
        let scene = make_scene(16, 16);
        assert_eq!(num_floors(&scene), 1);
    }

    #[test]
    fn num_floors_empty() {
        let scene = make_scene(0, 16);
        assert_eq!(num_floors(&scene), 1);
    }

    #[test]
    fn build_floor_scene_filters_and_remaps() {
        let scene = make_scene(20, 16);

        let (floor0, dpf0) = build_floor_scene(&scene, 0);
        assert_eq!(dpf0, 16);
        assert_eq!(floor0.len(), 16);
        for a in &floor0 {
            assert!(
                a.desk_index < 16,
                "desk_index {} out of range",
                a.desk_index
            );
        }

        let (floor1, dpf1) = build_floor_scene(&scene, 1);
        assert_eq!(dpf1, 16);
        assert_eq!(floor1.len(), 4);
        let mut indices: Vec<usize> = floor1.iter().map(|a| a.desk_index).collect();
        indices.sort();
        assert_eq!(indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn transition_t_progresses_linearly() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let tr = FloorTransition::new(0, 1, start);

        assert!((tr.t(start) - 0.0).abs() < f32::EPSILON);

        let mid = start + Duration::from_millis(250);
        assert!((tr.t(mid) - 0.5).abs() < 0.01);

        let end = start + Duration::from_millis(500);
        assert!((tr.t(end) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn transition_t_clamps_past_duration() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let tr = FloorTransition::new(0, 1, start);

        let past = start + Duration::from_millis(1000);
        assert!((tr.t(past) - 1.0).abs() < f32::EPSILON);
        assert!(tr.is_done(past));
    }
}
