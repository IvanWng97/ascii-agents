# Atmosphere & World Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add five ambient effects (dust motes, sun position on wall, weather → indoor tint, ceiling reflections, accumulated coffee stains) to the pixel-art office. Each is independently revertable.

**Architecture:** New orchestrator module `pixel_painter/ambient.rs` between the background pass and the y-sorted drawable pass. Time-of-day driven effects extend `background/time_of_day.rs`. Coffee-stain accumulated state lives on `TuiRenderer`.

**Tech Stack:** Pure Rust, no new deps. `RgbBuffer` half-block blitting. Existing `Theme`, `Weather`, `tool_glow_tint` APIs.

**Spec:** `docs/superpowers/specs/2026-05-28-v0.5.0-visual-refresh-design.md` (on `docs/v0.5-visual-refresh-spec` branch). This plan adjusts two spec assumptions that don't match reality:
- The spec calls `paint_sunbeam`; the actual painter is `paint_window_light_spill` in `background/mod.rs:163` (12-row warm spill below each window). Dust motes anchor to that spill's geometry.
- The spec assumes `theme.kind == ThemeKind::Dark`; no such field exists. Task 4 adds it (`pub kind: ThemeKind` on `Theme`) and populates each of 6 themes.

**Branch:** `feat/pr53-atmosphere` (already cut off main).

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/pixtuoid/src/tui/pixel_painter/ambient.rs` | **new** | Orchestrator for the ambient pass. Exposes `paint_ambient(ctx)`. |
| `crates/pixtuoid/src/tui/pixel_painter/mod.rs` | modify | Call `ambient::paint_ambient` after background, before drawables. Re-export. |
| `crates/pixtuoid/src/tui/pixel_painter/background/time_of_day.rs` | modify | Add `sun_on_wall(now) -> Option<SunSpot>` + `sun_intensity(now) -> f32`. |
| `crates/pixtuoid/src/tui/pixel_painter/background/mod.rs` | modify | Add `weather_indoor_tint(weather) -> Rgb`. Apply in `paint_floor_and_walls` floor pass. |
| `crates/pixtuoid/src/tui/pixel_painter/drawable.rs` | modify | New `Drawable::CeilingHalo` variant. Inserted by `paint_ambient` callers. |
| `crates/pixtuoid/src/tui/pixel_painter/palette.rs` | modify | `tool_glow_tint` already returns `Rgb`; reuse for ceiling halo. |
| `crates/pixtuoid/src/tui/theme/mod.rs` | modify | Add `pub kind: ThemeKind`. New enum `ThemeKind { Light, Dark }`. |
| `crates/pixtuoid/src/tui/theme/{normal,cyberpunk,dracula,tokyo_night,catppuccin,gruvbox}.rs` | modify | Each populates `kind`. |
| `crates/pixtuoid/src/tui/tui_renderer.rs` | modify | Add `coffee_stains: HashMap<AgentId, Vec<StainPos>>`. Insert on pantry-trip detection. Evict in `evict_missing`. |
| `crates/pixtuoid/src/tui/pixel_painter/drawable.rs` | modify | Paint accumulated stains in `paint_desk_personalization`. |
| `crates/pixtuoid/tests/pixel_painter.rs` *(may not exist; create if needed)* | new/modify | Snapshot + unit tests for atmosphere effects. |

---

## Task 0: Create `ambient.rs` scaffold + wire into render order

**Files:**
- Create: `crates/pixtuoid/src/tui/pixel_painter/ambient.rs`
- Modify: `crates/pixtuoid/src/tui/pixel_painter/mod.rs`

- [ ] **Step 1: Write a smoke test that `paint_ambient` exists and compiles**

Add to `crates/pixtuoid/src/tui/pixel_painter/ambient.rs` (new file):

```rust
//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: sun spot on wall, dust
//! motes in window spill, ceiling halos above active monitors.
//!
//! Each subroutine is independently togglable and self-contained. New
//! ambient effects go here, not in `background/` or `drawable.rs`.

use crate::tui::pixel_painter::PixelCtx;

pub(super) fn paint_ambient(_ctx: &mut PixelCtx<'_>) {
    // Subroutines added in Tasks 1–4. No-op for now.
}

#[cfg(test)]
mod tests {
    #[test]
    fn ambient_module_compiles() {
        // Placeholder: replaced by per-feature tests in later tasks.
    }
}
```

- [ ] **Step 2: Wire `mod ambient;` into `pixel_painter/mod.rs`**

In `crates/pixtuoid/src/tui/pixel_painter/mod.rs`, add `mod ambient;` next to the existing `mod` declarations (`mod anchors;`, `mod drawable;`, etc.).

- [ ] **Step 3: Call `ambient::paint_ambient(ctx)` after background, before drawables**

Locate the orchestrator in `render_to_rgb_buffer` (search for `paint_floor_and_walls` call). Insert `ambient::paint_ambient(&mut ctx);` immediately after the background pass and before the y-sorted drawable loop.

- [ ] **Step 4: Build + test**

Run: `cargo build --workspace && cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: PASS — no behavior change; orchestrator wired.

- [ ] **Step 5: Commit**

```bash
git add crates/pixtuoid/src/tui/pixel_painter/ambient.rs crates/pixtuoid/src/tui/pixel_painter/mod.rs
git commit -m "feat(atmosphere): scaffold ambient pass module"
```

---

## Task 1: Sun position on wall (spec §2)

**Files:**
- Modify: `crates/pixtuoid/src/tui/pixel_painter/background/time_of_day.rs`
- Modify: `crates/pixtuoid/src/tui/pixel_painter/ambient.rs`
- Test: `crates/pixtuoid/src/tui/pixel_painter/background/time_of_day.rs` (unit tests live alongside)

### Step 1: Failing unit test for `sun_on_wall`

Add to `time_of_day.rs` `mod tests`:

```rust
#[test]
fn sun_on_wall_east_at_morning() {
    let morning = epoch_at_local(7, 0);
    let spot = sun_on_wall(morning).expect("sun on wall in morning");
    assert!(matches!(spot.wall, WallSide::East));
    assert!(spot.warmth > 0.5, "morning sun is golden");
}

#[test]
fn sun_on_wall_overhead_at_noon() {
    let noon = epoch_at_local(12, 0);
    let spot = sun_on_wall(noon).expect("sun at noon");
    assert!(matches!(spot.wall, WallSide::South));
    assert!(spot.intensity > 0.85, "noon sun is brightest");
}

#[test]
fn sun_on_wall_none_at_midnight() {
    let midnight = epoch_at_local(0, 0);
    assert!(sun_on_wall(midnight).is_none());
}

#[test]
fn sun_on_wall_west_at_evening() {
    let evening = epoch_at_local(18, 0);
    let spot = sun_on_wall(evening).expect("sun on wall in evening");
    assert!(matches!(spot.wall, WallSide::West));
    assert!(spot.warmth > 0.6, "evening sun is warm");
}
```

`epoch_at_local(h, m)` helper: `SystemTime::UNIX_EPOCH + Duration::from_secs(h * 3600 + m * 60)` — UTC, but UTC + local TZ doesn't matter for the test because the function we're implementing uses raw `secs_since_epoch % 86400`.

### Step 2: Run tests to confirm they fail

Run: `cargo test -p pixtuoid --lib time_of_day::tests::sun_on_wall --features pixtuoid-core/test-renderer`
Expected: FAIL — `sun_on_wall not defined`.

### Step 3: Implement `sun_on_wall` + `WallSide` + `SunSpot`

Add to `time_of_day.rs`:

```rust
#[derive(Debug, Clone, Copy)]
pub(in crate::tui::pixel_painter) enum WallSide {
    East,
    South,
    West,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::tui::pixel_painter) struct SunSpot {
    pub wall: WallSide,
    /// 0.0..=1.0 along the wall (left→right for South, top→bottom for East/West).
    pub along: f32,
    /// Vertical position on the wall, 0.0=high, 1.0=low.
    pub vertical: f32,
    /// 0.0=dim, 1.0=brightest at noon.
    pub intensity: f32,
    /// 0.0=cool (noon white), 1.0=very warm (sunrise/sunset gold).
    pub warmth: f32,
}

pub(in crate::tui::pixel_painter) fn sun_on_wall(now: SystemTime) -> Option<SunSpot> {
    let secs = now.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    let day_secs = (secs % 86400) as f32;
    let hour = day_secs / 3600.0;
    // 06:00–08:30 → East wall, 08:30–16:00 → South (overhead), 16:00–19:00 → West.
    // Outside 06:00–19:00 returns None.
    if !(6.0..=19.0).contains(&hour) {
        return None;
    }
    let (wall, along) = if hour < 8.5 {
        (WallSide::East, (hour - 6.0) / 2.5)
    } else if hour < 16.0 {
        (WallSide::South, (hour - 8.5) / 7.5)
    } else {
        (WallSide::West, (hour - 16.0) / 3.0)
    };
    let noon_distance = (hour - 12.0).abs() / 6.0; // 0 at noon, 1 at 06:00/18:00
    let intensity = (1.0 - noon_distance * 0.7).clamp(0.3, 1.0);
    let warmth = noon_distance.clamp(0.0, 1.0); // 0=neutral at noon, 1=very warm at edges
    let vertical = match wall {
        WallSide::South => 0.15,           // overhead band
        WallSide::East | WallSide::West => 0.55 + noon_distance * 0.2, // low when near horizon
    };
    Some(SunSpot { wall, along, vertical, intensity, warmth })
}
```

### Step 4: Run tests, confirm green

Run: `cargo test -p pixtuoid --lib time_of_day::tests::sun_on_wall --features pixtuoid-core/test-renderer`
Expected: PASS (4 tests).

### Step 5: Failing snapshot test for paint integration

Add to `ambient.rs` `mod tests`:

```rust
#[test]
fn sun_spot_painted_at_noon() {
    let mut buf = RgbBuffer::new(160, 90);
    let theme = crate::tui::theme::ALL_THEMES[0]; // normal
    let noon = SystemTime::UNIX_EPOCH + Duration::from_secs(12 * 3600);
    let layout = crate::tui::layout::Layout::compute_with_seed(160, 90, 0);
    paint_sun_spot(&mut buf, &theme, &layout, noon);
    let centre = buf.get(80, layout.south_wall_y);
    let cold = buf.get(0, 0);
    assert!(
        centre != cold,
        "noon sun should brighten the south wall vs untouched corner"
    );
}
```

(Helpers + Layout method may need shims; if `south_wall_y` doesn't exist as-is, use any wall row your layout exposes — e.g. `layout.window_strip.y`.)

### Step 6: Implement `paint_sun_spot` in `ambient.rs`

```rust
pub(super) fn paint_sun_spot(
    buf: &mut RgbBuffer,
    theme: &Theme,
    layout: &Layout,
    now: SystemTime,
) {
    let Some(spot) = sun_on_wall(now) else { return; };
    // Pick wall pixel-rect from layout + WallSide; blit a warm gradient
    // sized ~8×6 modulated by spot.intensity, tinted between
    // theme.lighting.sun_spill (warmth=1) and white (warmth=0).
    // (Exact geometry per layout shape — see ambient.rs for full code.)
}
```

Then invoke it from `paint_ambient`:

```rust
pub(super) fn paint_ambient(ctx: &mut PixelCtx<'_>) {
    paint_sun_spot(ctx.buf, ctx.theme, ctx.layout, ctx.now);
}
```

### Step 7: Run all tests + visual verification

```bash
cargo test --workspace --features pixtuoid-core/test-renderer
cargo build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/sun-noon.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/sun-noon.png --scale 3
```

Read the cropped PNG and self-critique: is there a visible warm spot on the south wall at noon? If not, adjust the warm-tint blend strength.

### Step 8: Commit

```bash
git add crates/pixtuoid/src/tui/pixel_painter/background/time_of_day.rs crates/pixtuoid/src/tui/pixel_painter/ambient.rs
git commit -m "feat(atmosphere): sun position on wall by wallclock"
```

---

## Task 2: Sunbeam dust motes (spec §1)

**Files:**
- Modify: `crates/pixtuoid/src/tui/pixel_painter/ambient.rs`
- Modify: `crates/pixtuoid/src/tui/pixel_painter/background/mod.rs` (expose window spill anchors via a new helper)

### Step 1: Failing test for `dust_mote_positions` determinism

In `ambient.rs` `mod tests`:

```rust
#[test]
fn dust_mote_positions_deterministic_per_seed() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(12 * 3600 + 5);
    let positions_a = dust_mote_positions(42, now, &SunbeamColumn { x: 100, top_y: 12, depth: 12 });
    let positions_b = dust_mote_positions(42, now, &SunbeamColumn { x: 100, top_y: 12, depth: 12 });
    assert_eq!(positions_a, positions_b, "same seed + time → same positions");
}

#[test]
fn dust_motes_drift_over_time() {
    let now1 = SystemTime::UNIX_EPOCH + Duration::from_secs(12 * 3600);
    let now2 = now1 + Duration::from_millis(500);
    let col = SunbeamColumn { x: 100, top_y: 12, depth: 12 };
    let a = dust_mote_positions(7, now1, &col);
    let b = dust_mote_positions(7, now2, &col);
    assert_ne!(a, b, "positions advance over time");
}
```

### Step 2: Confirm tests fail

Run: `cargo test -p pixtuoid --lib ambient::tests::dust_mote --features pixtuoid-core/test-renderer`
Expected: FAIL — `SunbeamColumn`, `dust_mote_positions` undefined.

### Step 3: Implement

```rust
pub(super) struct SunbeamColumn {
    pub x: u16,
    pub top_y: u16,
    pub depth: u16,
}

const MOTES_PER_COLUMN: usize = 3;

/// Deterministic per (floor_seed, particle_id, now_ms / 50). Returns up to
/// MOTES_PER_COLUMN positions inside the column, sine-drifting in x,
/// slow-falling in y, with a fade band at top/bottom.
pub(super) fn dust_mote_positions(
    floor_seed: u32,
    now: SystemTime,
    col: &SunbeamColumn,
) -> Vec<(u16, u16, f32)> {
    let t_ms = now.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO).as_millis() as u64;
    let mut out = Vec::with_capacity(MOTES_PER_COLUMN);
    for i in 0..MOTES_PER_COLUMN {
        let seed = floor_seed.wrapping_mul(0x9E3779B1).wrapping_add(i as u32);
        let phase = (seed % 6283) as f32 / 1000.0;  // 0..2π
        let speed_y = 0.6 + (seed >> 12 & 0x3) as f32 * 0.2; // px/sec
        let speed_x = 0.4 + (seed >> 14 & 0x3) as f32 * 0.15;
        let y_offset = ((t_ms as f32 / 1000.0) * speed_y + (seed >> 4 & 0xFF) as f32) % col.depth as f32;
        let y = col.top_y + y_offset as u16;
        let sx = (phase + (t_ms as f32 / 1000.0) * speed_x).sin();
        let x = (col.x as f32 + sx * 2.5).round() as u16;
        // Fade in/out band: alpha 0 at top 15%, full 15-85%, 0 at bottom 85-100%
        let norm = y_offset / col.depth as f32;
        let alpha = if norm < 0.15 { norm / 0.15 }
                    else if norm > 0.85 { (1.0 - norm) / 0.15 }
                    else { 1.0 };
        out.push((x, y, alpha));
    }
    out
}
```

### Step 4: Run unit tests, confirm green

Run: `cargo test -p pixtuoid --lib ambient::tests::dust_mote --features pixtuoid-core/test-renderer`
Expected: PASS (2 tests).

### Step 5: Add `paint_dust_motes` and integrate

```rust
pub(super) fn paint_dust_motes(
    buf: &mut RgbBuffer,
    theme: &Theme,
    layout: &Layout,
    floor_seed: u32,
    now: SystemTime,
) {
    if sun_on_wall(now).is_none() {
        return; // no sun → no motes
    }
    let warm = theme.lighting.sun_spill;
    for window_rect in layout.window_strip_rects() {  // expose this if not present
        let col = SunbeamColumn {
            x: window_rect.x + window_rect.width / 2,
            top_y: window_rect.y + window_rect.height,
            depth: 12,
        };
        for (x, y, alpha) in dust_mote_positions(floor_seed, now, &col) {
            if x >= buf.width || y >= buf.height { continue; }
            let cur = buf.get(x, y);
            buf.put(x, y, Rgb(
                blend(cur.0, warm.0, alpha * 0.7),
                blend(cur.1, warm.1, alpha * 0.7),
                blend(cur.2, warm.2, alpha * 0.7),
            ));
        }
    }
}
```

Add to `paint_ambient`:

```rust
paint_dust_motes(ctx.buf, ctx.theme, ctx.layout, ctx.floor_meta.floor_seed, ctx.now);
```

(`window_strip_rects` or equivalent: if `Layout` doesn't expose window rects, add a getter. If only one strip is needed, use `layout.windows`.)

### Step 6: Visual verification

```bash
cargo build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/dust.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/dust.png --scale 3
```

Read the cropped PNG — do you see 2-3 single-pixel motes drifting through each window spill column?

### Step 7: Commit

```bash
git add crates/pixtuoid/src/tui/pixel_painter/ambient.rs
git commit -m "feat(atmosphere): dust motes in window sunbeam columns"
```

---

## Task 3: Weather → indoor floor tint (spec §3)

**Files:**
- Modify: `crates/pixtuoid/src/tui/pixel_painter/background/mod.rs`

### Step 1: Failing test

In `background/mod.rs` `mod tests` (create if needed):

```rust
#[test]
fn weather_floor_tint_differs_by_variant() {
    let clear = weather_floor_tint(Weather::Clear);
    let rain = weather_floor_tint(Weather::Rain);
    let fog = weather_floor_tint(Weather::Fog);
    assert_ne!(clear, rain, "rain biases floor cooler");
    assert_ne!(clear, fog, "fog desaturates");
    // Rain is cooler than clear: blue channel ≥ red.
    assert!(rain.2 >= rain.0, "rain tint should be cool (blue ≥ red), got {:?}", rain);
}

#[test]
fn weather_floor_tint_clear_is_neutral() {
    let clear = weather_floor_tint(Weather::Clear);
    // Identity-ish: clear should be ~RGB(255,255,255) or close so blend is no-op.
    assert!(clear.0 > 200 && clear.1 > 200 && clear.2 > 200);
}
```

### Step 2: Confirm tests fail

Run: `cargo test -p pixtuoid --lib background::tests::weather_floor --features pixtuoid-core/test-renderer`
Expected: FAIL.

### Step 3: Implement

In `background/mod.rs`:

```rust
/// Multiplicative-ish bias applied to floor cells after the base palette,
/// driven by current outdoor weather. Clear ≈ neutral (~white); rain/fog
/// pull cooler/desaturated; storm dims; snow goes blue.
pub(super) fn weather_floor_tint(w: Weather) -> Rgb {
    match w {
        Weather::Clear    => Rgb(255, 252, 240), // very slight warm
        Weather::Rain     => Rgb(190, 200, 220), // cool gray
        Weather::Storm    => Rgb(140, 145, 165), // dim cool
        Weather::Snow     => Rgb(220, 230, 250), // cool blue
        Weather::Fog      => Rgb(200, 200, 205), // desaturated
        Weather::Overcast => Rgb(210, 210, 215), // flat gray
        Weather::Windy    => Rgb(248, 248, 245), // near-neutral
    }
}
```

### Step 4: Apply in `paint_floor_and_walls`

In the floor cell loop inside `paint_floor_and_walls`, after the base color is computed, blend with `weather_floor_tint(weather)` at strength `0.15`:

```rust
let tint = weather_floor_tint(weather);
let cell = Rgb(
    blend(cell.0, tint.0, 0.15),
    blend(cell.1, tint.1, 0.15),
    blend(cell.2, tint.2, 0.15),
);
```

(Exact insertion point: the existing floor-cell write in `paint_floor_and_walls`.)

### Step 5: Run tests, visual verify

```bash
cargo test --workspace --features pixtuoid-core/test-renderer
cargo build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/weather.png
```

Repeat across `--seed 1`, `--seed 2`, etc. — different seeds produce different weather variants. Confirm floor cells visibly shift cooler under Rain seeds.

### Step 6: Commit

```bash
git add crates/pixtuoid/src/tui/pixel_painter/background/mod.rs
git commit -m "feat(atmosphere): weather biases indoor floor tint"
```

---

## Task 4: Ceiling reflections on dark themes (spec §4)

**Files:**
- Modify: `crates/pixtuoid/src/tui/theme/mod.rs` (add `ThemeKind` + `kind` field)
- Modify: 6 theme files: `normal.rs`, `cyberpunk.rs`, `dracula.rs`, `tokyo_night.rs`, `catppuccin.rs`, `gruvbox.rs`
- Modify: `crates/pixtuoid/src/tui/pixel_painter/ambient.rs`

### Step 1: Failing test for `ThemeKind`

In `theme/mod.rs` `mod tests` (create if needed):

```rust
#[test]
fn dark_themes_marked_dark() {
    assert_eq!(cyberpunk::THEME.kind, ThemeKind::Dark);
    assert_eq!(dracula::THEME.kind, ThemeKind::Dark);
    assert_eq!(tokyo_night::THEME.kind, ThemeKind::Dark);
}

#[test]
fn light_themes_marked_light() {
    assert_eq!(normal::THEME.kind, ThemeKind::Light);
}
```

### Step 2: Confirm tests fail

Run: `cargo test -p pixtuoid --lib theme::tests --features pixtuoid-core/test-renderer`
Expected: FAIL.

### Step 3: Add `ThemeKind` enum + `kind` field

In `theme/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Light,
    Dark,
}

pub struct Theme {
    pub name: &'static str,
    pub kind: ThemeKind,   // NEW
    // ... existing fields
}
```

### Step 4: Populate `kind` in each theme file

For `normal.rs` and `catppuccin.rs` (light variants):
```rust
pub static THEME: Theme = Theme {
    name: "normal",
    kind: ThemeKind::Light,
    // ...
};
```

For `cyberpunk.rs`, `dracula.rs`, `tokyo_night.rs`, `gruvbox.rs` (dark):
```rust
kind: ThemeKind::Dark,
```

(Catppuccin Mocha is dark, but the bundled variant uses light surface tones; mark as `Light`. Gruvbox-dark is dark — mark `Dark`.)

### Step 5: Run tests, confirm green

Run: `cargo test -p pixtuoid --lib theme::tests --features pixtuoid-core/test-renderer`
Expected: PASS.

### Step 6: Failing test for ceiling halo

In `ambient.rs` `mod tests`:

```rust
#[test]
fn ceiling_halo_painted_on_dark_theme_for_active_agent() {
    let mut buf = RgbBuffer::new(160, 90);
    let theme = &cyberpunk::THEME; // dark
    let layout = Layout::compute_with_seed(160, 90, 0);
    let halos = vec![CeilingHalo { x: 50, y: layout.ceiling_y, color: Rgb(0, 200, 255), intensity: 0.8 }];
    paint_ceiling_halos(&mut buf, theme, &halos);
    let on_halo = buf.get(50, layout.ceiling_y);
    let off_halo = buf.get(50, 0);
    assert_ne!(on_halo, off_halo, "halo should brighten ceiling pixel");
}

#[test]
fn ceiling_halo_skipped_on_light_theme() {
    let mut buf = RgbBuffer::new(160, 90);
    let theme = &normal::THEME; // light
    let layout = Layout::compute_with_seed(160, 90, 0);
    let halos = vec![CeilingHalo { x: 50, y: layout.ceiling_y, color: Rgb(0, 200, 255), intensity: 0.8 }];
    paint_ceiling_halos(&mut buf, theme, &halos);
    let on_halo = buf.get(50, layout.ceiling_y);
    let untouched = buf.get(50, 0);
    assert_eq!(on_halo, untouched, "no halo on light themes");
}
```

### Step 7: Implement `CeilingHalo` + `paint_ceiling_halos`

```rust
#[derive(Debug, Clone, Copy)]
pub(super) struct CeilingHalo {
    pub x: u16,
    pub y: u16,
    pub color: Rgb,
    pub intensity: f32,
}

pub(super) fn paint_ceiling_halos(
    buf: &mut RgbBuffer,
    theme: &Theme,
    halos: &[CeilingHalo],
) {
    if theme.kind != ThemeKind::Dark {
        return;
    }
    for halo in halos {
        // 5-wide × 2-tall gradient centered on (halo.x, halo.y), strength = halo.intensity.
        for dy in 0..2u16 {
            for dx in 0..5u16 {
                let x = halo.x.saturating_sub(2).saturating_add(dx);
                let y = halo.y.saturating_sub(dy);
                if x >= buf.width || y >= buf.height { continue; }
                let dist = ((dx as i32 - 2).abs() as f32 + dy as f32) / 3.0;
                let strength = halo.intensity * (1.0 - dist).max(0.0) * 0.4;
                let cur = buf.get(x, y);
                buf.put(x, y, Rgb(
                    blend(cur.0, halo.color.0, strength),
                    blend(cur.1, halo.color.1, strength),
                    blend(cur.2, halo.color.2, strength),
                ));
            }
        }
    }
}
```

### Step 8: Wire halos into ambient pass

In `paint_ambient`, iterate `ctx.scene.agents` filtered to `ActivityState::Active`, compute halo `x = monitor_x`, `color = tool_glow_tint(detail)`, then call `paint_ceiling_halos`. Use the existing `ctx.layout.home_desks` for monitor positions.

### Step 9: Run tests + visual verify

```bash
cargo test --workspace --features pixtuoid-core/test-renderer
./target/release/examples/snapshot --theme cyberpunk --cols 192 --rows 80 /tmp/halo-cyber.png
./target/release/examples/snapshot --theme normal --cols 192 --rows 80 /tmp/halo-normal.png
```

Crop and compare — halo only on cyberpunk.

### Step 10: Commit

```bash
git add crates/pixtuoid/src/tui/theme/ crates/pixtuoid/src/tui/pixel_painter/ambient.rs
git commit -m "feat(atmosphere): ceiling halos above active monitors on dark themes"
```

---

## Task 5: Accumulated coffee stains (spec §5)

**Files:**
- Modify: `crates/pixtuoid/src/tui/tui_renderer.rs` (state + insertion + eviction)
- Modify: `crates/pixtuoid/src/tui/pixel_painter/drawable.rs` (paint stains)
- Modify: `crates/pixtuoid/src/tui/pixel_painter/mod.rs` (thread stain map through `PixelCtx`)

### Step 1: Failing test for `coffee_stains` insertion on pantry trip

In `crates/pixtuoid/tests/tui_renderer.rs` (or wherever the existing renderer tests live):

```rust
#[test]
fn coffee_stain_added_when_agent_returns_from_pantry() {
    let mut renderer = TuiRenderer::new();
    let agent_id = AgentId::from_transcript_path("/p/a.jsonl");
    // Simulate first pantry trip
    renderer.note_coffee_stain(agent_id, SystemTime::now());
    let stains = renderer.coffee_stains_for(agent_id);
    assert_eq!(stains.len(), 1);
}

#[test]
fn coffee_stains_capped_at_4_per_desk() {
    let mut renderer = TuiRenderer::new();
    let agent_id = AgentId::from_transcript_path("/p/a.jsonl");
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for i in 0..6 {
        renderer.note_coffee_stain(agent_id, t0 + Duration::from_secs(i));
    }
    assert_eq!(renderer.coffee_stains_for(agent_id).len(), 4, "oldest evicted past 4");
}

#[test]
fn coffee_stains_cleared_on_agent_evict() {
    let mut renderer = TuiRenderer::new();
    let agent_id = AgentId::from_transcript_path("/p/a.jsonl");
    renderer.note_coffee_stain(agent_id, SystemTime::now());
    renderer.evict_agent(agent_id);
    assert_eq!(renderer.coffee_stains_for(agent_id).len(), 0);
}
```

### Step 2: Confirm tests fail

Run: `cargo test -p pixtuoid --test tui_renderer coffee_stain --features pixtuoid-core/test-renderer`
Expected: FAIL.

### Step 3: Implement state on `TuiRenderer`

```rust
#[derive(Debug, Clone, Copy)]
pub struct StainPos {
    pub offset_x: i8,
    pub offset_y: i8,
    pub painted_at: SystemTime,
}

pub struct TuiRenderer {
    // ... existing fields
    coffee_stains: HashMap<AgentId, Vec<StainPos>>,
}

const MAX_STAINS_PER_DESK: usize = 4;

impl TuiRenderer {
    pub fn note_coffee_stain(&mut self, agent_id: AgentId, now: SystemTime) {
        let stains = self.coffee_stains.entry(agent_id).or_default();
        if stains.len() >= MAX_STAINS_PER_DESK {
            stains.remove(0); // FIFO: oldest first
        }
        // Deterministic offset per stain count
        let count = stains.len() as u32;
        let seed = agent_id.raw().wrapping_mul(0x9E3779B1).wrapping_add(count);
        let offset_x = ((seed & 0x7) as i8) - 3;       // -3..=4
        let offset_y = (((seed >> 3) & 0x3) as i8) - 1; // -1..=2
        stains.push(StainPos { offset_x, offset_y, painted_at: now });
    }

    pub fn coffee_stains_for(&self, id: AgentId) -> &[StainPos] {
        self.coffee_stains.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn evict_agent(&mut self, id: AgentId) {
        self.coffee_stains.remove(&id);
        // ... existing evict logic
    }
}
```

### Step 4: Hook insertion into the existing pantry-trip signal

The pixel painter already tracks `coffee_holders: HashSet<AgentId>` — see CLAUDE.md "How does the coffee run work?". Add: when the agent transitions out of `Pose::Walking { carrying_coffee: true }` (i.e. arrives at desk), call `renderer.note_coffee_stain(agent_id, now)`. Find the existing branch that handles `coffee_holders` and add the call alongside.

### Step 5: Run tests, confirm green

Run: `cargo test -p pixtuoid --test tui_renderer coffee_stain --features pixtuoid-core/test-renderer`
Expected: PASS.

### Step 6: Paint stains in `paint_desk_personalization`

In `drawable.rs::paint_desk_personalization`, after the existing coffee-cup paint, iterate the agent's `StainPos` list and paint a single `·`-style dot at the desk corner offset by `(offset_x, offset_y)`. Color = desaturated brown blended toward desk surface; alpha decays with `(now - painted_at).as_secs() / 1800` (full → fade over 30 min within session).

```rust
for stain in renderer.coffee_stains_for(agent_id) {
    let age_secs = now.duration_since(stain.painted_at).unwrap_or(Duration::ZERO).as_secs() as f32;
    let alpha = (1.0 - age_secs / 1800.0).max(0.2).min(1.0); // never below 0.2 within session
    let x = desk_anchor_x as i32 + stain.offset_x as i32;
    let y = desk_anchor_y as i32 + stain.offset_y as i32;
    if x < 0 || y < 0 { continue; }
    let cur = buf.get(x as u16, y as u16);
    let stain_color = Rgb(98, 60, 38); // brown
    buf.put(x as u16, y as u16, Rgb(
        blend(cur.0, stain_color.0, alpha * 0.5),
        blend(cur.1, stain_color.1, alpha * 0.5),
        blend(cur.2, stain_color.2, alpha * 0.5),
    ));
}
```

### Step 7: Visual verify

```bash
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/stains.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/stains.png --scale 3
```

Hard to test in a single snapshot since stains accumulate over time; run `pixtuoid` live, force a few pantry trips (or stub via test-only API), and confirm rings appear on the desk.

### Step 8: Commit

```bash
git add crates/pixtuoid/src/tui/tui_renderer.rs crates/pixtuoid/src/tui/pixel_painter/drawable.rs crates/pixtuoid/src/tui/pixel_painter/mod.rs
git commit -m "feat(atmosphere): accumulated coffee stains per desk"
```

---

## Task 6: Final preflight + docs update

- [ ] **Step 1: Update CLAUDE.md "Where to look" section**

Add a bullet under the existing list:

> - "How do atmosphere effects work?" → `tui/pixel_painter/ambient.rs` orchestrates 4 ambient effects between background and drawables: sun spot on wall (`paint_sun_spot` from `background::time_of_day::sun_on_wall`), dust motes (`paint_dust_motes` anchored to window light spill), ceiling halos (`paint_ceiling_halos`, dark themes only via `Theme::kind == ThemeKind::Dark`). Coffee stains accumulate on `TuiRenderer::coffee_stains` and paint in `drawable::paint_desk_personalization`; max 4 per desk, FIFO eviction. Weather floor tint is in `background::weather_floor_tint`, applied in `paint_floor_and_walls`.

- [ ] **Step 2: Run full preflight**

```bash
scripts/preflight.sh
```

Fix any clippy/fmt/test breakage in-place.

- [ ] **Step 3: Final commit if docs changed**

```bash
git add CLAUDE.md
git commit -m "docs: atmosphere pass — ambient module + per-effect pointers"
```

- [ ] **Step 4: Push + open PR (only on user confirmation)**

```bash
git push -u origin feat/pr53-atmosphere
gh pr create --title "feat(atmosphere): 5 ambient effects across the office" --body "$(cat docs/superpowers/plans/2026-05-28-atmosphere.md)"
```

---

## Self-Review Checklist

- [x] **Spec coverage** — All 5 features have a task.
- [x] **Placeholder scan** — No "TBD", "TODO" markers in plan body.
- [x] **Type consistency** — `SunSpot`, `WallSide`, `SunbeamColumn`, `CeilingHalo`, `StainPos`, `ThemeKind` named consistently across tasks. `paint_*` functions follow the existing snake_case convention.
- [x] **Spec deviations called out** — `paint_sunbeam` → `paint_window_light_spill`; `theme.kind` field added since it didn't exist.

---

## Execution Handoff

Two ways to run this plan:

1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks, fast iteration in the same session.
2. **Inline Execution** — execute tasks here directly using `superpowers:executing-plans`, batch checkpoints for review.

Pick one.
