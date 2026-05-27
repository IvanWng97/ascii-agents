//! `Renderer` trait impl that drives the half-block terminal TUI.
//!
//! Closes the v1 gap where production code called the free function
//! `draw_scene` directly, leaving the core `Renderer` trait exercised only
//! by `TestRenderer` in tests. `TuiRenderer` is the production impl: it
//! owns the cross-frame mutable state (`RgbBuffer`, `FrameCache`,
//! `AStarRouter`, `OccupancyOverlay`, `PoseHistory`) per floor and forwards
//! to `draw_scene`, which recomputes its own layout per frame from
//! `terminal.size()` because the user can resize at any time.

use std::time::SystemTime;

use anyhow::Result;
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::state::SceneState;
use ascii_agents_core::Renderer;
use ratatui::backend::Backend;
use ratatui::Terminal;

use ratatui::layout::Rect;

use crate::tui::floor::{build_floor_scene, num_floors, FloorCtx, FloorMeta, FloorTransition};
use crate::tui::layout::{Layout, Point, MAX_VISIBLE_DESKS};
use crate::tui::pathfind::Router;
use crate::tui::pet::PetKind;
use crate::tui::pixel_painter::{render_to_rgb_buffer, PixelCtx};
use crate::tui::renderer::{draw_scene, flush_buffer_to_term_at_offset, DrawCtx, PetState};

pub struct TuiRenderer<B: Backend<Error: Send + Sync + 'static>> {
    pub terminal: Terminal<B>,
    floor_bufs: Vec<RgbBuffer>,
    floor_ctxs: Vec<FloorCtx>,
    current_floor: usize,
    transition: Option<FloorTransition>,
    mouse_pos: Option<(u16, u16)>,
    pinned_agent: Option<ascii_agents_core::AgentId>,
    pub ticker: crate::tui::renderer::TickerQueue,
    theme: &'static crate::tui::theme::Theme,
    theme_picker: Option<usize>,
    cached_layout: Option<Layout>,
    active_pet: Option<PetState>,
    last_pet_pos: Option<(Point, &'static str, PetKind)>,
    enabled_pets: Vec<PetKind>,
    chitchat_state: std::collections::HashMap<(usize, usize), crate::tui::chitchat::ActiveChitchat>,
    /// Persistent set of agents that have visited the pantry and carry a
    /// coffee cup back to their desk. Replaces the stateless
    /// `has_desk_coffee` cycle-scanning. Cleared on agent exit.
    coffee_holders: std::collections::HashSet<ascii_agents_core::AgentId>,
    /// Timestamp when each agent first returned with coffee (for steam).
    coffee_fetched_at: std::collections::HashMap<ascii_agents_core::AgentId, SystemTime>,
}

impl<B: Backend<Error: Send + Sync + 'static>> TuiRenderer<B> {
    pub fn new(
        terminal: Terminal<B>,
        theme: &'static crate::tui::theme::Theme,
        enabled_pets: Vec<PetKind>,
    ) -> Self {
        Self {
            terminal,
            floor_bufs: vec![RgbBuffer::filled(0, 0, Rgb(0, 0, 0))],
            floor_ctxs: vec![FloorCtx::new()],
            current_floor: 0,
            transition: None,
            mouse_pos: None,
            pinned_agent: None,
            ticker: crate::tui::renderer::TickerQueue::new(),
            theme,
            theme_picker: None,
            cached_layout: None,
            active_pet: None,
            last_pet_pos: None,
            enabled_pets,
            chitchat_state: std::collections::HashMap::new(),
            coffee_holders: std::collections::HashSet::new(),
            coffee_fetched_at: std::collections::HashMap::new(),
        }
    }

    pub fn current_floor(&self) -> usize {
        self.current_floor
    }

    pub fn cached_layout(&self) -> Option<&Layout> {
        self.cached_layout.as_ref()
    }

    pub fn current_floor_seed(&self) -> u64 {
        let nf = self.floor_ctxs.len();
        FloorMeta::for_floor(self.current_floor, nf).floor_seed
    }

    pub fn transition(&self) -> Option<&FloorTransition> {
        self.transition.as_ref()
    }

    pub fn navigate_floor(&mut self, target: usize, now: SystemTime) {
        if target == self.current_floor || self.transition.is_some() {
            return;
        }
        self.set_pinned_agent(None);
        self.transition = Some(FloorTransition::new(self.current_floor, target, now));
    }

    pub fn cancel_transition(&mut self) {
        self.transition = None;
    }

    pub fn set_mouse_pos(&mut self, pos: Option<(u16, u16)>) {
        self.mouse_pos = pos;
    }

    pub fn pinned_agent(&self) -> Option<ascii_agents_core::AgentId> {
        self.pinned_agent
    }

    pub fn set_pinned_agent(&mut self, id: Option<ascii_agents_core::AgentId>) {
        self.pinned_agent = id;
    }

    pub fn buf(&self) -> &RgbBuffer {
        &self.floor_bufs[self.current_floor]
    }

    pub fn set_theme(&mut self, theme: &'static crate::tui::theme::Theme) {
        if !std::ptr::eq(self.theme, theme) {
            self.theme = theme;
            for ctx in &mut self.floor_ctxs {
                ctx.cache = crate::tui::frame_cache::FrameCache::new();
            }
        }
    }

    pub fn set_theme_picker(&mut self, picker: Option<usize>) {
        self.theme_picker = picker;
    }

    pub fn set_active_pet(&mut self, pet: Option<PetState>) {
        self.active_pet = pet;
    }

    pub fn active_pet_ref(&self) -> Option<&PetState> {
        self.active_pet.as_ref()
    }

    pub fn cached_pet_pos(&self) -> Option<(Point, &'static str, PetKind)> {
        self.last_pet_pos
    }

    /// Drop the cached frame entries for agents no longer in `scene`.
    /// Forwarded so the render loop doesn't reach into the cache directly.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        for ctx in &mut self.floor_ctxs {
            ctx.cache.evict_missing(scene);
        }
    }

    /// Invalidate all floors' router path caches. Call when the static
    /// walkable mask changes (terminal resize, floor capacity change).
    pub fn invalidate_routes(&mut self) {
        for ctx in &mut self.floor_ctxs {
            ctx.router.invalidate();
        }
    }
}

impl<B: Backend<Error: Send + Sync + 'static>> Renderer for TuiRenderer<B> {
    fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        // Auto-expire pet state.
        if self.active_pet.as_ref().is_some_and(|p| !p.is_active(now)) {
            self.active_pet = None;
        }

        self.ticker.update(scene);

        // Compute how many floors the current scene needs.
        let nf = num_floors(scene).min(crate::tui::floor::MAX_FLOORS);

        // Grow vectors if needed.
        while self.floor_bufs.len() < nf {
            self.floor_bufs.push(RgbBuffer::filled(0, 0, Rgb(0, 0, 0)));
        }
        while self.floor_ctxs.len() < nf {
            self.floor_ctxs.push(FloorCtx::new());
        }

        // Cancel transition if target floors no longer exist.
        if let Some(ref tr) = self.transition {
            if tr.from_floor >= nf || tr.to_floor >= nf {
                self.transition = None;
                self.cached_layout = None;
            }
        }

        // Complete transition if done.
        if let Some(ref tr) = self.transition {
            if tr.is_done(now) {
                self.current_floor = tr.to_floor;
                self.transition = None;
            }
        }

        // Clamp current_floor after transition completion.
        if self.current_floor >= nf {
            self.current_floor = nf.saturating_sub(1);
        }

        let floor_info = if nf > 1 {
            Some((self.current_floor + 1, nf))
        } else {
            None
        };

        // --- Transition path: composite two floors sliding in/out ----------
        if let Some(ref tr) = self.transition {
            let from_floor = tr.from_floor;
            let to_floor = tr.to_floor;
            let t = tr.t(now);
            let going_down = to_floor > from_floor;

            // Build floor-scoped scenes for both floors. Each sub-scene
            // uses uniform(cap) so floor arithmetic stays self-consistent
            // with the remapped desk indices in [0..cap).
            let from_agents = build_floor_scene(scene, from_floor);
            let mut from_scene = SceneState::uniform(scene.floor_capacities[from_floor]);
            for a in from_agents {
                from_scene.agents.insert(a.agent_id, a);
            }

            let to_agents = build_floor_scene(scene, to_floor);
            let mut to_scene = SceneState::uniform(scene.floor_capacities[to_floor]);
            for a in to_agents {
                to_scene.agents.insert(a.agent_id, a);
            }

            let term_size = self.terminal.size()?;
            let full_rect = Rect {
                x: 0,
                y: 0,
                width: term_size.width,
                height: term_size.height,
            };
            let scene_rect = Rect {
                x: 0,
                y: 0,
                width: full_rect.width,
                height: full_rect.height.saturating_sub(1),
            };

            if scene_rect.width < 20 || scene_rect.height < 12 {
                return Ok(());
            }

            let buf_w = scene_rect.width;
            let buf_h = scene_rect.height.saturating_mul(2);

            // Render both floors into their respective buffers.
            // Use split_at_mut to get mutable access to two different indices.
            let (lo, hi) = if from_floor < to_floor {
                (from_floor, to_floor)
            } else {
                (to_floor, from_floor)
            };

            let (bufs_lo, bufs_hi) = self.floor_bufs.split_at_mut(hi);
            let lo_buf = &mut bufs_lo[lo];
            let hi_buf = &mut bufs_hi[0];
            let (from_buf, to_buf) = if from_floor < to_floor {
                (lo_buf, hi_buf)
            } else {
                (hi_buf, lo_buf)
            };

            let (ctxs_lo, ctxs_hi) = self.floor_ctxs.split_at_mut(hi);
            let lo_ctx = &mut ctxs_lo[lo];
            let hi_ctx = &mut ctxs_hi[0];
            let (from_ctx, to_ctx) = if from_floor < to_floor {
                (lo_ctx, hi_ctx)
            } else {
                (hi_ctx, lo_ctx)
            };

            from_buf.ensure_size(buf_w, buf_h, self.theme.surface.bg_fallback);
            to_buf.ensure_size(buf_w, buf_h, self.theme.surface.bg_fallback);

            let from_meta = FloorMeta::for_floor(from_floor, nf);
            let to_meta = FloorMeta::for_floor(to_floor, nf);

            // Transitions hide text overlays, so use throwaway coffee
            // and chitchat state — they won't be rendered anyway.
            let mut transition_chitchat = std::collections::HashMap::new();
            let empty_coffee = std::collections::HashSet::new();
            let empty_fetched = std::collections::HashMap::new();

            if let Some(layout) =
                Layout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, from_meta.floor_seed)
            {
                from_ctx.router.set_preferred_zone(layout.corridor);
                let _ = render_to_rgb_buffer(&mut PixelCtx {
                    scene: &from_scene,
                    layout: &layout,
                    pack,
                    now,
                    buf: from_buf,
                    cache: &mut from_ctx.cache,
                    router: &mut from_ctx.router,
                    overlay: &mut from_ctx.overlay,
                    history: &mut from_ctx.history,
                    theme: self.theme,
                    floor: from_meta,
                    active_pet: None,
                    floor_pet_kind: None,
                    chitchat_state: &mut transition_chitchat,
                    coffee_holders: &empty_coffee,
                    coffee_fetched_at: &empty_fetched,
                });
            }

            if let Some(layout) =
                Layout::compute_with_seed(buf_w, buf_h, MAX_VISIBLE_DESKS, to_meta.floor_seed)
            {
                to_ctx.router.set_preferred_zone(layout.corridor);
                let _ = render_to_rgb_buffer(&mut PixelCtx {
                    scene: &to_scene,
                    layout: &layout,
                    pack,
                    now,
                    buf: to_buf,
                    cache: &mut to_ctx.cache,
                    router: &mut to_ctx.router,
                    overlay: &mut to_ctx.overlay,
                    history: &mut to_ctx.history,
                    theme: self.theme,
                    floor: to_meta,
                    active_pet: None,
                    floor_pet_kind: None,
                    chitchat_state: &mut transition_chitchat,
                    coffee_holders: &empty_coffee,
                    coffee_fetched_at: &empty_fetched,
                });
            }

            // Compute y-offsets for vertical slide with divider gap.
            // t applies to total travel = screen_height + divider_height
            // so the easing covers the full distance including the gap.
            let h = scene_rect.height as f32;
            let divider_h = (scene_rect.height as f32) / 5.0;
            let total = h + divider_h;
            let (from_offset, to_offset) = if going_down {
                // Higher floor: current slides DOWN, new enters from TOP
                let from_y = (t * total) as i32;
                let to_y = -(total - t * total) as i32;
                (from_y, to_y)
            } else {
                // Lower floor: current slides UP, new enters from BOTTOM
                let from_y = -(t * total) as i32;
                let to_y = (total - t * total) as i32;
                (from_y, to_y)
            };

            let theme = self.theme;
            let theme_picker = self.theme_picker;

            self.terminal.draw(|f| {
                let actual_full = f.area();
                let actual_scene = Rect {
                    x: 0,
                    y: 0,
                    width: actual_full.width,
                    height: actual_full.height.saturating_sub(1),
                };
                crate::tui::renderer::paint_footer(f, scene, actual_full, theme, floor_info);
                flush_buffer_to_term_at_offset(f, from_buf, actual_scene, from_offset);
                flush_buffer_to_term_at_offset(f, to_buf, actual_scene, to_offset);

                if let Some(idx) = theme_picker {
                    crate::tui::renderer::paint_theme_picker(f, idx, actual_full, theme);
                }
            })?;

            self.cached_layout = None;
            return Ok(());
        }

        // --- Normal path: single floor ------------------------------------
        let floor_agents = build_floor_scene(scene, self.current_floor);
        let mut floor_scene = SceneState::uniform(scene.floor_capacities[self.current_floor]);
        for agent in floor_agents {
            floor_scene.agents.insert(agent.agent_id, agent);
        }

        // Evict coffee state for agents no longer in the scene.
        self.coffee_holders
            .retain(|id| scene.agents.contains_key(id));
        self.coffee_fetched_at
            .retain(|id, _| scene.agents.contains_key(id));

        let floor_meta = FloorMeta::for_floor(self.current_floor, nf);
        let fctx = &mut self.floor_ctxs[self.current_floor];
        let mut draw_ctx = DrawCtx {
            buf: &mut self.floor_bufs[self.current_floor],
            cache: &mut fctx.cache,
            router: &mut fctx.router,
            overlay: &mut fctx.overlay,
            history: &mut fctx.history,
            mouse_pos: self.mouse_pos,
            pinned_agent: self.pinned_agent,
            ticker: &self.ticker,
            theme: self.theme,
            theme_picker: self.theme_picker,
            floor_info,
            floor: floor_meta,
            active_pet: self.active_pet.as_ref(),
            last_pet_pos: None,
            floor_pet_kind: crate::tui::pet::select_pet_for_floor(
                floor_meta.floor_seed,
                &self.enabled_pets,
            ),
            chitchat_state: &mut self.chitchat_state,
            chitchat_bubbles: Vec::new(),
            coffee_holders: &self.coffee_holders,
            coffee_fetched_at: &self.coffee_fetched_at,
            new_coffee_carriers: Vec::new(),
        };
        let result = draw_scene(&mut self.terminal, &floor_scene, pack, now, &mut draw_ctx);
        self.last_pet_pos = draw_ctx.last_pet_pos;
        // Persist newly detected coffee carriers.
        for id in draw_ctx.new_coffee_carriers {
            if self.coffee_holders.insert(id) {
                self.coffee_fetched_at.insert(id, now);
            }
        }
        if let Ok(ref layout_opt) = result {
            self.cached_layout = layout_opt.clone();
        }
        result.map(|_| ())
    }
}
