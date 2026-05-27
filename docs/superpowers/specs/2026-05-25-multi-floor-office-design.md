# Multi-Floor Office Design

**Date:** 2026-05-25
**Status:** Draft
**Author:** Ivan + Claude

## Overview

Extend the terminal office from a single floor to a dynamic multi-floor building. When agents exceed the desk capacity of one floor, they auto-overflow to the next floor. Users navigate floors with PageUp/PageDown. A slide transition animates the switch, with both floors rendering simultaneously during the animation.

## Goals

1. Support up to 5 dynamic floors (created on demand as agents overflow)
2. Each floor has a themed layout variant (different room arrangements)
3. Smooth slide transition between floors (~500ms)
4. Both floors stay animated during transition (agents keep typing/walking)
5. Floor indicators in footer and neon wall display
6. Zero changes to core crate (reducer, SceneState, AgentSlot)

## Non-Goals

- User-assignable floor placement (agents auto-overflow only)
- Source-based floor segregation (CC on floor 1, AG on floor 2)
- Elevator sprite/character walking between floors
- Per-floor theme colors (all floors share the active theme)

## Architecture: Floor as Render-Time Viewport

### Core Invariant

`SceneState`, `Reducer`, and `AgentSlot` are **unchanged**. Floor membership is derived at render time from `desk_index / desks_per_floor` where `desks_per_floor = SceneState::max_desks`. The reducer's `next_free_desk()` naturally fills floor 0 first; when all `max_desks` slots are taken, the next agent gets `desk_index = max_desks`, which maps to floor 1.

### Why Not In Core

Floors are a visual grouping, not a semantic one. The reducer doesn't need to know which floor an agent is on — it only cares about event ordering, dedup, and state transitions. Putting floor logic in the reducer would violate the separation between headless state (core) and terminal presentation (TUI).

### Data Model

#### New: `FloorCtx` (per-floor render state)

```rust
// tui/floor.rs
struct FloorCtx {
    router: AStarRouter,
    overlay: OccupancyOverlay,
    history: PoseHistory,
    cache: FrameCache,
}
```

Each floor gets its own A* router, occupancy overlay, pose history, and frame cache. This prevents cache corruption when switching floors or rendering two floors simultaneously during a transition.

#### New: `FloorTransition` (slide animation state)

```rust
// tui/floor.rs
struct FloorTransition {
    from_floor: usize,
    to_floor: usize,
    started_at: SystemTime,
    duration_ms: u64,  // 500ms
}

impl FloorTransition {
    fn t(&self, now: SystemTime) -> f32 {
        let elapsed = now.duration_since(self.started_at)
            .unwrap_or_default().as_millis() as f32;
        (elapsed / self.duration_ms as f32).clamp(0.0, 1.0)
    }

    fn is_done(&self, now: SystemTime) -> bool {
        self.t(now) >= 1.0
    }
}
```

#### New: `FloorScene` (read-only floor projection)

```rust
// tui/floor.rs
struct FloorScene {
    agents: Vec<AgentSlot>,  // filtered + desk_index remapped to [0..desks_per_floor)
    max_desks: usize,        // = desks_per_floor
}
```

Built per-frame from `SceneState` by filtering agents whose `desk_index` falls in `[floor * desks_per_floor, (floor+1) * desks_per_floor)` and remapping `desk_index` to the floor-local range.

#### Modified: `TuiRenderer`

```rust
pub struct TuiRenderer<B: Backend> {
    pub terminal: Terminal<B>,
    floor_bufs: Vec<RgbBuffer>,      // one pixel buffer per floor
    floor_ctxs: Vec<FloorCtx>,      // parallel; one per floor
    current_floor: usize,
    transition: Option<FloorTransition>,
    // ... existing fields unchanged (mouse_pos, pinned_agent, ticker, theme, theme_picker)
}
```

### Floor Assignment

```
desk_index:  0  1  2 ... 15 | 16 17 18 ... 31 | 32 33 ...
floor:       ----floor 0---- | ----floor 1----- | --floor 2--
```

- `floor(agent) = agent.desk_index / scene.max_desks`
- `floor_local_desk(agent) = agent.desk_index % scene.max_desks`
- `num_floors = max(1, ceil(max_occupied_desk / max_desks))`

The reducer's `next_free_desk()` currently searches `0..max_desks`. To support multi-floor, extend the search range to `0..(max_desks * MAX_FLOORS)`:

```rust
// state/mod.rs — the one core change
pub fn next_free_desk(&self) -> Option<usize> {
    let occupied: BTreeSet<usize> = self.agents.values().map(|a| a.desk_index).collect();
    (0..self.max_desks * MAX_FLOORS).find(|i| !occupied.contains(i))
}
```

`MAX_FLOORS = 5` as a constant. This is the only core change — it just widens the desk index range.

### Render Pipeline

#### Normal Frame (no transition)

```
TuiRenderer::render(scene, pack, now)
  ├─ compute floor_count from max occupied desk_index
  ├─ grow floor_bufs/floor_ctxs if floor_count increased
  ├─ build FloorScene for current_floor (filter + remap)
  ├─ Layout::compute(buf_w, buf_h, floor_scene.max_desks)
  ├─ render_to_rgb_buffer(floor_scene, layout, pack, now,
  │    floor_bufs[current_floor], floor_ctxs[current_floor], ...)
  ├─ flush_buffer_to_term(f, floor_bufs[current_floor], scene_rect)
  ├─ paint labels, tooltips, wall display, footer (with floor indicator)
  └─ paint theme picker if open
```

#### Transition Frame (slide animation active)

```
TuiRenderer::render(scene, pack, now)
  ├─ compute t = transition.t(now)  // 0.0 → 1.0
  ├─ render BOTH floors into their respective floor_bufs
  ├─ composited_flush:
  │    outgoing floor: x_offset = -(t * term_width)
  │    incoming floor: x_offset = term_width - (t * term_width)
  ├─ if transition.is_done(now):
  │    current_floor = transition.to_floor
  │    transition = None
  └─ paint footer with floor indicator
```

### Slide Transition

`flush_buffer_to_term_at_offset` is a trivial generalization of the existing `flush_buffer_to_term` that adds an `x_offset: i32` to each cell's column:

```rust
fn flush_buffer_to_term_at_offset(
    f: &mut Frame, buf: &RgbBuffer, scene_rect: Rect, x_offset: i32
) {
    for cy in 0..cell_rows {
        for cx in 0..buf.width {
            let target_x = cx as i32 + x_offset;
            if target_x < 0 || target_x >= scene_rect.width as i32 { continue; }
            // ... same half-block logic, writing to (target_x as u16, cy)
        }
    }
}
```

Direction: PageDown slides current floor LEFT, new floor enters from RIGHT. PageUp reverses.

### Themed Floor Variants

Each floor gets a `FloorVariant` enum that influences `SceneLayout::compute`:

```rust
enum FloorVariant {
    Standard,       // floor 0: current layout (cubicles + meeting + pantry)
    OpenPlan,       // floor 1: wider cubicle band, no meeting room, bigger windows
    Executive,      // floor 2: fewer desks, larger meeting room, more plants
    Lab,            // floor 3: dense desks, minimal decoration, more monitors
    Lounge,         // floor 4: mostly sofas and meeting areas, few desks
}
```

For v1, `Standard` and `OpenPlan` are implemented. Remaining variants (`Executive`, `Lab`, `Lounge`) are future work — the enum variant is passed to `SceneLayout::compute` and drives room proportions. The infrastructure supports adding variants without changing the render pipeline.

### UI Indicators

#### Footer

Extend `build_status_summary` to include floor info:

```
 12 agents · 3 active · 2 waiting · 7 idle    F1/3 [↑↓]    [p]ause [t]heme [q]uit
```

`F1/3` = floor 1 of 3. `[↑↓]` = PageUp/PageDown hint (only shown when num_floors > 1).

#### Neon Wall Display

Add floor number to the top line of `paint_wall_display`:

```
pixtuoid v0.2.0 ★ Star  Floor 2/3
●●●●●  ↑5m
[scrolling ticker...]
```

### Key Bindings

| Key | Action |
|---|---|
| PageUp | Navigate to previous floor (with slide transition) |
| PageDown | Navigate to next floor (with slide transition) |

No action if already at floor 0 (PageUp) or last floor (PageDown). No action during an active transition (debounce).

### Edge Cases

1. **Agent exits mid-transition**: The floor projection is rebuilt every frame, so a departing agent simply disappears from the filtered view. No special handling needed.
2. **Terminal resize during transition**: Cancel the transition, jump to `to_floor` immediately. Resize already invalidates routes; adding a transition cancel is one line.
3. **All agents on floor 2 exit**: Floor count drops. If `current_floor >= num_floors`, clamp to `num_floors - 1`. No orphaned empty floors.
4. **Theme switch during transition**: Each `FloorCtx` has its own `FrameCache`. `set_theme` flushes all caches in all contexts.
5. **max_desks changed via +/- keys**: This changes `desks_per_floor`, which shifts all floor assignments. Invalidate all floor contexts. Agents may "teleport" between floors — acceptable since +/- is a manual override.

## Build Sequence

### Phase 1 — Decouple render from SceneState

Change `render_to_rgb_buffer` to take `agents: &[&AgentSlot], max_desks: usize` instead of `&SceneState`. Update the single callsite in `TuiRenderer::render`. All existing tests pass unchanged.

### Phase 2 — Add FloorCtx and multi-buffer bookkeeping

Add `floor_bufs`, `floor_ctxs`, `current_floor` to `TuiRenderer`. Default-initialize with 1 entry. Render still does exactly what it does today (single floor, no transition). Tests pass.

### Phase 3 — Add floor projection

Add `FloorScene`, `agents_for_floor`, `floor_local_desk_index`. Wire into render path. Extend `next_free_desk` range to `max_desks * MAX_FLOORS`. Add floor count to footer. Unit test: 20 agents over `max_desks=16` yields 16 on floor 0, 4 on floor 1.

### Phase 4 — PageUp/PageDown navigation

Wire key events in `tui/mod.rs`. Add `navigate_floor`. Add `FloorTransition`. Write test: `FloorTransition::t()` returns 0.0 at start, 1.0 after duration.

### Phase 5 — Slide compositing

Implement `flush_buffer_to_term_at_offset`. Both floors render into independent buffers. Composited flush during active transitions. Visual verification via snapshot.

### Phase 6 — Floor indicator on neon display

Update `paint_wall_display` to show "Floor N/M". Update CLAUDE.md and README.

## Files Modified

| File | Change |
|---|---|
| `tui/floor.rs` (NEW) | `FloorCtx`, `FloorTransition`, `FloorScene`, `agents_for_floor` |
| `tui/tui_renderer.rs` | Multi-buffer, floor navigation, transition state |
| `tui/renderer.rs` | `flush_buffer_to_term_at_offset`, floor indicator in footer/wall |
| `tui/mod.rs` | PageUp/PageDown key handling |
| `tui/pixel_painter/mod.rs` | `render_to_rgb_buffer` signature change |
| `state/mod.rs` | `next_free_desk` range extended to `max_desks * MAX_FLOORS` |

## Files Unchanged

| File | Why |
|---|---|
| `state/reducer.rs` | Floor is a visual concept, not a state concept |
| `layout/mod.rs` | Called per-floor with floor-local agent count, no floor awareness needed |
| `source/*.rs` | Sources don't know about floors |
| `pose.rs` | Poses are floor-agnostic |
