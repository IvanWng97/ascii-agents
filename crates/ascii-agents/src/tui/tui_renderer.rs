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

use crate::tui::floor::{build_floor_scene, num_floors, FloorCtx, FloorTransition};
use crate::tui::layout::Layout;
use crate::tui::pathfind::Router;
use crate::tui::pixel_painter::render_to_rgb_buffer;
use crate::tui::renderer::{draw_scene, flush_buffer_to_term_at_offset};

pub struct TuiRenderer<B: Backend> {
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
}

impl<B: Backend> TuiRenderer<B> {
    pub fn new(terminal: Terminal<B>, theme: &'static crate::tui::theme::Theme) -> Self {
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
        }
    }

    pub fn current_floor(&self) -> usize {
        self.current_floor
    }

    pub fn transition(&self) -> Option<&FloorTransition> {
        self.transition.as_ref()
    }

    pub fn navigate_floor(&mut self, target: usize, now: SystemTime) {
        if target == self.current_floor || target >= self.floor_ctxs.len() {
            return;
        }
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

    /// Drop the cached frame entries for agents no longer in `scene`.
    /// Forwarded so the render loop doesn't reach into the cache directly.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        for ctx in &mut self.floor_ctxs {
            ctx.cache.evict_missing(scene);
        }
    }

    /// Invalidate all floors' router path caches. Call when the static
    /// walkable mask changes (terminal resize, max_desks change).
    pub fn invalidate_routes(&mut self) {
        for ctx in &mut self.floor_ctxs {
            ctx.router.invalidate();
        }
    }
}

impl<B: Backend> Renderer for TuiRenderer<B> {
    fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
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

        // Clamp current_floor if agents left and floors shrank.
        if self.current_floor >= nf {
            self.current_floor = nf.saturating_sub(1);
        }

        // Complete transition if done.
        if let Some(ref tr) = self.transition {
            if tr.is_done(now) {
                self.current_floor = tr.to_floor;
                self.transition = None;
            }
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

            // Build floor-scoped scenes for both floors.
            let (from_agents, from_dpf) = build_floor_scene(scene, from_floor);
            let mut from_scene = SceneState::new(from_dpf);
            for a in from_agents {
                from_scene.agents.insert(a.agent_id, a);
            }

            let (to_agents, to_dpf) = build_floor_scene(scene, to_floor);
            let mut to_scene = SceneState::new(to_dpf);
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

            let buf_w = scene_rect.width;
            let buf_h = scene_rect.height * 2;

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

            if let Some(layout) = Layout::compute(buf_w, buf_h, from_scene.max_desks) {
                from_ctx.router.set_preferred_zone(layout.corridor);
                render_to_rgb_buffer(
                    &from_scene,
                    &layout,
                    pack,
                    now,
                    from_buf,
                    &mut from_ctx.cache,
                    &mut from_ctx.router,
                    &mut from_ctx.overlay,
                    &mut from_ctx.history,
                    self.theme,
                );
            }

            if let Some(layout) = Layout::compute(buf_w, buf_h, to_scene.max_desks) {
                to_ctx.router.set_preferred_zone(layout.corridor);
                render_to_rgb_buffer(
                    &to_scene,
                    &layout,
                    pack,
                    now,
                    to_buf,
                    &mut to_ctx.cache,
                    &mut to_ctx.router,
                    &mut to_ctx.overlay,
                    &mut to_ctx.history,
                    self.theme,
                );
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
                crate::tui::renderer::paint_footer(f, scene, full_rect, theme, floor_info);
                flush_buffer_to_term_at_offset(f, from_buf, scene_rect, from_offset);
                flush_buffer_to_term_at_offset(f, to_buf, scene_rect, to_offset);
                if let Some(idx) = theme_picker {
                    crate::tui::renderer::paint_theme_picker(f, idx, full_rect, theme);
                }
            })?;

            return Ok(());
        }

        // --- Normal path: single floor ------------------------------------
        let (floor_agents, desks_per_floor) = build_floor_scene(scene, self.current_floor);
        let mut floor_scene = SceneState::new(desks_per_floor);
        for agent in floor_agents {
            floor_scene.agents.insert(agent.agent_id, agent);
        }

        let ctx = &mut self.floor_ctxs[self.current_floor];
        let buf = &mut self.floor_bufs[self.current_floor];
        draw_scene(
            &mut self.terminal,
            &floor_scene,
            pack,
            now,
            buf,
            &mut ctx.cache,
            &mut ctx.router,
            &mut ctx.overlay,
            &mut ctx.history,
            self.mouse_pos,
            self.pinned_agent,
            &self.ticker,
            self.theme,
            self.theme_picker,
            floor_info,
        )
    }
}
