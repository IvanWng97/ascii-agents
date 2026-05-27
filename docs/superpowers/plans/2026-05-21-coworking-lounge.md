# Coworking-Lounge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the uniform cubicle-grid scene with a coworking-floor layout where agents seat / stand / walk between zones, driven by their CC state. Visible sense of place + visible reaction to activity.

**Architecture:** Three new files split rendering responsibility — `layout.rs` (pure zone math), `pose.rs` (pure state→pose derivation, including wander state machine), `renderer.rs` (rewritten to orchestrate). Sprites grow from the current 1 character pose to 4 (seated / typing / standing / walking) at smaller 8×10 / 6×12 footprints, plus two new decor sprites (couch, coffee station). The reducer is untouched.

**Tech Stack:** Rust, ratatui half-block (▀) renderer, existing `.sprite` text format + `Pack` loader, hand-drawn pixel art.

---

## File Structure

**Create:**
- `crates/pixtuoid/src/tui/layout.rs` — `Layout` struct + `Layout::compute()` zone math, with `#[cfg(test)] mod tests` inline.
- `crates/pixtuoid/src/tui/pose.rs` — `Pose` enum + `pose::derive(slot, now, &layout)` + wander state machine constants, with `#[cfg(test)] mod tests` inline.
- `assets/sprites/default/seated.sprite` (8×10)
- `assets/sprites/default/standing.sprite` (6×12)
- `assets/sprites/default/walking_0.sprite` (6×12)
- `assets/sprites/default/walking_1.sprite` (6×12)
- `assets/sprites/default/couch.sprite` (14×5)
- `assets/sprites/default/coffee.sprite` (8×8)

**Modify:**
- `assets/sprites/default/pack.toml` — palette gains `C` and `K`; animations rewritten.
- `assets/sprites/default/typing_0.sprite` — rewrite at 8×10.
- `assets/sprites/default/typing_1.sprite` — rewrite at 8×10.
- `crates/pixtuoid/src/tui/mod.rs` — `pub mod layout; pub mod pose;`
- `crates/pixtuoid/src/tui/embedded_pack.rs` — update `include_str!` list.
- `crates/pixtuoid/src/tui/renderer.rs` — gut `cubicle_grid` etc., delegate layout/pose, add `paint_lounge_decor`, refactor character drawing.
- `crates/pixtuoid-core/tests/sprite_format.rs` — update `default_pack_loads_with_required_animations`.

**Delete:**
- `assets/sprites/default/idle.sprite` — `seated.sprite` covers idle.
- `assets/sprites/default/typing_2.sprite` — cycle is 2 frames now.
- `assets/sprites/default/waiting.sprite` — `standing.sprite` + existing `?` bubble cover this.

Sprite art is drawn in tasks 8–11 *after* the wiring tasks land green with placeholder art (single-color rectangles), so we never have a broken `cargo test` in between.

---

## Task 1: Scaffold `layout.rs` with empty `Layout` struct

**Files:**
- Create: `crates/pixtuoid/src/tui/layout.rs`
- Modify: `crates/pixtuoid/src/tui/mod.rs:1-4`

- [ ] **Step 1: Create the module file with a placeholder type**

```rust
//! Zone-based scene layout for the top-down office.
//!
//! Splits a buf-pixel rectangle into three vertical bands (cubicle, walkway,
//! lounge), then computes one home-desk position per agent inside the cubicle
//! band and a fixed set of named waypoints inside the lounge band. Pure
//! function — no I/O, no time, no buffer.

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub buf_w: u16,
    pub buf_h: u16,
    pub cubicle_band: Rect,
    pub walkway: Rect,
    pub lounge_band: Rect,
    pub home_desks: Vec<Point>,
    pub waypoints: Vec<Point>,
}

pub const WAYPOINT_COUNT: usize = 4;
pub const DESK_W: u16 = 12;
pub const DESK_H: u16 = 6;
pub const DESK_GAP_X: u16 = 4;
pub const DESK_GAP_Y: u16 = 2;

impl Layout {
    /// Returns `None` if the buffer is too small for even one cubicle and the
    /// fixed lounge area. Caller should paint a "terminal too small" message.
    pub fn compute(_buf_w: u16, _buf_h: u16, _num_agents: usize) -> Option<Self> {
        None // implemented in Task 2
    }
}

#[cfg(test)]
mod tests {
    // tests land in Task 2.
}
```

- [ ] **Step 2: Wire the module in tui/mod.rs**

Open `crates/pixtuoid/src/tui/mod.rs`, change line 1 from:
```rust
pub mod embedded_pack;
pub mod renderer;
```
to:
```rust
pub mod embedded_pack;
pub mod layout;
pub mod renderer;
```

- [ ] **Step 3: Verify compile**

Run: `cargo build --workspace`
Expected: `Finished dev profile` with no errors. The `unused` warning on `Layout` is acceptable for now.

- [ ] **Step 4: Commit**

```bash
git add crates/pixtuoid/src/tui/layout.rs crates/pixtuoid/src/tui/mod.rs
git commit -m "scaffold: tui::layout module for coworking-lounge"
```

---

## Task 2: Implement `Layout::compute` (TDD)

**Files:**
- Modify: `crates/pixtuoid/src/tui/layout.rs`

- [ ] **Step 1: Write the failing tests inside the `tests` module**

Replace the placeholder `#[cfg(test)] mod tests { ... }` block with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_returns_none_when_buf_too_small() {
        assert!(Layout::compute(20, 20, 4).is_none());
    }

    #[test]
    fn compute_zones_are_ordered_top_to_bottom_and_nonoverlapping() {
        let l = Layout::compute(120, 80, 6).expect("fits");
        assert!(l.cubicle_band.y < l.walkway.y);
        assert!(l.walkway.y < l.lounge_band.y);
        let c_bot = l.cubicle_band.y + l.cubicle_band.height;
        let w_bot = l.walkway.y + l.walkway.height;
        assert!(c_bot <= l.walkway.y, "cubicle overlaps walkway");
        assert!(w_bot <= l.lounge_band.y, "walkway overlaps lounge");
    }

    #[test]
    fn compute_places_one_home_desk_per_agent() {
        let l = Layout::compute(120, 80, 5).expect("fits");
        assert_eq!(l.home_desks.len(), 5);
        for d in &l.home_desks {
            assert!(d.y >= l.cubicle_band.y);
            assert!(d.y + DESK_H <= l.cubicle_band.y + l.cubicle_band.height);
        }
    }

    #[test]
    fn compute_places_exactly_waypoint_count_waypoints_in_lounge() {
        let l = Layout::compute(120, 80, 1).expect("fits");
        assert_eq!(l.waypoints.len(), WAYPOINT_COUNT);
        for w in &l.waypoints {
            assert!(w.y >= l.lounge_band.y);
            assert!(w.y < l.lounge_band.y + l.lounge_band.height);
            assert!(w.x < l.buf_w);
        }
    }

    #[test]
    fn compute_truncates_home_desks_when_more_agents_than_fit() {
        // 30 cells wide buffer, DESK_W=12 + GAP=4 = 16 per column → 1 col.
        let l = Layout::compute(30, 80, 20).expect("fits");
        assert!(l.home_desks.len() < 20, "should clamp to what fits");
        assert!(!l.home_desks.is_empty(), "should fit at least 1");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --workspace --lib --features pixtuoid-core/test-renderer layout::`
Expected: 5 failures with "called `Option::unwrap()` on a `None` value" / "fits".

- [ ] **Step 3: Implement `Layout::compute`**

Replace the body of `pub fn compute` with:

```rust
    pub fn compute(buf_w: u16, buf_h: u16, num_agents: usize) -> Option<Self> {
        const MIN_W: u16 = DESK_W + DESK_GAP_X * 2;
        const MIN_H: u16 = 40;
        if buf_w < MIN_W || buf_h < MIN_H {
            return None;
        }

        // Vertical split: 50% cubicle band, 15% walkway, 35% lounge.
        let cubicle_h = buf_h * 50 / 100;
        let walkway_h = buf_h * 15 / 100;
        let lounge_h = buf_h - cubicle_h - walkway_h;
        let cubicle_band = Rect { x: 0, y: 0, width: buf_w, height: cubicle_h };
        let walkway = Rect { x: 0, y: cubicle_h, width: buf_w, height: walkway_h };
        let lounge_band = Rect {
            x: 0,
            y: cubicle_h + walkway_h,
            width: buf_w,
            height: lounge_h,
        };

        // Home desks: pack into the cubicle band as a grid.
        let col_w = DESK_W + DESK_GAP_X;
        let row_h = DESK_H + DESK_GAP_Y;
        let cols = ((buf_w - DESK_GAP_X) / col_w).max(1);
        let rows = (cubicle_h / row_h).max(1);
        let max_desks = (cols * rows) as usize;
        let n = num_agents.min(max_desks);
        let mut home_desks = Vec::with_capacity(n);
        for i in 0..n {
            let r = (i as u16) / cols;
            let c = (i as u16) % cols;
            home_desks.push(Point {
                x: DESK_GAP_X + c * col_w,
                y: cubicle_band.y + DESK_GAP_Y + r * row_h,
            });
        }

        // Waypoints: 4 fixed positions evenly spaced in the lounge band.
        let waypoint_y = lounge_band.y + lounge_band.height / 2;
        let stride = buf_w / (WAYPOINT_COUNT as u16 + 1);
        let waypoints: Vec<Point> = (1..=WAYPOINT_COUNT as u16)
            .map(|i| Point { x: stride * i, y: waypoint_y })
            .collect();

        Some(Self {
            buf_w,
            buf_h,
            cubicle_band,
            walkway,
            lounge_band,
            home_desks,
            waypoints,
        })
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --workspace --lib --features pixtuoid-core/test-renderer layout::`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/pixtuoid/src/tui/layout.rs
git commit -m "feat(tui): Layout::compute with zone math + tests"
```

---

## Task 3: Scaffold `pose.rs` with `Pose` enum + constants

**Files:**
- Create: `crates/pixtuoid/src/tui/pose.rs`
- Modify: `crates/pixtuoid/src/tui/mod.rs`

- [ ] **Step 1: Create the module file**

```rust
//! State → pose derivation for the coworking-lounge renderer.
//!
//! Pure function: given an `AgentSlot`, current `SystemTime`, and `Layout`,
//! returns which `Pose` the agent should appear in this frame. Includes the
//! wander state machine for Idle agents (cycles between desk and waypoints).

use std::time::{Duration, SystemTime};

use pixtuoid_core::state::{ActivityState, AgentSlot};

use crate::tui::layout::{Layout, Point};

/// Length of one full wander cycle. After 9 seconds we loop.
pub const WANDER_CYCLE_MS: u64 = 9_000;
/// Per-phase boundaries (cumulative).
const PHASE_SEATED_END: u64 = 3_500;
const PHASE_WALK_OUT_END: u64 = 5_000;
const PHASE_AT_WAYPOINT_END: u64 = 7_500;
/// PHASE_WALK_BACK_END == WANDER_CYCLE_MS.

/// Frame-cycle period for animated poses.
pub const TYPING_FRAME_MS: u64 = 140;
pub const WALKING_FRAME_MS: u64 = 220;
pub const TYPING_FRAMES: usize = 2;
pub const WALKING_FRAMES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    SeatedIdle,
    SeatedTyping { frame: usize },
    StandingAtDesk,
    StandingAtWaypoint { wp: usize },
    Walking { from: Point, to: Point, t_x1000: u16, frame: usize },
}

/// Returns `None` if the slot's desk_index is out of range for `layout`.
pub fn derive(_slot: &AgentSlot, _now: SystemTime, _layout: &Layout) -> Option<Pose> {
    None // implemented in Task 4
}

fn _unused() {
    // Suppress dead_code while scaffolding.
    let _ = (PHASE_SEATED_END, PHASE_WALK_OUT_END, PHASE_AT_WAYPOINT_END);
    let _ = (Duration::from_millis(0),);
}

#[cfg(test)]
mod tests {
    // tests land in Task 4.
}
```

> Note on `t_x1000: u16` — we encode the lerp parameter as integer thousandths (0..=1000) instead of `f32` so `Pose` stays `PartialEq + Eq + Copy`, which makes the tests far easier to write.

- [ ] **Step 2: Wire in tui/mod.rs**

Add `pub mod pose;` to `crates/pixtuoid/src/tui/mod.rs` so it reads:

```rust
pub mod embedded_pack;
pub mod layout;
pub mod pose;
pub mod renderer;
```

- [ ] **Step 3: Verify compile**

Run: `cargo build --workspace`
Expected: clean build (warnings on unused items are fine while scaffolding).

- [ ] **Step 4: Commit**

```bash
git add crates/pixtuoid/src/tui/pose.rs crates/pixtuoid/src/tui/mod.rs
git commit -m "scaffold: tui::pose module with Pose enum + constants"
```

---

## Task 4: Implement `pose::derive` (TDD)

**Files:**
- Modify: `crates/pixtuoid/src/tui/pose.rs`

- [ ] **Step 1: Write the failing tests**

Replace the `#[cfg(test)] mod tests {}` block at the bottom of `pose.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;
    use pixtuoid_core::source::Activity;
    use pixtuoid_core::AgentId;

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
        Layout::compute(120, 80, 4).expect("fits")
    }

    fn typing() -> ActivityState {
        ActivityState::Active {
            activity: Activity::Typing,
            tool_use_id: Some("t".into()),
            detail: Some("Edit".into()),
        }
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
        let (s, now) = slot(ActivityState::Idle, PHASE_SEATED_END - 1);
        let l = layout();
        assert_eq!(derive(&s, now, &l), Some(Pose::SeatedIdle));
    }

    #[test]
    fn idle_phase_1_is_walking_out() {
        // Halfway through walk-out (3500..5000), t=0.5 → t_x1000 ≈ 500.
        let (s, now) = slot(ActivityState::Idle, 4_250);
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
        let (s, now) = slot(ActivityState::Idle, 6_000);
        let l = layout();
        match derive(&s, now, &l).expect("pose") {
            Pose::StandingAtWaypoint { wp } => assert!(wp < l.waypoints.len()),
            other => panic!("expected StandingAtWaypoint, got {other:?}"),
        }
    }

    #[test]
    fn idle_phase_3_is_walking_back() {
        let (s, now) = slot(ActivityState::Idle, 8_250);
        let l = layout();
        match derive(&s, now, &l).expect("pose") {
            Pose::Walking { t_x1000, .. } => {
                assert!((400..=600).contains(&t_x1000));
            }
            other => panic!("expected Walking, got {other:?}"),
        }
    }

    #[test]
    fn idle_cycle_loops_after_wander_cycle_ms() {
        let (s_early, now_early) = slot(ActivityState::Idle, 1_000);
        let (s_loop, now_loop) = slot(ActivityState::Idle, 1_000 + WANDER_CYCLE_MS);
        let l = layout();
        assert_eq!(derive(&s_early, now_early, &l), derive(&s_loop, now_loop, &l));
    }

    #[test]
    fn derive_returns_none_when_desk_index_out_of_range() {
        let (mut s, now) = slot(ActivityState::Idle, 0);
        s.desk_index = 999;
        assert!(derive(&s, now, &layout()).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --workspace --lib --features pixtuoid-core/test-renderer pose::`
Expected: 8 failures, all from `derive` returning `None`.

- [ ] **Step 3: Implement `derive` and remove the `_unused` stub**

Delete the `fn _unused() {...}` block. Replace the body of `pub fn derive` with:

```rust
pub fn derive(slot: &AgentSlot, now: SystemTime, layout: &Layout) -> Option<Pose> {
    let _desk = layout.home_desks.get(slot.desk_index)?;

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
        ActivityState::Idle => Some(idle_pose(slot, *_desk, layout, elapsed)),
    }
}

fn idle_pose(slot: &AgentSlot, desk: Point, layout: &Layout, elapsed_ms: u64) -> Pose {
    let phase_t = elapsed_ms % WANDER_CYCLE_MS;
    let wp_idx = (slot.agent_id.raw() as usize) % layout.waypoints.len();
    let wp = layout.waypoints[wp_idx];

    if phase_t < PHASE_SEATED_END {
        Pose::SeatedIdle
    } else if phase_t < PHASE_WALK_OUT_END {
        let span = PHASE_WALK_OUT_END - PHASE_SEATED_END;
        let t = ((phase_t - PHASE_SEATED_END) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        Pose::Walking { from: desk, to: wp, t_x1000: t, frame }
    } else if phase_t < PHASE_AT_WAYPOINT_END {
        Pose::StandingAtWaypoint { wp: wp_idx }
    } else {
        let span = WANDER_CYCLE_MS - PHASE_AT_WAYPOINT_END;
        let t = ((phase_t - PHASE_AT_WAYPOINT_END) * 1000 / span) as u16;
        let frame = ((elapsed_ms / WALKING_FRAME_MS) as usize) % WALKING_FRAMES;
        Pose::Walking { from: wp, to: desk, t_x1000: t, frame }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --workspace --lib --features pixtuoid-core/test-renderer pose::`
Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/pixtuoid/src/tui/pose.rs
git commit -m "feat(tui): pose::derive + wander state machine + tests"
```

---

## Task 5: Update `pack.toml` palette and animation list

**Files:**
- Modify: `assets/sprites/default/pack.toml`

- [ ] **Step 1: Add `C` and `K` palette keys**

Open `assets/sprites/default/pack.toml`. After the `"g" = "#c2734f"` line (plant pot), add:

```toml
"C" = "#7e6a55"   # couch fabric
"K" = "#a8a8b0"   # coffee station chrome
```

- [ ] **Step 2: Replace the animations block**

Find the existing `[animations.idle]` block down to the end of file. Replace everything from `[animations.idle]` to EOF with:

```toml
[animations.seated]
frames   = ["seated.sprite"]
frame_ms = 600

[animations.typing]
frames   = ["typing_0.sprite", "typing_1.sprite"]
frame_ms = 140

[animations.standing]
frames   = ["standing.sprite"]
frame_ms = 600

[animations.walking]
frames   = ["walking_0.sprite", "walking_1.sprite"]
frame_ms = 220

[animations.desk]
frames   = ["desk.sprite"]
frame_ms = 600

[animations.plant]
frames   = ["plant.sprite"]
frame_ms = 600

[animations.couch]
frames   = ["couch.sprite"]
frame_ms = 600

[animations.coffee]
frames   = ["coffee.sprite"]
frame_ms = 600
```

- [ ] **Step 3: Commit**

```bash
git add assets/sprites/default/pack.toml
git commit -m "feat(pack): coworking-lounge palette + animation list"
```

> The pack won't load yet — referenced .sprite files don't exist. Fixed in Task 6.

---

## Task 6: Add placeholder sprite files (compiles, ugly but loads)

**Files:**
- Create: `assets/sprites/default/seated.sprite`
- Create: `assets/sprites/default/standing.sprite`
- Create: `assets/sprites/default/walking_0.sprite`
- Create: `assets/sprites/default/walking_1.sprite`
- Create: `assets/sprites/default/couch.sprite`
- Create: `assets/sprites/default/coffee.sprite`
- Modify: `assets/sprites/default/typing_0.sprite`
- Modify: `assets/sprites/default/typing_1.sprite`

> These are intentionally minimal — recognizable shape at the right dimensions, but not art. The art polish happens in Tasks 11–12. The point of this task is to unblock the renderer wiring.

- [ ] **Step 1: Write `seated.sprite` (8×10, half-body chibi)**

```
@frame 0
. H H H H H H .
H H H H H H H H
H S S S S S S H
H S e S S e S H
. S S S m S S .
. B B B B B B .
B B B B B B B B
B B B B B B B B
. B B B B B B .
. . S S S S . .
```

- [ ] **Step 2: Rewrite `typing_0.sprite` to match new 8×10 footprint**

```
@frame 0
. H H H H H H .
H H H H H H H H
H S S S S S S H
H S e S S e S H
. S S S m S S .
. B B B B B B .
B B B B B B B B
B B B B B B B B
S B B B B B B S
. . S . . . S .
```

- [ ] **Step 3: Rewrite `typing_1.sprite` (alternate frame, arms tighter)**

```
@frame 0
. H H H H H H .
H H H H H H H H
H S S S S S S H
H S e S S e S H
. S S S m S S .
. B B B B B B .
B B B B B B B B
S B B B B B B S
. B B B B B B .
. S . . . . S .
```

- [ ] **Step 4: Write `standing.sprite` (6×12, taller standing pose)**

```
@frame 0
. H H H H .
H H H H H H
H S S S S H
H S e e S H
. S S m S .
. B B B B .
B B B B B B
B B B B B B
. B B B B .
. B B B B .
. S . . S .
. S . . S .
```

- [ ] **Step 5: Write `walking_0.sprite` (6×12, left foot forward)**

```
@frame 0
. H H H H .
H H H H H H
H S S S S H
H S e e S H
. S S m S .
. B B B B .
B B B B B B
B B B B B B
. B B B B .
. B B B B .
S . . . S .
S . . . . .
```

- [ ] **Step 6: Write `walking_1.sprite` (6×12, right foot forward)**

```
@frame 0
. H H H H .
H H H H H H
H S S S S H
H S e e S H
. S S m S .
. B B B B .
B B B B B B
B B B B B B
. B B B B .
. B B B B .
. S . . . S
. . . . . S
```

- [ ] **Step 7: Write `couch.sprite` (14×5)**

```
@frame 0
. C C C C C C C C C C C C .
C C C C C C C C C C C C C C
C C C C C C C C C C C C C C
C C C C C C C C C C C C C C
n n n n n n n n n n n n n n
```

- [ ] **Step 8: Write `coffee.sprite` (8×8)**

```
@frame 0
K K K K K K K K
K n n n n n n K
K n c c c c n K
K n c c c c n K
K n n n n n n K
K K K K K K K K
K K K K K K K K
n n K K K K n n
```

- [ ] **Step 9: Delete the now-obsolete sprites**

```bash
git rm assets/sprites/default/idle.sprite \
       assets/sprites/default/typing_2.sprite \
       assets/sprites/default/waiting.sprite
```

- [ ] **Step 10: Commit**

```bash
git add assets/sprites/default/
git commit -m "feat(pack): placeholder sprites for coworking-lounge poses"
```

---

## Task 7: Update `embedded_pack.rs` include list

**Files:**
- Modify: `crates/pixtuoid/src/tui/embedded_pack.rs`

- [ ] **Step 1: Replace the file contents**

Open `crates/pixtuoid/src/tui/embedded_pack.rs`. Replace the body of `load_default_pack`:

```rust
//! Embeds the bundled top-down sprite pack into the binary at compile time.

use anyhow::Result;
use pixtuoid_core::sprite::format::{load_pack_from_strings, Pack};

pub fn load_default_pack() -> Result<Pack> {
    let pack_toml = include_str!("../../../../assets/sprites/default/pack.toml");
    let seated     = include_str!("../../../../assets/sprites/default/seated.sprite");
    let typing_0   = include_str!("../../../../assets/sprites/default/typing_0.sprite");
    let typing_1   = include_str!("../../../../assets/sprites/default/typing_1.sprite");
    let standing   = include_str!("../../../../assets/sprites/default/standing.sprite");
    let walking_0  = include_str!("../../../../assets/sprites/default/walking_0.sprite");
    let walking_1  = include_str!("../../../../assets/sprites/default/walking_1.sprite");
    let desk       = include_str!("../../../../assets/sprites/default/desk.sprite");
    let plant      = include_str!("../../../../assets/sprites/default/plant.sprite");
    let couch      = include_str!("../../../../assets/sprites/default/couch.sprite");
    let coffee     = include_str!("../../../../assets/sprites/default/coffee.sprite");

    load_pack_from_strings(
        pack_toml,
        &[
            ("seated.sprite", seated),
            ("typing_0.sprite", typing_0),
            ("typing_1.sprite", typing_1),
            ("standing.sprite", standing),
            ("walking_0.sprite", walking_0),
            ("walking_1.sprite", walking_1),
            ("desk.sprite", desk),
            ("plant.sprite", plant),
            ("couch.sprite", couch),
            ("coffee.sprite", coffee),
        ],
    )
}
```

- [ ] **Step 2: Verify pack loads at compile + test time**

Run: `cargo build --workspace`
Expected: clean build.

Run: `cargo test --workspace --features pixtuoid-core/test-renderer sprite_format::tests::default_pack_loads`
Expected: still fails because the test asserts old animation names.

- [ ] **Step 3: Update the sprite_format test**

Open `crates/pixtuoid-core/tests/sprite_format.rs`. Replace the `default_pack_loads_with_required_animations` test body:

```rust
#[test]
fn default_pack_loads_with_required_animations() {
    let pack = load_pack(Path::new("../../assets/sprites/default")).unwrap();
    for name in &[
        "seated", "typing", "standing", "walking",
        "desk", "plant", "couch", "coffee",
    ] {
        assert!(pack.animation(name).is_some(), "missing animation: {name}");
    }
    let seated = pack.animation("seated").unwrap();
    assert_eq!(seated.frames[0].width, 8);
    assert_eq!(seated.frames[0].height, 10);

    let standing = pack.animation("standing").unwrap();
    assert_eq!(standing.frames[0].width, 6);
    assert_eq!(standing.frames[0].height, 12);

    let walking = pack.animation("walking").unwrap();
    assert_eq!(walking.frames.len(), 2);
}
```

- [ ] **Step 4: Verify the pack test passes**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer sprite_format`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pixtuoid/src/tui/embedded_pack.rs \
        crates/pixtuoid-core/tests/sprite_format.rs
git commit -m "feat(pack): wire coworking-lounge animations into embedded loader"
```

> The TUI itself is currently broken (renderer.rs still calls `pack.animation("idle")` etc.). Fixed in Task 8.

---

## Task 8: Gut `renderer.rs` and rebuild around `Layout` + `Pose`

**Files:**
- Modify: `crates/pixtuoid/src/tui/renderer.rs` (substantial rewrite)

- [ ] **Step 1: Replace the entire file**

Overwrite `crates/pixtuoid/src/tui/renderer.rs` with:

```rust
//! Top-down coworking-lounge renderer.
//!
//! Zone-based layout via `tui::layout`, state→pose derivation via `tui::pose`.
//! This file owns the actual pixel painting (floor, walls, decor, character
//! sprites, terminal flush). Layout and pose are pure functions tested in
//! isolation; this file is the integrator.

use std::io::{stdout, Stdout};
use std::time::SystemTime;

use anyhow::Result;
use pixtuoid_core::sprite::blit::blit_frame;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentSlot, SceneState};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

use crate::tui::layout::{Layout, Point, DESK_H, DESK_W};
use crate::tui::pose::{self, Pose};

pub type Term = Terminal<CrosstermBackend<Stdout>>;

// --- Colors ---------------------------------------------------------------
const BG: Rgb = Rgb(28, 32, 40);
const PLANK_A: Rgb = Rgb(120, 84, 50);
const PLANK_B: Rgb = Rgb(100, 70, 38);
const PLANK_LINE: Rgb = Rgb(72, 48, 24);
const WALL: Rgb = Rgb(56, 56, 70);
const WALL_TRIM: Rgb = Rgb(80, 80, 100);
const BASEBOARD: Rgb = Rgb(40, 40, 52);
const RUG_PALETTE: &[Rgb] = &[
    Rgb(0x4a, 0x55, 0x80),
    Rgb(0x6a, 0x3f, 0x55),
    Rgb(0x40, 0x60, 0x4f),
    Rgb(0x6e, 0x4d, 0x2e),
];
const SHIRT_PRESETS: &[Rgb] = &[
    Rgb(0x2e, 0x62, 0xcf),
    Rgb(0x16, 0xa0, 0x6e),
    Rgb(0xb0, 0x32, 0xa8),
    Rgb(0xc6, 0x6a, 0x1e),
    Rgb(0x6c, 0x4f, 0x9e),
    Rgb(0x9c, 0x27, 0x27),
    Rgb(0x32, 0x82, 0x9b),
    Rgb(0x80, 0x55, 0x32),
];
const HAIR_PRESETS: &[Rgb] = &[
    Rgb(0x2a, 0x1a, 0x0e),
    Rgb(0x52, 0x32, 0x10),
    Rgb(0xc7, 0xa3, 0x4a),
    Rgb(0x7a, 0x32, 0x10),
    Rgb(0x3a, 0x3a, 0x3a),
];

// --- Terminal lifecycle ---------------------------------------------------
pub fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

pub fn teardown_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    Ok(())
}

// --- Per-agent recolor ----------------------------------------------------
fn agent_palette(base: &Palette, agent: &AgentSlot) -> Palette {
    let seed = agent.agent_id.raw() as usize;
    let shirt = SHIRT_PRESETS[seed % SHIRT_PRESETS.len()];
    let hair = HAIR_PRESETS[(seed / 7) % HAIR_PRESETS.len()];
    base.with_override('B', Some(shirt))
        .with_override('H', Some(hair))
}

fn recolor_frame(frame: &Frame, pal: &Palette, base_pal: &Palette) -> Frame {
    let base_shirt = base_pal.get('B').flatten();
    let base_hair = base_pal.get('H').flatten();
    let agent_shirt = pal.get('B').flatten();
    let agent_hair = pal.get('H').flatten();
    let pixels: Vec<Pixel> = frame
        .pixels
        .iter()
        .map(|p| match p {
            Some(rgb) if Some(*rgb) == base_shirt => agent_shirt,
            Some(rgb) if Some(*rgb) == base_hair => agent_hair,
            other => *other,
        })
        .collect();
    Frame {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

// --- Floor / walls / decor -----------------------------------------------
fn paint_floor_and_walls(buf: &mut RgbBuffer, buf_w: u16, buf_h: u16) {
    const PLANK_H: u16 = 6;
    const TOP_WALL_H: u16 = 6;
    const BASEBOARD_H: u16 = 3;

    for y in 0..buf_h {
        let band = y / PLANK_H;
        let seam_offset = (band as u32 * 13) % 16;
        for x in 0..buf_w {
            let in_seam = y % PLANK_H == PLANK_H - 1
                || ((x as u32).wrapping_add(seam_offset)) % 16 == 0;
            let color = if in_seam {
                PLANK_LINE
            } else if band % 2 == 0 {
                PLANK_A
            } else {
                PLANK_B
            };
            buf.put(x, y, color);
        }
    }
    for y in 0..TOP_WALL_H.min(buf_h) {
        for x in 0..buf_w {
            buf.put(x, y, WALL);
        }
    }
    if TOP_WALL_H < buf_h {
        for x in 0..buf_w {
            buf.put(x, TOP_WALL_H, WALL_TRIM);
        }
    }
    let base_y = buf_h.saturating_sub(BASEBOARD_H);
    for y in base_y..buf_h {
        for x in 0..buf_w {
            buf.put(x, y, BASEBOARD);
        }
    }
}

fn paint_rug(buf: &mut RgbBuffer, x: u16, y: u16, w: u16, h: u16, color: Rgb) {
    let lighter = Rgb(
        color.0.saturating_add(40),
        color.1.saturating_add(40),
        color.2.saturating_add(40),
    );
    for dy in 1..h.saturating_sub(1) {
        for dx in 1..w.saturating_sub(1) {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width || py >= buf.height {
                continue;
            }
            let on_border = dy == 1 || dy + 2 == h || dx == 1 || dx + 2 == w;
            buf.put(px, py, if on_border { lighter } else { color });
        }
    }
}

fn paint_lounge_decor(buf: &mut RgbBuffer, layout: &Layout, pack: &Pack) {
    // Couch sits on the left side of the lounge band.
    if let Some(couch) = pack.animation("couch").and_then(|a| a.frames.first()) {
        let cx = layout.lounge_band.x + 2;
        let cy = layout.lounge_band.y + (layout.lounge_band.height.saturating_sub(couch.height)) / 2;
        blit_frame(couch, cx, cy, buf);
    }
    // Coffee station on the right side.
    if let Some(coffee) = pack.animation("coffee").and_then(|a| a.frames.first()) {
        let cx = layout
            .lounge_band
            .x
            .saturating_add(layout.lounge_band.width)
            .saturating_sub(coffee.width + 4);
        let cy = layout.lounge_band.y + (layout.lounge_band.height.saturating_sub(coffee.height)) / 2;
        blit_frame(coffee, cx, cy, buf);
    }
    // Plants at each waypoint that isn't covered by couch/coffee.
    if let Some(plant) = pack.animation("plant").and_then(|a| a.frames.first()) {
        for (i, wp) in layout.waypoints.iter().enumerate() {
            if i == 0 || i == layout.waypoints.len() - 1 {
                continue; // skip first/last — couch & coffee live there
            }
            let px = wp.x.saturating_sub(plant.width / 2);
            let py = wp.y.saturating_sub(plant.height / 2);
            blit_frame(plant, px, py, buf);
        }
    }
}

// --- Character placement --------------------------------------------------
fn seated_anchor(desk: Point) -> Point {
    // 8×10 sprite centered on the desk top.
    Point {
        x: desk.x + DESK_W.saturating_sub(8) / 2,
        y: desk.y.saturating_sub(8),
    }
}

fn standing_at_desk_anchor(desk: Point) -> Point {
    // 6×12 sprite next to the desk, taller so it stands "up".
    Point {
        x: desk.x + DESK_W.saturating_sub(6) / 2,
        y: desk.y.saturating_sub(12),
    }
}

fn walking_anchor(p: Point) -> Point {
    Point {
        x: p.x.saturating_sub(3),
        y: p.y.saturating_sub(12),
    }
}

fn waypoint_anchor(wp: Point) -> Point {
    Point {
        x: wp.x.saturating_sub(3),
        y: wp.y.saturating_sub(12),
    }
}

fn walking_position(from: Point, to: Point, t_x1000: u16) -> Point {
    let t = t_x1000 as i32;
    let dx = to.x as i32 - from.x as i32;
    let dy = to.y as i32 - from.y as i32;
    Point {
        x: (from.x as i32 + dx * t / 1000) as u16,
        y: (from.y as i32 + dy * t / 1000) as u16,
    }
}

/// Paint a character at an arbitrary anchor with per-agent recolor.
/// `anchor` is buf-pixel top-left of where the sprite blits.
fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
) {
    let base_pal = pack.palette.clone();
    let pal = agent_palette(&base_pal, agent);
    let Some(anim) = pack.animation(anim_name) else { return };
    let Some(frame) = anim.frames.get(frame_idx).or_else(|| anim.frames.first()) else { return };
    let recolored = recolor_frame(frame, &pal, &base_pal);
    blit_frame(&recolored, anchor.x, anchor.y, buf);
}

// --- Speech bubble overlay (kept from the prior renderer) -----------------
fn paint_waiting_bubble(buf: &mut RgbBuffer, anchor: Point) {
    // Tiny `?` bubble drawn directly into the buf so it tracks the character.
    // 5x3 px, centered above the head.
    const BUBBLE_FG: Rgb = Rgb(240, 200, 80);
    const BUBBLE_BG: Rgb = Rgb(30, 30, 40);
    let bx = anchor.x;
    let by = anchor.y.saturating_sub(4);
    let dots: &[(u16, u16, Rgb)] = &[
        (0, 0, BUBBLE_BG), (1, 0, BUBBLE_BG), (2, 0, BUBBLE_BG), (3, 0, BUBBLE_BG), (4, 0, BUBBLE_BG),
        (0, 1, BUBBLE_BG), (2, 1, BUBBLE_FG), (4, 1, BUBBLE_BG),
        (0, 2, BUBBLE_BG), (1, 2, BUBBLE_BG), (2, 2, BUBBLE_FG), (3, 2, BUBBLE_BG), (4, 2, BUBBLE_BG),
    ];
    for (dx, dy, c) in dots {
        let px = bx + dx;
        let py = by + dy;
        if px < buf.width && py < buf.height {
            buf.put(px, py, *c);
        }
    }
}

// --- draw_scene ----------------------------------------------------------
pub fn draw_scene<B: Backend>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    pack: &Pack,
    now: SystemTime,
    buf: &mut RgbBuffer,
) -> Result<()> {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    term.draw(|f| {
        let size = f.area();

        let title = Paragraph::new(Line::from(vec![
            Span::raw(" pixtuoid — "),
            Span::raw(format!(
                "{} session{} ",
                agents.len(),
                if agents.len() == 1 { "" } else { "s" }
            )),
        ]));
        f.render_widget(
            title,
            Rect { x: size.x, y: size.y, width: size.width, height: 1 },
        );

        let footer = Paragraph::new(Span::raw(" [q] quit "))
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(
            footer,
            Rect {
                x: size.x,
                y: size.y + size.height.saturating_sub(1),
                width: size.width,
                height: 1,
            },
        );

        let scene_rect = Rect {
            x: size.x,
            y: size.y + 1,
            width: size.width,
            height: size.height.saturating_sub(2),
        };
        if scene_rect.width < 20 || scene_rect.height < 12 {
            return;
        }

        let buf_w = scene_rect.width;
        let buf_h = scene_rect.height * 2;
        buf.ensure_size(buf_w, buf_h, BG);

        let Some(layout) = Layout::compute(buf_w, buf_h, agents.len()) else {
            return;
        };

        paint_floor_and_walls(buf, buf_w, buf_h);
        paint_lounge_decor(buf, &layout, pack);

        // Pass 1: rugs (per-cubicle backdrop).
        let desk_anim = pack.animation("desk");
        for (i, desk) in layout.home_desks.iter().enumerate() {
            let agent = &agents[i];
            let rug = RUG_PALETTE[(agent.agent_id.raw() as usize / 11) % RUG_PALETTE.len()];
            paint_rug(
                buf,
                desk.x.saturating_sub(1),
                desk.y.saturating_sub(10),
                DESK_W + 2,
                DESK_H + 12,
                rug,
            );
            if let Some(frame) = desk_anim.and_then(|a| a.frames.first()) {
                blit_frame(frame, desk.x, desk.y, buf);
            }
        }

        // Pass 2: characters (per-agent pose). Each match arm resolves the
        // (animation, frame, anchor) tuple and then calls paint_character_at.
        for (i, desk) in layout.home_desks.iter().enumerate() {
            let agent = &agents[i];
            let Some(p) = pose::derive(agent, now, &layout) else { continue };
            match p {
                Pose::SeatedIdle => {
                    paint_character_at(buf, "seated", 0, seated_anchor(*desk), agent, pack);
                }
                Pose::SeatedTyping { frame } => {
                    paint_character_at(buf, "typing", frame, seated_anchor(*desk), agent, pack);
                }
                Pose::StandingAtDesk => {
                    let anchor = standing_at_desk_anchor(*desk);
                    paint_character_at(buf, "standing", 0, anchor, agent, pack);
                    if matches!(agent.state, ActivityState::Waiting { .. }) {
                        paint_waiting_bubble(buf, anchor);
                    }
                }
                Pose::StandingAtWaypoint { wp } => {
                    if let Some(wp_pt) = layout.waypoints.get(wp) {
                        paint_character_at(
                            buf,
                            "standing",
                            0,
                            waypoint_anchor(*wp_pt),
                            agent,
                            pack,
                        );
                    }
                }
                Pose::Walking { from, to, t_x1000, frame } => {
                    let pos = walking_position(from, to, t_x1000);
                    paint_character_at(buf, "walking", frame, walking_anchor(pos), agent, pack);
                }
            }
        }

        // Flush half-block cells.
        let term_buf = f.buffer_mut();
        let w = buf.width as usize;
        let cell_rows = (buf.height / 2) as usize;
        for cy in 0..cell_rows {
            for cx in 0..(buf.width as usize) {
                let x = scene_rect.x + cx as u16;
                let y = scene_rect.y + cy as u16;
                if x >= scene_rect.x + scene_rect.width
                    || y >= scene_rect.y + scene_rect.height
                {
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

        // Labels above each home desk.
        for (i, desk) in layout.home_desks.iter().enumerate() {
            let agent = &agents[i];
            let lx = scene_rect.x + desk.x;
            let ly = scene_rect.y + (desk.y / 2).saturating_sub(1);
            let para = Paragraph::new(Span::styled(
                format!("{} {}", agent.label, summarize_state(&agent.state)),
                Style::default().fg(Color::White),
            ));
            f.render_widget(
                para,
                Rect { x: lx, y: ly, width: DESK_W + 4, height: 1 },
            );
        }
    })?;
    Ok(())
}

fn summarize_state(state: &ActivityState) -> &'static str {
    match state {
        ActivityState::Idle => "idle",
        ActivityState::Active { .. } => "working",
        ActivityState::Waiting { .. } => "waiting",
    }
}
```

- [ ] **Step 2: Verify all tests pass**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: all green (60+ tests).

- [ ] **Step 3: Verify the snapshot example renders**

Run: `cargo run --example snapshot --release -- /tmp/lounge.png`
Expected: writes `/tmp/lounge.png` with no error.

- [ ] **Step 4: Commit**

```bash
git add crates/pixtuoid/src/tui/renderer.rs
git commit -m "feat(tui): rewrite renderer around Layout + Pose with lounge zones"
```

---

## Task 9: Snapshot regression captures for each pose

**Files:**
- Modify: `crates/pixtuoid/examples/snapshot.rs:150-205` (the `sample_scene` function)

- [ ] **Step 1: Replace `sample_scene` with one that covers every pose**

Find `fn sample_scene(now: SystemTime) -> SceneState` and replace its body so the 7 agents collectively exercise: Active, Waiting, Idle just-started, Idle mid-walk-out, Idle at-waypoint, Idle mid-walk-back. Concrete time offsets line up with the phase boundaries in `pose.rs`:

```rust
fn sample_scene(now: SystemTime) -> SceneState {
    use std::time::Duration as D;
    let mut s = SceneState::new(12);
    let agents: [(&str, ActivityState, D); 7] = [
        ("working",   ActivityState::Active {
            activity: Activity::Typing,
            tool_use_id: Some("tu_a".into()),
            detail: Some("Write: src/foo.rs".into()),
        }, D::from_millis(0)),
        ("waiting",   ActivityState::Waiting { reason: "permission?".into() }, D::from_millis(0)),
        ("idle-sit",  ActivityState::Idle, D::from_millis(1_000)),       // phase 0
        ("walk-out",  ActivityState::Idle, D::from_millis(4_250)),       // phase 1
        ("at-wp",     ActivityState::Idle, D::from_millis(6_000)),       // phase 2
        ("walk-back", ActivityState::Idle, D::from_millis(8_250)),       // phase 3
        ("working-2", ActivityState::Active {
            activity: Activity::Typing,
            tool_use_id: Some("tu_b".into()),
            detail: Some("Edit: lib.rs".into()),
        }, D::from_millis(140)),                                          // mid typing cycle
    ];
    for (i, (key, state, age)) in agents.iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: "claude-code".into(),
                session_id: format!("demo-{key}"),
                cwd: PathBuf::from("/demo"),
                label: key.to_string(),
                state: state.clone(),
                state_started_at: now - *age,
                desk_index: i,
            },
        );
    }
    s
}
```

- [ ] **Step 2: Render the snapshot**

Run: `cargo run --example snapshot --release -- /tmp/lounge.png`
Expected: writes `/tmp/lounge.png`. Open it (`open /tmp/lounge.png`) and confirm 7 agents in distinct poses: working, waiting (bubble), 4 idle in different wander phases, working again.

- [ ] **Step 3: Commit**

```bash
git add crates/pixtuoid/examples/snapshot.rs
git commit -m "test(snapshot): sample scene exercises every coworking pose"
```

---

## Task 10: Verify live run end-to-end

**Files:** none (smoke-test step)

- [ ] **Step 1: Build release**

Run: `cargo build --release --workspace`
Expected: `Finished release profile` clean.

- [ ] **Step 2: Run the binary against a real CC session**

In one terminal: `./target/release/pixtuoid run`
In another terminal: start a CC session in any project and make it call a tool.

Expected: the working agent appears seated (typing animation), then returns to seated-idle, then after ~3.5s starts walking to a lounge waypoint, stands there ~2.5s, walks back. Cycle repeats while idle. Title bar shows the live session count.

- [ ] **Step 3: Kill the daemon when done**

`Ctrl-C` or `q`.

- [ ] **Step 4: No commit** (smoke test only)

---

## Task 11: Polish character art — `seated.sprite` + `typing_*`

**Files:**
- Modify: `assets/sprites/default/seated.sprite`
- Modify: `assets/sprites/default/typing_0.sprite`
- Modify: `assets/sprites/default/typing_1.sprite`

> Hand-drawing pixel art is iterative. The placeholders from Task 6 are recognizably a chibi person but blocky; this task refines them. Use the snapshot tool (`cargo run --example snapshot --release -- /tmp/lounge.png && open /tmp/lounge.png`) to iterate.

- [ ] **Step 1: Edit `seated.sprite`** to your taste while keeping 8 cols × 10 rows. Suggested goals:
  - Cleaner round head (use `n` for outline on the silhouette corners)
  - Distinct face features (eyes 2-pixel symmetrical, mouth dot)
  - Visible neck pixel between head and shirt
  - Hands visible at sides as `S` pixels in row 8

- [ ] **Step 2: Edit `typing_0.sprite` and `typing_1.sprite`** so the 2-frame cycle reads as subtle hand motion (one hand up vs down on the keyboard, ~1-2 px difference).

- [ ] **Step 3: Re-render and review**

Run: `cargo run --example snapshot --release -- /tmp/lounge.png && open /tmp/lounge.png`
Iterate until satisfied.

- [ ] **Step 4: Commit**

```bash
git add assets/sprites/default/seated.sprite assets/sprites/default/typing_*.sprite
git commit -m "art: polish seated + typing chibi"
```

---

## Task 12: Polish art — `standing.sprite` + `walking_*` + lounge decor

**Files:**
- Modify: `assets/sprites/default/standing.sprite`
- Modify: `assets/sprites/default/walking_0.sprite`
- Modify: `assets/sprites/default/walking_1.sprite`
- Modify: `assets/sprites/default/couch.sprite`
- Modify: `assets/sprites/default/coffee.sprite`

- [ ] **Step 1: Iterate on `standing.sprite`** at 6×12. Goals: visible head, torso, arms at sides, two legs.

- [ ] **Step 2: Iterate on `walking_0/1`**. Frame 0 = left foot forward, frame 1 = right foot forward. Otherwise identical body.

- [ ] **Step 3: Iterate on `couch.sprite`** (14×5). Goal: visible armrests on left/right, cushion mid-row, baseboard stripe at the bottom.

- [ ] **Step 4: Iterate on `coffee.sprite`** (8×8). Goal: machine body + dispenser opening + base.

- [ ] **Step 5: Re-render between every change**

Run: `cargo run --example snapshot --release -- /tmp/lounge.png && open /tmp/lounge.png`

- [ ] **Step 6: Commit**

```bash
git add assets/sprites/default/standing.sprite assets/sprites/default/walking_*.sprite assets/sprites/default/couch.sprite assets/sprites/default/coffee.sprite
git commit -m "art: polish standing, walking, couch, coffee"
```

---

## Task 13: Final pass — push and update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update sprite/animation footprints in CLAUDE.md**

Find the line:
```
assets/sprites/default/     bundled top-down pack (idle, typing x3, waiting, desk, plant) at 12×14 px
```
Replace with:
```
assets/sprites/default/     coworking-lounge pack (seated, typing x2, standing, walking x2, desk, plant, couch, coffee) at 8×10 + 6×12
```

- [ ] **Step 2: Replace the "How is the office laid out?" pointer**

Find the line in the "Where to look" section that mentions `cubicle_grid` and `paint_floor_and_walls`. Replace with:

```
- "How is the office laid out?" → `tui::layout::Layout::compute` for zone math + home-desk + waypoint placement; `tui::pose::derive` for state→pose mapping (incl. wander state machine); `tui::renderer::draw_scene` for pixel painting + flush.
```

- [ ] **Step 3: Commit + push**

```bash
git add CLAUDE.md
git commit -m "docs(CLAUDE): coworking-lounge layout pointer + sprite footprint"
git push origin main
```

---

## Self-review checklist (run before handing off)

- [ ] **Spec coverage:** Every section of `docs/superpowers/specs/2026-05-21-coworking-lounge-design.md` has a corresponding task here. Components 1–5 → Tasks 5/6 (sprites), Task 2 (layout), Task 4 (pose), Task 8 (renderer). Testing section → Tasks 2, 4, 9. File touch list → Tasks 5–8, 11–12.
- [ ] **Placeholder scan:** No "TBD", "TODO", or "implement later" in any task. Every code block is complete.
- [ ] **Type consistency:** `Pose` variants used in `pose.rs` tests (Task 4) match the dispatch in `renderer.rs` (Task 8). `Point` and `Layout` exported from `layout.rs` (Task 1) are imported by `pose.rs` (Task 3) and `renderer.rs` (Task 8). `Pose::Walking::t_x1000: u16` is the integer-thousandths trick noted in Task 3.
- [ ] **Existing tests preserved:** `sprite_format::default_pack_loads_with_required_animations` is updated in Task 7 (Step 3) — the only existing test that breaks.

---

## Execution choices

After saving this plan, two ways to run it:

**1. Subagent-Driven (recommended)** — One fresh subagent per task, review in between, fastest iteration on art.

**2. Inline Execution** — Execute tasks in this session with batch checkpoints; faster for the scaffolding tasks but riskier on the art passes.

**Which approach?**
