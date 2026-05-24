//! `Renderer` trait impl that drives the half-block terminal TUI.
//!
//! Closes the v1 gap where production code called the free function
//! `draw_scene` directly, leaving the core `Renderer` trait exercised only
//! by `TestRenderer` in tests. `TuiRenderer` is the production impl: it
//! owns the cross-frame mutable state (`RgbBuffer`, `FrameCache`,
//! `AStarRouter`, `OccupancyOverlay`, `PoseHistory`) and forwards to
//! `draw_scene`, which recomputes its own layout per frame from
//! `terminal.size()` because the user can resize at any time.

use std::time::SystemTime;

use anyhow::Result;
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::state::SceneState;
use ascii_agents_core::walkable::OccupancyOverlay;
use ascii_agents_core::Renderer;
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::tui::frame_cache::FrameCache;
use crate::tui::pathfind::AStarRouter;
use crate::tui::pose::PoseHistory;
use crate::tui::renderer::draw_scene;

pub struct TuiRenderer<B: Backend> {
    pub terminal: Terminal<B>,
    buf: RgbBuffer,
    cache: FrameCache,
    router: AStarRouter,
    overlay: OccupancyOverlay,
    history: PoseHistory,
    mouse_pos: Option<(u16, u16)>,
    pinned_agent: Option<ascii_agents_core::AgentId>,
    pub ticker: crate::tui::renderer::TickerQueue,
    theme: &'static crate::tui::theme::Theme,
}

impl<B: Backend> TuiRenderer<B> {
    pub fn new(terminal: Terminal<B>, theme: &'static crate::tui::theme::Theme) -> Self {
        Self {
            terminal,
            buf: RgbBuffer::filled(0, 0, Rgb(0, 0, 0)),
            cache: FrameCache::new(),
            router: AStarRouter::new(),
            overlay: OccupancyOverlay::new(),
            history: PoseHistory::new(),
            mouse_pos: None,
            pinned_agent: None,
            ticker: crate::tui::renderer::TickerQueue::new(),
            theme,
        }
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
        &self.buf
    }

    pub fn set_theme(&mut self, theme: &'static crate::tui::theme::Theme) {
        self.theme = theme;
        self.cache = FrameCache::new();
    }

    /// Drop the cached frame entries for agents no longer in `scene`.
    /// Forwarded so the render loop doesn't reach into the cache directly.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.cache.evict_missing(scene);
    }

    /// Invalidate the router's path cache. Call when the static walkable
    /// mask changes (terminal resize, max_desks change).
    pub fn invalidate_routes(&mut self) {
        use crate::tui::pathfind::Router;
        self.router.invalidate();
    }
}

impl<B: Backend> Renderer for TuiRenderer<B> {
    fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        self.ticker.update(scene);
        draw_scene(
            &mut self.terminal,
            scene,
            pack,
            now,
            &mut self.buf,
            &mut self.cache,
            &mut self.router,
            &mut self.overlay,
            &mut self.history,
            self.mouse_pos,
            self.pinned_agent,
            &self.ticker,
            self.theme,
        )
    }
}
