# Multi-Floor Office Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support up to 5 dynamic office floors with slide-transition navigation and per-floor render isolation.

**Architecture:** Floor is a render-time viewport — core stays unchanged. Each floor gets its own `RgbBuffer`, `AStarRouter`, `OccupancyOverlay`, `PoseHistory`, and `FrameCache` via a `FloorCtx` struct. Floor membership is derived from `desk_index / desks_per_floor`. Slide transition composites two buffers at x-offsets during a ~500ms animation.

**Tech Stack:** Rust, ratatui, crossterm, tokio. Spec: `docs/superpowers/specs/2026-05-25-multi-floor-office-design.md`

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/pixtuoid/src/tui/floor.rs` (NEW) | `FloorCtx`, `FloorTransition`, `FloorScene`, floor math helpers |
| `crates/pixtuoid/src/tui/tui_renderer.rs` | Multi-buffer ownership, floor navigation, transition state |
| `crates/pixtuoid/src/tui/renderer.rs` | `flush_buffer_to_term_at_offset`, floor indicator in footer + wall display |
| `crates/pixtuoid/src/tui/mod.rs` | PageUp/PageDown key events, `pub mod floor` |
| `crates/pixtuoid/src/tui/pixel_painter/mod.rs` | `render_to_rgb_buffer` signature: accept agent slice instead of `&SceneState` |
| `crates/pixtuoid-core/src/state/mod.rs` | `MAX_FLOORS` constant, `next_free_desk` range extended |

---

### Task 1: Create `floor.rs` with FloorCtx, FloorTransition, FloorScene, and math helpers

**Files:**
- Create: `crates/pixtuoid/src/tui/floor.rs`
- Modify: `crates/pixtuoid/src/tui/mod.rs` (add `pub mod floor;`)

- [ ] **Step 1: Write the failing tests for floor math**

Add to `crates/pixtuoid/src/tui/floor.rs`:

```rust
//! Multi-floor office infrastructure — per-floor render context,
//! slide transition state, and floor-scoped agent projection.

use std::time::SystemTime;

use pixtuoid_core::state::{AgentSlot, SceneState};
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::walkable::OccupancyOverlay;

use crate::tui::frame_cache::FrameCache;
use crate::tui::pathfind::AStarRouter;
use crate::tui::pose::PoseHistory;

pub const MAX_FLOORS: usize = 5;
const TRANSITION_DURATION_MS: u64 = 500;

pub struct FloorCtx {
    pub router: AStarRouter,
    pub overlay: OccupancyOverlay,
    pub history: PoseHistory,
    pub cache: FrameCache,
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

    pub fn flush_caches(&mut self) {
        self.cache = FrameCache::new();
    }
}

pub struct FloorTransition {
    pub from_floor: usize,
    pub to_floor: usize,
    pub started_at: SystemTime,
    pub duration_ms: u64,
}

impl FloorTransition {
    pub fn new(from: usize, to: usize, now: SystemTime) -> Self {
        Self {
            from_floor: from,
            to_floor: to,
            started_at: now,
            duration_ms: TRANSITION_DURATION_MS,
        }
    }

    pub fn t(&self, now: SystemTime) -> f32 {
        let elapsed = now
            .duration_since(self.started_at)
            .unwrap_or_default()
            .as_millis() as f32;
        (elapsed / self.duration_ms as f32).clamp(0.0, 1.0)
    }

    pub fn is_done(&self, now: SystemTime) -> bool {
        self.t(now) >= 1.0
    }
}

pub fn floor_of(desk_index: usize, desks_per_floor: usize) -> usize {
    if desks_per_floor == 0 {
        return 0;
    }
    desk_index / desks_per_floor
}

pub fn floor_local_desk(desk_index: usize, desks_per_floor: usize) -> usize {
    if desks_per_floor == 0 {
        return 0;
    }
    desk_index % desks_per_floor
}

pub fn num_floors(scene: &SceneState) -> usize {
    let max_desk = scene
        .agents
        .values()
        .map(|a| a.desk_index)
        .max()
        .unwrap_or(0);
    if scene.max_desks == 0 {
        return 1;
    }
    (max_desk / scene.max_desks) + 1
}

pub fn build_floor_scene(scene: &SceneState, floor_idx: usize) -> (Vec<AgentSlot>, usize) {
    let dpf = scene.max_desks;
    let lo = floor_idx * dpf;
    let hi = lo + dpf;
    let mut agents: Vec<AgentSlot> = scene
        .agents
        .values()
        .filter(|a| a.desk_index >= lo && a.desk_index < hi)
        .cloned()
        .map(|mut a| {
            a.desk_index = floor_local_desk(a.desk_index, dpf);
            a
        })
        .collect();
    agents.sort_by_key(|a| a.desk_index);
    (agents, dpf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use pixtuoid_core::state::ActivityState;
    use pixtuoid_core::AgentId;

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
        let s = make_scene(20, 16);
        assert_eq!(num_floors(&s), 2);
    }

    #[test]
    fn num_floors_exact_fit() {
        let s = make_scene(16, 16);
        assert_eq!(num_floors(&s), 1);
    }

    #[test]
    fn num_floors_empty() {
        let s = make_scene(0, 16);
        assert_eq!(num_floors(&s), 1);
    }

    #[test]
    fn build_floor_scene_filters_and_remaps() {
        let s = make_scene(20, 16);
        let (floor0, max) = build_floor_scene(&s, 0);
        assert_eq!(floor0.len(), 16);
        assert_eq!(max, 16);
        assert!(floor0.iter().all(|a| a.desk_index < 16));

        let (floor1, _) = build_floor_scene(&s, 1);
        assert_eq!(floor1.len(), 4);
        assert_eq!(floor1[0].desk_index, 0);
        assert_eq!(floor1[3].desk_index, 3);
    }

    #[test]
    fn transition_t_progresses_linearly() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let tr = FloorTransition::new(0, 1, t0);
        assert!((tr.t(t0) - 0.0).abs() < 0.01);
        assert!((tr.t(t0 + Duration::from_millis(250)) - 0.5).abs() < 0.01);
        assert!((tr.t(t0 + Duration::from_millis(500)) - 1.0).abs() < 0.01);
        assert!(!tr.is_done(t0 + Duration::from_millis(250)));
        assert!(tr.is_done(t0 + Duration::from_millis(500)));
    }

    #[test]
    fn transition_t_clamps_past_duration() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let tr = FloorTransition::new(0, 1, t0);
        assert!((tr.t(t0 + Duration::from_millis(1000)) - 1.0).abs() < 0.01);
    }
}
```

- [ ] **Step 2: Add `pub mod floor;` to `tui/mod.rs`**

In `crates/pixtuoid/src/tui/mod.rs`, add after the existing module declarations:

```rust
pub mod floor;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p pixtuoid -- tui::floor::tests -v`
Expected: All 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/pixtuoid/src/tui/floor.rs crates/pixtuoid/src/tui/mod.rs
git commit -m "feat(floor): add FloorCtx, FloorTransition, FloorScene with tests"
```

---

### Task 2: Extend `next_free_desk` range for multi-floor

**Files:**
- Modify: `crates/pixtuoid-core/src/state/mod.rs:79-84`
- Test: `crates/pixtuoid-core/src/state/mod.rs` (inline tests)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/pixtuoid-core/src/state/mod.rs`:

```rust
#[test]
fn next_free_desk_overflows_to_second_floor() {
    let mut s = SceneState::new(4);
    let now = SystemTime::now();
    for i in 0..4 {
        let id = AgentId::from_transcript_path(&format!("f{i}"));
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
    assert_eq!(
        s.next_free_desk(),
        Some(4),
        "should overflow to desk 4 (floor 1)"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pixtuoid-core -- next_free_desk_overflows -v`
Expected: FAIL — current range is `0..max_desks` (0..4), so `next_free_desk` returns `None`.

- [ ] **Step 3: Update `next_free_desk`**

In `crates/pixtuoid-core/src/state/mod.rs`, change:

```rust
pub const MAX_FLOORS: usize = 5;

impl SceneState {
    pub fn new(max_desks: usize) -> Self {
        Self {
            agents: BTreeMap::new(),
            max_desks,
        }
    }

    pub fn next_free_desk(&self) -> Option<usize> {
        let occupied: std::collections::BTreeSet<usize> =
            self.agents.values().map(|a| a.desk_index).collect();
        (0..self.max_desks * MAX_FLOORS).find(|i| !occupied.contains(i))
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p pixtuoid-core -- next_free_desk -v`
Expected: All 3 desk tests pass (existing 2 + new overflow).

- [ ] **Step 5: Commit**

```bash
git add crates/pixtuoid-core/src/state/mod.rs
git commit -m "feat(core): extend next_free_desk range to max_desks * MAX_FLOORS"
```

---

### Task 3: Refactor TuiRenderer to use per-floor contexts

**Files:**
- Modify: `crates/pixtuoid/src/tui/tui_renderer.rs`

- [ ] **Step 1: Replace single buf/cache/router/overlay/history with floor vectors**

Replace the struct definition and `new()`:

```rust
use crate::tui::floor::{FloorCtx, FloorTransition};

pub struct TuiRenderer<B: Backend> {
    pub terminal: Terminal<B>,
    floor_bufs: Vec<RgbBuffer>,
    floor_ctxs: Vec<FloorCtx>,
    current_floor: usize,
    transition: Option<FloorTransition>,
    mouse_pos: Option<(u16, u16)>,
    pinned_agent: Option<pixtuoid_core::AgentId>,
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

    pub fn navigate_floor(&mut self, target: usize, now: std::time::SystemTime) {
        if target == self.current_floor || self.transition.is_some() {
            return;
        }
        self.transition = Some(FloorTransition::new(self.current_floor, target, now));
    }
```

- [ ] **Step 2: Update `set_theme` to flush all floor caches**

```rust
    pub fn set_theme(&mut self, theme: &'static crate::tui::theme::Theme) {
        if !std::ptr::eq(self.theme, theme) {
            self.theme = theme;
            for ctx in &mut self.floor_ctxs {
                ctx.flush_caches();
            }
        }
    }
```

- [ ] **Step 3: Update `buf()` to return current floor's buffer**

```rust
    pub fn buf(&self) -> &RgbBuffer {
        &self.floor_bufs[self.current_floor]
    }
```

- [ ] **Step 4: Update `evict_missing` and `invalidate_routes` for all floors**

```rust
    pub fn evict_missing(&mut self, scene: &SceneState) {
        for ctx in &mut self.floor_ctxs {
            ctx.cache.evict_missing(scene);
        }
    }

    pub fn invalidate_routes(&mut self) {
        use crate::tui::pathfind::Router;
        for ctx in &mut self.floor_ctxs {
            ctx.router.invalidate();
        }
    }
```

- [ ] **Step 5: Update `Renderer::render` to use floor projection**

```rust
impl<B: Backend> Renderer for TuiRenderer<B> {
    fn render(&mut self, scene: &SceneState, pack: &Pack, now: SystemTime) -> Result<()> {
        use crate::tui::floor::{build_floor_scene, num_floors};

        self.ticker.update(scene);
        let n_floors = num_floors(scene);

        // Grow floor vectors if needed
        while self.floor_bufs.len() < n_floors {
            self.floor_bufs.push(RgbBuffer::filled(0, 0, Rgb(0, 0, 0)));
            self.floor_ctxs.push(FloorCtx::new());
        }

        // Clamp current floor if agents left
        if self.current_floor >= n_floors {
            self.current_floor = n_floors - 1;
            self.transition = None;
        }

        // Complete transition if done
        if let Some(ref tr) = self.transition {
            if tr.is_done(now) {
                self.current_floor = tr.to_floor;
                self.transition = None;
            }
        }

        // Build floor-scoped scene and render
        let floor_idx = self.current_floor;
        let (agents, max_desks) = build_floor_scene(scene, floor_idx);
        let mut floor_scene = SceneState::new(max_desks);
        for a in agents {
            floor_scene.agents.insert(a.agent_id, a);
        }

        let ctx = &mut self.floor_ctxs[floor_idx];
        let buf = &mut self.floor_bufs[floor_idx];

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
            Some((floor_idx + 1, n_floors)),
        )
    }
}
```

Note: `draw_scene` gets a new parameter `floor_info: Option<(usize, usize)>` — the current floor (1-indexed for display) and total floors. Passed to footer/wall display. When `None`, no floor indicator is shown (backward compat for tests/snapshot).

- [ ] **Step 6: Run full test suite**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: Compilation errors from `draw_scene` signature change. Fix in next task.

- [ ] **Step 7: Commit (WIP — compiles after Task 4)**

Hold commit until Task 4 updates `draw_scene` signature.

---

### Task 4: Update `draw_scene` signature and add floor indicator to footer

**Files:**
- Modify: `crates/pixtuoid/src/tui/renderer.rs`
- Modify: `crates/pixtuoid/tests/snapshot_regression.rs`
- Modify: `crates/pixtuoid/examples/snapshot.rs`

- [ ] **Step 1: Add `floor_info` parameter to `draw_scene`**

In `crates/pixtuoid/src/tui/renderer.rs`, update the signature:

```rust
pub fn draw_scene<B: Backend>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    pack: &Pack,
    now: SystemTime,
    buf: &mut RgbBuffer,
    cache: &mut FrameCache,
    router: &mut dyn Router,
    overlay: &mut OccupancyOverlay,
    history: &mut pose::PoseHistory,
    mouse_pos: Option<(u16, u16)>,
    pinned_agent: Option<AgentId>,
    ticker: &TickerQueue,
    theme: &crate::tui::theme::Theme,
    theme_picker: Option<usize>,
    floor_info: Option<(usize, usize)>,  // (current_floor_1indexed, total_floors)
) -> Result<()> {
```

- [ ] **Step 2: Update footer to show floor indicator**

In `build_status_summary`, add floor info when `num_floors > 1`. Change the function signature to accept floor info:

```rust
pub(super) fn build_status_summary(
    scene: &SceneState,
    term_width: u16,
    floor_info: Option<(usize, usize)>,
) -> String {
```

And before the QUIT suffix assembly, add:

```rust
    let floor_str = match floor_info {
        Some((current, total)) if total > 1 => format!(" F{current}/{total} [↑↓]"),
        _ => String::new(),
    };
    // Change QUIT to include floor_str
    let right_side = format!("{floor_str}{QUIT}");
```

Then use `right_side` instead of `QUIT` in the width calculations.

- [ ] **Step 3: Thread `floor_info` through `paint_footer` and `paint_wall_display`**

Update `paint_footer` to pass `floor_info` to `build_status_summary`.

In `paint_wall_display`, add floor to the top line when `floor_info` is `Some`:

```rust
    let floor_label = match floor_info {
        Some((current, total)) if total > 1 => format!("  Floor {current}/{total}"),
        _ => String::new(),
    };
    let top_line = Line::from(vec![
        Span::styled(
            format!("pixtuoid v{version}"),
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("★ Star", ...),
        Span::styled(floor_label, Style::default().fg(to_color(theme.ui.neon_ticker))),
    ]);
```

- [ ] **Step 4: Update all `draw_scene` callers to pass `None` for floor_info**

In `crates/pixtuoid/tests/snapshot_regression.rs`, add `None,` as the last arg to all 3 `draw_scene` calls.

In `crates/pixtuoid/examples/snapshot.rs`, add `None,` as the last arg to both `draw_scene` calls.

- [ ] **Step 5: Update `build_status_summary` test callers**

All tests calling `build_status_summary(&s, width)` need the new third arg: `build_status_summary(&s, width, None)`.

- [ ] **Step 6: Run full test suite**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: All pass. `cargo clippy` clean.

- [ ] **Step 7: Commit Tasks 3 + 4 together**

```bash
git add crates/pixtuoid/src/tui/tui_renderer.rs \
       crates/pixtuoid/src/tui/renderer.rs \
       crates/pixtuoid/tests/snapshot_regression.rs \
       crates/pixtuoid/examples/snapshot.rs
git commit -m "feat(floor): per-floor render contexts + floor indicator in footer/wall"
```

---

### Task 5: Add PageUp/PageDown key handling

**Files:**
- Modify: `crates/pixtuoid/src/tui/mod.rs`

- [ ] **Step 1: Add floor navigation to key event handler**

In `crates/pixtuoid/src/tui/mod.rs`, inside the `match (k.code, k.modifiers)` block (after the `Char('-')` arm), add:

```rust
                                (KeyCode::PageDown, _) => {
                                    let n_floors = crate::tui::floor::num_floors(&snapshot);
                                    let cur = renderer.current_floor();
                                    if cur + 1 < n_floors && renderer.transition().is_none() {
                                        renderer.navigate_floor(cur + 1, now);
                                    }
                                }
                                (KeyCode::PageUp, _) => {
                                    let cur = renderer.current_floor();
                                    if cur > 0 && renderer.transition().is_none() {
                                        renderer.navigate_floor(cur - 1, now);
                                    }
                                }
```

- [ ] **Step 2: Cancel transition on terminal resize**

In the layout sig check (around line 62), add after `renderer.invalidate_routes()`:

```rust
                if last_layout_sig != Some(sig) {
                    renderer.invalidate_routes();
                    renderer.cancel_transition();
                    last_layout_sig = Some(sig);
                }
```

And add `cancel_transition` to `TuiRenderer`:

```rust
    pub fn cancel_transition(&mut self) {
        if let Some(tr) = self.transition.take() {
            self.current_floor = tr.to_floor;
        }
    }
```

- [ ] **Step 3: Run and test manually**

Run: `cargo build --release --workspace`
Run: `./target/release/pixtuoid run`
Test: With 16+ agents, press PageDown/PageUp. Floor indicator should update in footer and neon display.

- [ ] **Step 4: Commit**

```bash
git add crates/pixtuoid/src/tui/mod.rs crates/pixtuoid/src/tui/tui_renderer.rs
git commit -m "feat(floor): PageUp/PageDown floor navigation"
```

---

### Task 6: Implement slide transition compositing

**Files:**
- Modify: `crates/pixtuoid/src/tui/renderer.rs`
- Modify: `crates/pixtuoid/src/tui/tui_renderer.rs`

- [ ] **Step 1: Add `flush_buffer_to_term_at_offset`**

In `crates/pixtuoid/src/tui/renderer.rs`, after the existing `flush_buffer_to_term`:

```rust
fn flush_buffer_to_term_at_offset(
    f: &mut ratatui::Frame<'_>,
    buf: &RgbBuffer,
    scene_rect: Rect,
    x_offset: i32,
) {
    let term_buf = f.buffer_mut();
    let w = buf.width as usize;
    let cell_rows = (buf.height / 2) as usize;
    for cy in 0..cell_rows {
        for cx in 0..(buf.width as usize) {
            let target_x = cx as i32 + x_offset;
            if target_x < 0 || target_x >= scene_rect.width as i32 {
                continue;
            }
            let x = scene_rect.x + target_x as u16;
            let y = scene_rect.y + cy as u16;
            if y >= scene_rect.y + scene_rect.height {
                continue;
            }
            let py_top = cy * 2;
            let py_bot = cy * 2 + 1;
            let fg = buf.pixels[py_top * w + cx];
            let bg = buf.pixels[py_bot * w + cx];
            let cell = &mut term_buf[(x, y)];
            cell.set_symbol("▀");
            cell.fg = Color::Rgb(fg.0, fg.1, fg.2);
            cell.bg = Color::Rgb(bg.0, bg.1, bg.2);
        }
    }
}
```

- [ ] **Step 2: Make `flush_buffer_to_term_at_offset` pub(super)**

```rust
pub(super) fn flush_buffer_to_term_at_offset(...)
```

- [ ] **Step 3: Update `TuiRenderer::render` to composite during transitions**

In `tui_renderer.rs`, update the render method to handle transitions. When `self.transition.is_some()`, render both floors and composite with offsets:

```rust
        if let Some(ref tr) = self.transition {
            let t = tr.t(now);
            let from_floor = tr.from_floor;
            let to_floor = tr.to_floor;
            let direction = if to_floor > from_floor { -1i32 } else { 1i32 };

            // Render outgoing floor
            let (from_agents, from_max) = build_floor_scene(scene, from_floor);
            let mut from_scene = SceneState::new(from_max);
            for a in from_agents { from_scene.agents.insert(a.agent_id, a); }
            let from_ctx = &mut self.floor_ctxs[from_floor];
            let from_buf = &mut self.floor_bufs[from_floor];
            // ... render from_scene into from_buf using from_ctx

            // Render incoming floor
            let (to_agents, to_max) = build_floor_scene(scene, to_floor);
            let mut to_scene = SceneState::new(to_max);
            for a in to_agents { to_scene.agents.insert(a.agent_id, a); }
            let to_ctx = &mut self.floor_ctxs[to_floor];
            let to_buf = &mut self.floor_bufs[to_floor];
            // ... render to_scene into to_buf using to_ctx

            // Composite both into terminal with x offsets
            // term.draw(|f| {
            //     let w = scene_rect.width as i32;
            //     let out_x = (direction as f32 * t * w as f32) as i32;
            //     let in_x = out_x - direction * w;
            //     flush_buffer_to_term_at_offset(f, from_buf, scene_rect, out_x);
            //     flush_buffer_to_term_at_offset(f, to_buf, scene_rect, in_x);
            //     paint_footer(f, scene, full_rect, theme);
            // });
        }
```

The exact wiring requires accessing both floor contexts mutably, which means splitting the borrow. Use index-based access with temporary variables.

- [ ] **Step 4: Build and visually verify**

Run: `cargo build --release --workspace`
Run: `./target/release/pixtuoid run` with 16+ agents
Test: PageDown should show a smooth slide transition.

- [ ] **Step 5: Commit**

```bash
git add crates/pixtuoid/src/tui/renderer.rs crates/pixtuoid/src/tui/tui_renderer.rs
git commit -m "feat(floor): slide transition compositing between floors"
```

---

### Task 7: Update docs and keyboard shortcuts display

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`
- Modify: `crates/pixtuoid/src/tui/renderer.rs` (keyboard shortcut string)

- [ ] **Step 1: Update keyboard shortcuts in footer**

In `renderer.rs`, update the QUIT constant:

```rust
const QUIT: &str = " [p]ause [t]heme [+/-]desks [PgUp/Dn]floor [q]uit ";
```

- [ ] **Step 2: Update README features table**

Add to the features table:

```markdown
| 🛗 | **Multi-floor office** | PageUp/PageDown to navigate floors with slide transition; auto-overflow when desks fill |
```

- [ ] **Step 3: Update CLAUDE.md**

Add to "Where to look":

```markdown
- "How do multi-floor offices work?" → `tui/floor.rs` defines `FloorCtx` (per-floor render state), `FloorTransition` (slide animation), `FloorScene` (agent projection). `tui_renderer.rs` owns `Vec<FloorCtx>` + `Vec<RgbBuffer>` and switches between them. Floor membership derived from `desk_index / max_desks`. `next_free_desk` in `state/mod.rs` searches `0..max_desks * MAX_FLOORS`. PageUp/PageDown in `tui/mod.rs`.
```

- [ ] **Step 4: Run full test suite and preflight**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Run: `cargo clippy --workspace --all-targets --features pixtuoid-core/test-renderer -- -D warnings`
Run: `cargo fmt --all --check`
Expected: All clean.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md README.md crates/pixtuoid/src/tui/renderer.rs
git commit -m "docs: multi-floor office — keyboard shortcuts, README, CLAUDE.md"
```

---

## Self-Review

**Spec coverage:**
- ✅ Dynamic floors up to 5 — Task 2 (`MAX_FLOORS = 5`)
- ✅ Auto-overflow — Task 2 (`next_free_desk` extended range)
- ✅ Per-floor render isolation — Task 3 (`FloorCtx` per floor)
- ✅ Slide transition — Task 6 (composited flush with x-offsets)
- ✅ Floor indicators — Task 4 (footer + neon display)
- ✅ PageUp/PageDown — Task 5
- ✅ Edge cases — Task 5 (cancel on resize), Task 3 (clamp on agent exit)
- ⚠️ Themed floor variants — infrastructure ready (`FloorVariant` enum in spec) but not implemented in plan. This is explicitly deferred per spec ("v1: Standard and OpenPlan"). Add as a follow-up task after the base multi-floor works.

**Placeholder scan:** No TBDs. Task 6 step 3 has pseudo-code for the compositing — the exact borrow-split pattern depends on the final struct layout after Task 3. The engineer must resolve the split at implementation time.

**Type consistency:** `FloorCtx`, `FloorTransition`, `FloorScene`, `build_floor_scene`, `num_floors`, `floor_of`, `floor_local_desk` — consistent across all tasks.
