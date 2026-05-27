//! Per-agent cache of recolored sprite frames.
//!
//! `recolor_frame` clones a Frame and rewrites pixels — cheap per call,
//! but called once per agent per render tick (~30fps). With N agents the
//! per-second work scales linearly. Since shirt+hair colors are deterministic
//! from agent_id, the recolored frame is stable across the agent's lifetime
//! and can be cached.

use std::collections::HashMap;

use pixtuoid_core::sprite::{Frame, Rgb};
use pixtuoid_core::{AgentId, SceneState};

#[derive(Default)]
pub struct FrameCache {
    entries: HashMap<(AgentId, &'static str, usize, bool, Option<Rgb>), Frame>,
}

impl FrameCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lookup a cached frame, or compute and insert one and return a borrow.
    /// `anim_name` should be a `&'static str` so the key is cheap. `flip_x`
    /// is part of the key so mirrored (left-facing) walkers cache separately;
    /// `glow_tint` is part of the key so each monitor-glow color variant
    /// caches separately from the base variant.
    #[allow(clippy::too_many_arguments)]
    pub fn get_or_make<F: FnOnce() -> Frame>(
        &mut self,
        agent_id: AgentId,
        anim_name: &'static str,
        frame_idx: usize,
        flip_x: bool,
        glow_tint: Option<Rgb>,
        compute: F,
    ) -> &Frame {
        self.entries
            .entry((agent_id, anim_name, frame_idx, flip_x, glow_tint))
            .or_insert_with(compute)
    }

    /// Drop cached frames for agents no longer present in the scene.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.entries
            .retain(|(id, _, _, _, _), _| scene.agents.contains_key(id));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
