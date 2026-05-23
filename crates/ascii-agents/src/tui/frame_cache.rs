//! Per-agent cache of recolored sprite frames.
//!
//! `recolor_frame` clones a Frame and rewrites pixels — cheap per call,
//! but called once per agent per render tick (~30fps). With N agents the
//! per-second work scales linearly. Since shirt+hair colors are deterministic
//! from agent_id, the recolored frame is stable across the agent's lifetime
//! and can be cached.

use std::collections::HashMap;

use ascii_agents_core::sprite::Frame;
use ascii_agents_core::{AgentId, SceneState};

#[derive(Default)]
pub struct FrameCache {
    entries: HashMap<(AgentId, &'static str, usize, bool, bool), Frame>,
}

impl FrameCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lookup a cached frame, or compute and insert one and return a borrow.
    /// `anim_name` should be a `&'static str` so the key is cheap. `flip_x`
    /// is part of the key so mirrored (left-facing) walkers cache separately;
    /// `face_lit` is part of the key so the monitor-glow skin tint variant
    /// caches separately from the base variant.
    #[allow(clippy::too_many_arguments)]
    pub fn get_or_make<F: FnOnce() -> Frame>(
        &mut self,
        agent_id: AgentId,
        anim_name: &'static str,
        frame_idx: usize,
        flip_x: bool,
        face_lit: bool,
        compute: F,
    ) -> &Frame {
        self.entries
            .entry((agent_id, anim_name, frame_idx, flip_x, face_lit))
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
