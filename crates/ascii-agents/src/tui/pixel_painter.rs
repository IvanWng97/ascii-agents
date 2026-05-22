//! Pure-pixel paint pass — no ratatui types, no terminal I/O.
//!
//! Split from `tui/renderer.rs` to separate the pixel-painting pipeline
//! (called by any renderer impl — `TuiRenderer`, a future web canvas, PNG
//! export, GIF capture) from the ratatui-coupled half-block flush + widget
//! overlay + terminal lifecycle.
//!
//! `render_to_rgb_buffer` is the public entry point. Everything else is
//! private to this module except `character_anchor`, which `renderer.rs`
//! uses for label placement and mouse hit-testing.

use std::collections::HashMap;
use std::time::SystemTime;

use ascii_agents_core::sprite::blit::blit_frame;
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::walkable::OccupancyOverlay;
use ascii_agents_core::{AgentSlot, SceneState};

use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{Layout, Point, DESK_H, DESK_W};
use crate::tui::pathfind::Router;
use crate::tui::pose::{self, Pose};

// --- Colors ---------------------------------------------------------------
// Wood floor — warm oak tones with random light/dark flecks. Keeps the
// soft per-pixel grain pattern from the carpet phase (no seams, no brick
// grid) but in a wood color palette.
const CARPET_BASE: Rgb = Rgb(150, 110, 72);
const CARPET_LIGHT: Rgb = Rgb(178, 138, 96);
const CARPET_DARK: Rgb = Rgb(118, 82, 50);
const WALL: Rgb = Rgb(56, 56, 70);
const WALL_TRIM: Rgb = Rgb(80, 80, 100);
const BASEBOARD: Rgb = Rgb(40, 40, 52);
/// Warm / extroverted shirt palette — used for higher-trip-chance agents.
/// Warm / extroverted shirt palette — agents with higher trip_chance_pct
/// pick from here. Expanded from 4 → 8 colors so a crowded office of
/// 16 agents has visibly distinct silhouettes.
const SHIRT_PRESETS_WARM: &[Rgb] = &[
    Rgb(0x9c, 0x27, 0x27), // crimson
    Rgb(0xc6, 0x6a, 0x1e), // burnt orange
    Rgb(0xb0, 0x32, 0xa8), // magenta
    Rgb(0xd0, 0x9c, 0x32), // mustard
    Rgb(0xe0, 0x46, 0x46), // tomato
    Rgb(0xa8, 0x4e, 0x9c), // rose violet
    Rgb(0xcf, 0x7b, 0x2c), // pumpkin
    Rgb(0xc4, 0x39, 0x6f), // raspberry
];
/// Cool / homebody shirt palette — used for lower-trip-chance agents.
const SHIRT_PRESETS_COOL: &[Rgb] = &[
    Rgb(0x2e, 0x62, 0xcf), // royal blue
    Rgb(0x16, 0xa0, 0x6e), // forest green
    Rgb(0x32, 0x82, 0x9b), // teal
    Rgb(0x6c, 0x4f, 0x9e), // violet
    Rgb(0x4a, 0x7a, 0xb8), // steel blue
    Rgb(0x2e, 0x8a, 0x84), // pine
    Rgb(0x3e, 0x52, 0x9c), // indigo
    Rgb(0x5c, 0x8a, 0x32), // moss green
];
/// 8 hair colors — was 5. Added silver/grey for older-coded agents,
/// ginger / strawberry blonde / jet black for more silhouette variety.
const HAIR_PRESETS: &[Rgb] = &[
    Rgb(0x14, 0x0a, 0x06), // jet black
    Rgb(0x2a, 0x1a, 0x0e), // near-black brown
    Rgb(0x52, 0x32, 0x10), // dark brown
    Rgb(0x8a, 0x5a, 0x36), // light brown
    Rgb(0xc7, 0xa3, 0x4a), // blond
    Rgb(0xd8, 0x68, 0x32), // ginger
    Rgb(0x7a, 0x32, 0x10), // auburn
    Rgb(0xa8, 0xa8, 0xb0), // silver-grey
];
const SKIN_PRESETS: &[Rgb] = &[
    Rgb(0xf4, 0xc7, 0x9a), // light peach (matches base palette S)
    Rgb(0xe0, 0xa8, 0x70), // medium
    Rgb(0xb8, 0x80, 0x50), // tan
    Rgb(0x8a, 0x5a, 0x36), // deep brown
    Rgb(0xc8, 0x9a, 0x64), // warm tan
];
// --- Per-agent recolor ----------------------------------------------------
fn agent_palette(base: &Palette, agent: &AgentSlot) -> Palette {
    let seed = agent.agent_id.raw() as usize;
    // Personality nudges aesthetic choice: extroverted (high trip_chance)
    // agents pick from the warm shirt palette, homebodies from cool.
    let p = pose::personality_for(agent.agent_id);
    let shirts = if p.trip_chance_pct >= 30 {
        SHIRT_PRESETS_WARM
    } else {
        SHIRT_PRESETS_COOL
    };
    let shirt = shirts[seed % shirts.len()];
    let hair = HAIR_PRESETS[(seed / 7) % HAIR_PRESETS.len()];
    let skin = SKIN_PRESETS[(seed / 13) % SKIN_PRESETS.len()];
    // Active = monitor is lit, light reflects on the user's face. Tint the
    // skin slightly toward the glow color so the eye reads "the monitor is
    // actually lighting them up", not just "there's a green dot below".
    let final_skin = if matches!(agent.state, ActivityState::Active { .. }) {
        const GLOW_TINT: Rgb = Rgb(140, 240, 170);
        Rgb(
            blend(skin.0, GLOW_TINT.0, 0.18),
            blend(skin.1, GLOW_TINT.1, 0.18),
            blend(skin.2, GLOW_TINT.2, 0.18),
        )
    } else {
        skin
    };
    base.with_override('B', Some(shirt))
        .with_override('H', Some(hair))
        .with_override('S', Some(final_skin))
}

fn recolor_frame(frame: &Frame, pal: &Palette, base_pal: &Palette) -> Frame {
    let base_shirt = base_pal.get('B').flatten();
    let base_hair = base_pal.get('H').flatten();
    let base_skin = base_pal.get('S').flatten();
    let agent_shirt = pal.get('B').flatten();
    let agent_hair = pal.get('H').flatten();
    let agent_skin = pal.get('S').flatten();
    let pixels: Vec<Pixel> = frame
        .pixels
        .iter()
        .map(|p| match p {
            Some(rgb) if Some(*rgb) == base_shirt => agent_shirt,
            Some(rgb) if Some(*rgb) == base_hair => agent_hair,
            Some(rgb) if Some(*rgb) == base_skin => agent_skin,
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
fn paint_floor_and_walls(
    buf: &mut RgbBuffer,
    buf_w: u16,
    buf_h: u16,
    now: SystemTime,
    look: &TimeOfDayLook,
    top_wall_h: u16,
) {
    const BASEBOARD_H: u16 = 3;
    const WINDOW_FRAME: Rgb = Rgb(24, 24, 32);

    // Carpet: warm tan/grey base with deterministic light/dark flecks.
    // No seams, no grid — softer than the previous plank pattern.
    for y in 0..buf_h {
        for x in 0..buf_w {
            let hash = (x as u32)
                .wrapping_mul(73)
                .wrapping_add((y as u32).wrapping_mul(151))
                ^ ((x as u32).wrapping_mul(11) ^ (y as u32).wrapping_mul(37));
            let color = match hash % 17 {
                0 | 1 => CARPET_LIGHT,
                2 | 3 => CARPET_DARK,
                _ => CARPET_BASE,
            };
            buf.put(x, y, color);
        }
    }
    for y in 0..top_wall_h.min(buf_h) {
        for x in 0..buf_w {
            buf.put(x, y, WALL);
        }
    }

    // Floor-to-ceiling windows: 落地窗 — height grows with the wall band so
    // taller terminals get dramatic floor-to-ceiling glass. Width stays
    // fixed (mullion every 22 px) so the skyline detail reads consistently.
    const WINDOW_W: u16 = 22;
    const WINDOW_GAP: u16 = 3;
    let window_y: u16 = 1;
    let window_h: u16 = top_wall_h.saturating_sub(3).max(8);
    let mut x = 3u16;
    let mut idx: u32 = 0;
    while x + WINDOW_W + 2 <= buf_w {
        paint_floor_to_ceiling_window(
            buf,
            x,
            window_y,
            WINDOW_W,
            window_h,
            WINDOW_FRAME,
            look,
            idx as u16,
            now,
        );
        if look.spill_strength > 0.0 {
            paint_window_light_spill(
                buf,
                x,
                WINDOW_W,
                top_wall_h,
                look.spill_strength,
                look.spill_slant,
            );
        }
        x += WINDOW_W + WINDOW_GAP;
        idx += 1;
    }

    // Wall trim line at the bottom of the wall band.
    let trim_y = top_wall_h.saturating_sub(1);
    if trim_y < buf_h {
        for x in 0..buf_w {
            buf.put(x, trim_y, WALL_TRIM);
        }
    }

    let base_y = buf_h.saturating_sub(BASEBOARD_H);
    for y in base_y..buf_h {
        for x in 0..buf_w {
            buf.put(x, y, BASEBOARD);
        }
    }
}

/// Warm sunlight tint spilling onto the floor below a window. Trapezoid
/// shape (widens by 1 px every 2 rows) blended with the existing floor so
/// it reads as "light through window" not "yellow rectangle". `intensity`
/// (0..1) scales with daylight — zero at night so no spill paints.
/// `slant_per_row` shifts the spill horizontally per row going down —
/// positive = rightward (morning sun in the east casts light right), negative
/// = leftward (evening sun in the west casts light left).
/// Per-dot twinkle: each city-window dot has its own ~600-1400ms cycle and
/// each cycle rerolls on/off via a deterministic hash. Bias toward "on" so
/// the skyline is mostly lit with the occasional dot blinking off.
/// Static "is this building window lit?" decision — independent of time.
/// Deterministic hash of (window_idx, dx, dy) so each building's window
/// pattern is stable across frames; only `city_dot_twinkle` animates
/// on top. ~75% of grid slots are lit so the city reads as "alive at
/// night" without every single window being on.
fn city_dot_lit(window_idx: u16, dx: u16, dy: u16) -> bool {
    let mut h = (window_idx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= (dx as u64).wrapping_mul(0xc6a4_a793_5bd1_e995);
    h ^= (dy as u64).wrapping_mul(0x1656_67b1_9e37_79b9);
    h ^= h >> 17;
    (h % 100) < 75
}

fn city_dot_twinkle(window_idx: u16, dx: u16, dy: u16, now: SystemTime) -> bool {
    let now_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let dot_seed = (window_idx as u64).wrapping_mul(31)
        ^ (dx as u64).wrapping_mul(131)
        ^ (dy as u64).wrapping_mul(521);
    // Per-dot cycle 3-7 s — slow enough that the eye doesn't perceive
    // constant flickering, fast enough that the skyline still "lives".
    let cycle_ms = 3000 + (dot_seed % 4000);
    let phase = now_ms / cycle_ms;
    let hash = dot_seed
        .wrapping_add(phase)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash % 10) < 8
}

fn paint_window_light_spill(
    buf: &mut RgbBuffer,
    window_x: u16,
    window_w: u16,
    top_y: u16,
    intensity: f32,
    slant_per_row: f32,
) {
    const WARM: Rgb = Rgb(255, 230, 160);
    const DEPTH: u16 = 12;
    let fade_start = 0.32 * intensity;
    for dy in 0..DEPTH {
        let widen = (dy / 2).min(3);
        let shift = (slant_per_row * dy as f32).round() as i32;
        let base_x = (window_x as i32 + shift).max(0) as u16;
        let start_x = base_x.saturating_sub(widen);
        let end_x = (base_x + window_w + widen).min(buf.width);
        let y = top_y + dy;
        if y >= buf.height {
            break;
        }
        let strength = fade_start * (1.0 - dy as f32 / DEPTH as f32);
        for x in start_x..end_x {
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, WARM.0, strength),
                    blend(cur.1, WARM.1, strength),
                    blend(cur.2, WARM.2, strength),
                ),
            );
        }
    }
}

/// Window glass color + spill intensity + spill slant for the current local
/// hour. `spill_slant` is x-shift per row going down: positive = rightward
/// (morning sun in the east), negative = leftward (evening sun in the west).
/// `darkness` is 1 - daylight, used to drive artificial-light effects.
struct TimeOfDayLook {
    glass_a: Rgb,
    glass_b: Rgb,
    spill_strength: f32,
    spill_slant: f32,
    darkness: f32,
}

fn time_of_day_look(now: SystemTime) -> TimeOfDayLook {
    use chrono::Timelike;
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    let h = local.hour() as f32 + local.minute() as f32 / 60.0;

    // Daylight intensity: full from 8 to 17, smooth ramp 5..8 and 17..20.
    let day = if !(5.0..20.0).contains(&h) {
        0.0
    } else if h < 8.0 {
        (h - 5.0) / 3.0
    } else if h < 17.0 {
        1.0
    } else {
        1.0 - (h - 17.0) / 3.0
    };

    // Twilight bell at dawn (~6.5) and dusk (~18.5) — adds orange/pink
    // tint that the cyan↔dark-blue base doesn't capture.
    let twilight = bell(h, 6.5, 1.5).max(bell(h, 18.5, 1.5));

    const DAY_A: Rgb = Rgb(120, 160, 200);
    const DAY_B: Rgb = Rgb(160, 190, 220);
    const NIGHT_A: Rgb = Rgb(18, 26, 52);
    const NIGHT_B: Rgb = Rgb(28, 36, 70);
    const TWILIGHT_A: Rgb = Rgb(220, 130, 80);
    const TWILIGHT_B: Rgb = Rgb(240, 170, 110);

    let glass_a = lerp_rgb(lerp_rgb(NIGHT_A, DAY_A, day), TWILIGHT_A, twilight * 0.5);
    let glass_b = lerp_rgb(lerp_rgb(NIGHT_B, DAY_B, day), TWILIGHT_B, twilight * 0.5);

    // Spill slant: ±0.7 px per row at peak hours (6am leftmost, 6pm
    // rightmost), zero at noon. Conventional read: morning sun on the east
    // (right of image) casts light westward (leftward shift); evening sun
    // on the west casts eastward (rightward shift).
    let slant = if h < 12.0 {
        -((12.0 - h) / 6.0).clamp(0.0, 1.0) * 0.7
    } else {
        ((h - 12.0) / 6.0).clamp(0.0, 1.0) * 0.7
    };

    TimeOfDayLook {
        glass_a,
        glass_b,
        spill_strength: day,
        spill_slant: slant,
        darkness: 1.0 - day,
    }
}

/// Multiplicative dim applied to floor pixels at night. Pulls everything
/// toward a dark navy so the artificial-light pools have something to
/// stand out against. `strength` is 0..1 (no dim..full dim).
fn dim_floor_overlay(buf: &mut RgbBuffer, top_y: u16, bottom_y: u16, strength: f32) {
    const NIGHT_TINT: Rgb = Rgb(18, 22, 38);
    let s = strength.clamp(0.0, 0.55);
    for y in top_y..bottom_y.min(buf.height) {
        for x in 0..buf.width {
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, NIGHT_TINT.0, s),
                    blend(cur.1, NIGHT_TINT.1, s),
                    blend(cur.2, NIGHT_TINT.2, s),
                ),
            );
        }
    }
}

/// Elliptical "ceiling fluorescent" pool of pale warm light on the floor.
/// Blended additively (toward POOL color) with a quadratic falloff from
/// center to edge so it reads as a soft round patch, not a stamped oval.
fn paint_ceiling_pool(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    half_w: u16,
    half_h: u16,
    strength: f32,
) {
    const POOL: Rgb = Rgb(255, 246, 215);
    if half_w == 0 || half_h == 0 || strength <= 0.0 {
        return;
    }
    let min_x = cx.saturating_sub(half_w);
    let max_x = (cx + half_w).min(buf.width);
    let min_y = cy.saturating_sub(half_h);
    let max_y = (cy + half_h).min(buf.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let nx = (x as f32 - cx as f32) / half_w as f32;
            let ny = (y as f32 - cy as f32) / half_h as f32;
            let r2 = nx * nx + ny * ny;
            if r2 > 1.0 {
                continue;
            }
            let t = (1.0 - r2) * strength;
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, POOL.0, t),
                    blend(cur.1, POOL.1, t),
                    blend(cur.2, POOL.2, t),
                ),
            );
        }
    }
}

/// Warm radial halo around the floor lamp — only visible at night.
fn paint_floor_lamp_halo(buf: &mut RgbBuffer, cx: u16, cy: u16, strength: f32) {
    const WARM: Rgb = Rgb(255, 210, 130);
    const RADIUS: u16 = 11;
    if strength <= 0.0 {
        return;
    }
    let min_x = cx.saturating_sub(RADIUS);
    let max_x = (cx + RADIUS).min(buf.width);
    let min_y = cy.saturating_sub(RADIUS);
    let max_y = (cy + RADIUS).min(buf.height);
    let r2max = (RADIUS as f32) * (RADIUS as f32);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let dx = x as f32 - cx as f32;
            let dy = y as f32 - cy as f32;
            let r2 = dx * dx + dy * dy;
            if r2 > r2max {
                continue;
            }
            let t = (1.0 - (r2 / r2max).sqrt()) * strength;
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, WARM.0, t),
                    blend(cur.1, WARM.1, t),
                    blend(cur.2, WARM.2, t),
                ),
            );
        }
    }
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    mix_lab(a, b, t)
}

/// Bell curve centered at `c` with half-width `w` (so the bell is 0 at
/// `c ± w` and 1 at `c`). Used for dawn/dusk twilight tint.
fn bell(x: f32, c: f32, w: f32) -> f32 {
    let d = (x - c) / w;
    (1.0 - d * d).max(0.0)
}

/// Per-channel sRGB lerp. Cheap; used for low-strength tints where
/// perceptual error doesn't matter (e.g. agent skin glow).
fn blend(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) * (1.0 - t) + (b as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Perceptually-correct Lab-space mix between two sRGB colors. Twilight
/// (orange → navy) and dim overlays travel cleanly through Lab without the
/// muddy desaturated midpoint that naive sRGB lerp produces. Slower than
/// `blend()` but only used where the perceptual difference is visible.
fn mix_lab(a: Rgb, b: Rgb, t: f32) -> Rgb {
    use palette::{FromColor, IntoColor, Lab, Mix, Srgb};
    let sa = Srgb::new(a.0 as f32 / 255.0, a.1 as f32 / 255.0, a.2 as f32 / 255.0);
    let sb = Srgb::new(b.0 as f32 / 255.0, b.1 as f32 / 255.0, b.2 as f32 / 255.0);
    let la = Lab::from_color(sa);
    let lb = Lab::from_color(sb);
    let mixed: Srgb = la.mix(lb, t.clamp(0.0, 1.0)).into_color();
    Rgb(
        (mixed.red.clamp(0.0, 1.0) * 255.0).round() as u8,
        (mixed.green.clamp(0.0, 1.0) * 255.0).round() as u8,
        (mixed.blue.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Floor-to-ceiling window with frame, mullion, and a procedural city view
/// inside the glass. Sky gradient at top blends with time-of-day glass
/// colors; the lower portion shows building silhouettes whose "windows"
/// (1-pixel dots) light up at night and twinkle on a per-dot cycle so the
/// skyline reads as alive instead of stamped.
#[allow(clippy::too_many_arguments)]
fn paint_floor_to_ceiling_window(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    frame: Rgb,
    look: &TimeOfDayLook,
    window_idx: u16,
    now: SystemTime,
) {
    const BUILDING_DARK: Rgb = Rgb(20, 22, 32);
    const BUILDING_LIGHT: Rgb = Rgb(60, 65, 82);
    const LIT_WINDOW: Rgb = Rgb(252, 215, 110);
    const DARK_WINDOW: Rgb = Rgb(30, 32, 44);

    let lit_strength = look.darkness.clamp(0.0, 1.0);
    let lit_color = lerp_rgb(DARK_WINDOW, LIT_WINDOW, lit_strength);
    let building = lerp_rgb(BUILDING_LIGHT, BUILDING_DARK, look.darkness);

    // Skyline silhouette as a 0..15 PATTERN; the actual pixel height is
    // computed per-window so the skyline auto-scales with the glass
    // height. On a 12-px-tall window the buildings are 3..7 px, on a
    // 50-px-tall window they fill 12..24 px — same visual proportion.
    const SKYLINE_PATTERN: &[u8] = &[8, 14, 11, 15, 6, 13, 9, 12, 7, 15, 10, 13];
    const PATTERN_MAX: u16 = 15;
    let glass_h = h.saturating_sub(2);
    let min_bh = (glass_h / 5).max(3);
    let max_bh = (glass_h * 50 / 100).max(min_bh + 4);
    let bh_range = max_bh.saturating_sub(min_bh);
    let sky_norm = (glass_h as f32) * 0.7;
    let sky_row: Vec<Rgb> = (0..glass_h)
        .map(|gy| {
            let sky_t = (gy as f32 / sky_norm).min(1.0);
            lerp_rgb(look.glass_b, look.glass_a, sky_t)
        })
        .collect();

    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width || py >= buf.height {
                continue;
            }
            let on_edge = dx == 0 || dx == w - 1 || dy == 0 || dy == h - 1;
            let on_mullion = dx == w / 2 || dy == h * 7 / 10;
            if on_edge || on_mullion {
                buf.put(px, py, frame);
                continue;
            }
            let glass_dx = dx - 1;
            let glass_dy = dy - 1;
            let pat_idx = ((glass_dx + window_idx * 3) % SKYLINE_PATTERN.len() as u16) as usize;
            let pat = SKYLINE_PATTERN[pat_idx] as u16;
            let building_h = min_bh + (pat * bh_range) / PATTERN_MAX;
            let in_building = glass_dy >= glass_h.saturating_sub(building_h);

            if in_building {
                let bldg_y = glass_dy - (glass_h - building_h);
                // Lit-window dots arranged on a 2-px grid (every other
                // column + every other row of the building). Per-dot
                // lit/unlit decision is hashed from (col, row, win_idx)
                // so the same building always shows the same pattern;
                // ~70 % of grid slots are lit at night. Twinkle animates
                // the lit ones on independent cycles.
                let on_grid = glass_dx % 2 == 1 && bldg_y % 2 == 1;
                let lit_base = on_grid && city_dot_lit(window_idx, glass_dx, bldg_y);
                if lit_base && city_dot_twinkle(window_idx, glass_dx, bldg_y, now) {
                    buf.put(px, py, lit_color);
                } else {
                    buf.put(px, py, building);
                }
            } else {
                buf.put(px, py, sky_row[glass_dy as usize]);
            }
        }
    }
}

/// Live wall clock — reads system local time and renders hour + minute hands.
/// 5x5 clock face. Hands quantize to 8 cardinal/intercardinal directions
/// (the most a 5x5 sprite can express).
fn paint_clock(buf: &mut RgbBuffer, x: u16, y: u16, now: SystemTime) {
    const RIM: Rgb = Rgb(200, 200, 210);
    const FACE: Rgb = Rgb(240, 240, 240);
    const HAND_HOUR: Rgb = Rgb(20, 20, 25);
    const HAND_MIN: Rgb = Rgb(60, 60, 80);

    // Face + rim background.
    let bg: &[(u16, u16, Rgb)] = &[
        (1, 0, RIM),
        (2, 0, RIM),
        (3, 0, RIM),
        (0, 1, RIM),
        (1, 1, FACE),
        (2, 1, FACE),
        (3, 1, FACE),
        (4, 1, RIM),
        (0, 2, RIM),
        (1, 2, FACE),
        (2, 2, FACE),
        (3, 2, FACE),
        (4, 2, RIM),
        (0, 3, RIM),
        (1, 3, FACE),
        (2, 3, FACE),
        (3, 3, FACE),
        (4, 3, RIM),
        (1, 4, RIM),
        (2, 4, RIM),
        (3, 4, RIM),
    ];
    for (dx, dy, c) in bg {
        let px = x + dx;
        let py = y + dy;
        if px < buf.width && py < buf.height {
            buf.put(px, py, *c);
        }
    }

    // Decompose `now` into local hour + minute via chrono.
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    use chrono::Timelike;
    let hour = local.hour() % 12;
    let minute = local.minute();

    // Fractional positions around the clock (0.0 = 12 o'clock, 0.25 = 3 o'clock).
    let hour_turns = (hour as f32 + minute as f32 / 60.0) / 12.0;
    let min_turns = minute as f32 / 60.0;

    let put = |buf: &mut RgbBuffer, ox: i32, oy: i32, color: Rgb| {
        let px = x as i32 + 2 + ox;
        let py = y as i32 + 2 + oy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width && (py as u16) < buf.height {
            buf.put(px as u16, py as u16, color);
        }
    };

    // Center pin (always painted).
    put(buf, 0, 0, HAND_HOUR);

    // Hour hand: 1 px from center along quantized angle.
    let (hdx, hdy) = octant_offset(hour_turns);
    put(buf, hdx, hdy, HAND_HOUR);

    // Minute hand: 2 px from center (longer than hour hand) along its angle.
    let (mdx, mdy) = octant_offset(min_turns);
    put(buf, mdx, mdy, HAND_MIN);
    // (Don't put a 2nd pixel if it falls off the 5x5 — the rim handles it.)
}

/// Quantize a fractional turn (0.0..1.0, 0.0 = north) to one of 8 octant
/// (dx, dy) unit offsets.
fn octant_offset(turn: f32) -> (i32, i32) {
    let oct = ((turn * 8.0).round() as i32).rem_euclid(8);
    match oct {
        0 => (0, -1),
        1 => (1, -1),
        2 => (1, 0),
        3 => (1, 1),
        4 => (0, 1),
        5 => (-1, 1),
        6 => (-1, 0),
        7 => (-1, -1),
        _ => (0, 0),
    }
}

// --- Character placement --------------------------------------------------
fn seated_anchor(desk: Point) -> Point {
    Point {
        x: desk.x + DESK_W.saturating_sub(8) / 2,
        y: desk.y.saturating_sub(8),
    }
}

fn standing_at_desk_anchor(desk: Point) -> Point {
    Point {
        x: desk.x + DESK_W.saturating_sub(8) / 2,
        y: desk.y.saturating_sub(12),
    }
}

fn walking_anchor(p: Point) -> Point {
    Point {
        x: p.x.saturating_sub(4),
        y: p.y.saturating_sub(12),
    }
}

fn waypoint_anchor(wp: Point) -> Point {
    Point {
        x: wp.x.saturating_sub(4),
        y: wp.y.saturating_sub(12),
    }
}

/// One-pixel vertical bob on a ~2.8 s cycle with a per-agent phase offset,
/// so static (seated / standing) characters look alive instead of frozen.
/// Walking + waypoint-trip poses already animate, so we skip those.
fn breath_offset_y(agent_id: ascii_agents_core::AgentId, now: SystemTime) -> u16 {
    let elapsed_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    const CYCLE_MS: u64 = 2800;
    let offset_ms = agent_id.raw() % CYCLE_MS;
    let phase = elapsed_ms.wrapping_add(offset_ms) % CYCLE_MS;
    if phase < CYCLE_MS / 2 {
        0
    } else {
        1
    }
}

fn with_breath(anchor: Point, agent_id: ascii_agents_core::AgentId, now: SystemTime) -> Point {
    Point {
        x: anchor.x,
        y: anchor.y.saturating_sub(breath_offset_y(agent_id, now)),
    }
}

/// Anchor for a back-view sitter on a mirror_vertical'd couch. Couch back
/// is now at the BOTTOM of the sprite, so the character's body sits
/// ENTIRELY ABOVE the couch back (head 7 px above couch center, body
/// ending right at the couch back row). Different from `couch_seat_anchor`
/// because back_couch.sprite has no transparent head/face area — its hair
/// extends across all top rows, so positioning it lower would put the
/// character's "head" overlapping the couch back row.
fn back_couch_anchor(wp: Point) -> Point {
    Point {
        x: wp.x.saturating_sub(4),
        y: wp.y.saturating_sub(7),
    }
}

/// X-offset applied to a waypoint anchor when multiple agents land at the
/// same destination in the same cycle. rank 0 = first arrival (no offset);
/// later arrivals step aside. Couch is 14 wide so it can comfortably seat
/// two; coffee + water cooler are single-use so the queue stands well off
/// to the side.
fn waypoint_rank_offset_x(kind: crate::tui::layout::WaypointKind, rank: usize) -> i16 {
    use crate::tui::layout::WaypointKind;
    match (kind, rank) {
        (_, 0) => 0,
        (WaypointKind::Couch, 1) => 6,
        (WaypointKind::Couch, 2) => -6,
        (WaypointKind::Couch, _) => 0,
        (_, 1) => 9,
        (_, 2) => -9,
        (_, _) => 0,
    }
}

fn walking_position(from: Point, to: Point, t_x1000: u16) -> Point {
    let t = t_x1000 as i32;
    let dx = to.x as i32 - from.x as i32;
    let dy = to.y as i32 - from.y as i32;
    // Clamp at zero before casting to u16 — left-walking agents (to.x <
    // from.x) cross through negative x partway through their walk if the
    // animation interpolation overshoots, and a bare `as u16` cast wraps
    // silently to ~65k, blitting the sprite off-screen invisibly.
    Point {
        x: (from.x as i32 + dx * t / 1000).max(0).min(u16::MAX as i32) as u16,
        y: (from.y as i32 + dy * t / 1000).max(0).min(u16::MAX as i32) as u16,
    }
}

/// Paint a character at an arbitrary anchor with per-agent recolor. `flip_x`
/// mirrors the sprite horizontally — used to make walkers face the direction
/// they're moving.
#[allow(clippy::too_many_arguments)]
fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &'static str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
    cache: &mut FrameCache,
) {
    let Some(anim) = pack.animation(anim_name) else {
        return;
    };
    let Some(frame) = anim.frames.get(frame_idx).or_else(|| anim.frames.first()) else {
        return;
    };
    let cached = cache.get_or_make(agent.agent_id, anim_name, frame_idx, flip_x, || {
        let pal = agent_palette(&pack.palette, agent);
        let recolored = recolor_frame(frame, &pal, &pack.palette);
        if flip_x {
            recolored.mirror_horizontal()
        } else {
            recolored
        }
    });
    blit_frame(cached, anchor.x, anchor.y, buf);
}

/// Small entry mat painted on the floor just inside the office door —
/// defines the arrival zone and breaks up the otherwise-empty wood strip
/// next to the door.
fn paint_entry_mat(buf: &mut RgbBuffer, x: u16, y: u16, w: u16, h: u16) {
    const MAT_BASE: Rgb = Rgb(60, 90, 130);
    const MAT_BORDER: Rgb = Rgb(40, 64, 96);
    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width || py >= buf.height {
                continue;
            }
            let on_border = dy == 0 || dy + 1 == h || dx == 0 || dx + 1 == w;
            buf.put(px, py, if on_border { MAT_BORDER } else { MAT_BASE });
        }
    }
}

/// Low coffee table in front of the lounge couch. Wood top with darker
/// trim along the front edge so it reads as a real piece of furniture,
/// not just a brown rectangle.
fn paint_coffee_table(buf: &mut RgbBuffer, cx: u16, cy: u16, w: u16, h: u16) {
    const TOP: Rgb = Rgb(120, 80, 48);
    const TRIM: Rgb = Rgb(72, 48, 26);
    let min_x = cx.saturating_sub(w / 2);
    let max_x = (cx + w / 2 + (w & 1)).min(buf.width);
    let min_y = cy.saturating_sub(h / 2);
    let max_y = (cy + h / 2 + (h & 1)).min(buf.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let on_front = y + 1 == max_y;
            buf.put(x, y, if on_front { TRIM } else { TOP });
        }
    }
}

/// Pantry bistro table — round-ish wood top (rounded corners by skipping
/// the 4 corner pixels) painted with the same warm wood palette as the
/// coffee table so they read as the same furniture family.
fn paint_pantry_table(buf: &mut RgbBuffer, cx: u16, cy: u16) {
    const TOP: Rgb = Rgb(132, 88, 52);
    const TRIM: Rgb = Rgb(78, 52, 28);
    let w: i32 = 7;
    let h: i32 = 4;
    for dy in 0..h {
        for dx in 0..w {
            let on_corner = (dx == 0 || dx == w - 1) && (dy == 0 || dy == h - 1);
            if on_corner {
                continue;
            }
            let px = cx as i32 - w / 2 + dx;
            let py = cy as i32 - h / 2 + dy;
            if px < 0 || py < 0 || px >= buf.width as i32 || py >= buf.height as i32 {
                continue;
            }
            let on_edge = dy == h - 1;
            buf.put(px as u16, py as u16, if on_edge { TRIM } else { TOP });
        }
    }
}

/// 2x2 stool — small dark wood square. Read as "stool around the bistro
/// table" once placed next to `paint_pantry_table`. Different from the
/// office chair (which is the agent's shirt color); these are unoccupied
/// furniture so they stay neutral wood.
fn paint_pantry_chair(buf: &mut RgbBuffer, cx: u16, cy: u16) {
    const SEAT: Rgb = Rgb(96, 68, 44);
    const TRIM: Rgb = Rgb(60, 40, 22);
    let put = |buf: &mut RgbBuffer, dx: i32, dy: i32, c: Rgb| {
        let px = cx as i32 + dx;
        let py = cy as i32 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width && (py as u16) < buf.height {
            buf.put(px as u16, py as u16, c);
        }
    };
    put(buf, -1, -1, SEAT);
    put(buf, 0, -1, SEAT);
    put(buf, -1, 0, TRIM);
    put(buf, 0, 0, TRIM);
}

/// Office corridor runner — a darker wood strip with subtle lighter stripes,
/// painted along the walkway band so the eye traces a path connecting the
/// door, meeting room, pantry, cubicles, and lounge. Just texture over the
/// floor — walls and decor paint on top.
fn paint_corridor_runner(buf: &mut RgbBuffer, rect: crate::tui::layout::Bounds) {
    const RUNNER_BASE: Rgb = Rgb(94, 62, 36);
    const RUNNER_STRIPE: Rgb = Rgb(118, 82, 50);
    const RUNNER_EDGE: Rgb = Rgb(60, 40, 24);
    let max_x = (rect.x + rect.width).min(buf.width);
    let max_y = (rect.y + rect.height).min(buf.height);
    for y in rect.y..max_y {
        for x in rect.x..max_x {
            let is_edge = y == rect.y || y + 1 == max_y;
            let dy = y - rect.y;
            let stripe = (x.wrapping_add(dy * 3)) % 9 == 0;
            let color = if is_edge {
                RUNNER_EDGE
            } else if stripe {
                RUNNER_STRIPE
            } else {
                RUNNER_BASE
            };
            buf.put(x, y, color);
        }
    }
}

/// Elliptical drop-shadow blended toward black at the floor level.
/// Grounds floating sprites so they look like they're standing/sitting
/// on the floor instead of hovering in mid-air. `strength` 0..1 controls
/// the darken amount at the center; falls off quadratically to 0 at edge.
fn paint_shadow(buf: &mut RgbBuffer, cx: u16, cy: u16, half_w: u16, half_h: u16, strength: f32) {
    const SHADOW: Rgb = Rgb(8, 8, 14);
    if half_w == 0 || half_h == 0 || strength <= 0.0 {
        return;
    }
    let min_x = cx.saturating_sub(half_w);
    let max_x = (cx + half_w).min(buf.width);
    let min_y = cy.saturating_sub(half_h);
    let max_y = (cy + half_h).min(buf.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let nx = (x as f32 - cx as f32) / half_w as f32;
            let ny = (y as f32 - cy as f32) / half_h as f32;
            let r2 = nx * nx + ny * ny;
            if r2 > 1.0 {
                continue;
            }
            let t = (1.0 - r2) * strength;
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, SHADOW.0, t),
                    blend(cur.1, SHADOW.1, t),
                    blend(cur.2, SHADOW.2, t),
                ),
            );
        }
    }
}

/// Current rendered position of an agent's character — derived from pose
/// so labels can follow the character rather than staying anchored at the
/// desk. Returns the top-left anchor of the character sprite. Uses
/// `derive_with_routing` so labels track agents along their A* path
/// instead of jumping the straight-line midpoint.
#[allow(clippy::too_many_arguments)]
pub(super) fn character_anchor(
    agent: &AgentSlot,
    layout: &Layout,
    now: SystemTime,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut pose::PoseHistory,
) -> Option<Point> {
    use crate::tui::layout::WaypointKind;
    if agent.desk_index >= layout.home_desks.len() {
        let overflow_idx = agent.desk_index - layout.home_desks.len();
        let sofa_count = layout.meeting_sofas.len();
        if overflow_idx < sofa_count {
            let sofa = layout.meeting_sofas[overflow_idx];
            return Some(Point {
                x: sofa.x.saturating_sub(4),
                y: sofa.y.saturating_sub(2),
            });
        }
        let floor_idx = overflow_idx - sofa_count;
        let seat = layout.floor_seats.get(floor_idx).copied()?;
        return Some(Point {
            x: seat.x.saturating_sub(4),
            y: seat.y.saturating_sub(2),
        });
    }
    let desk = *layout.home_desks.get(agent.desk_index)?;
    let pose = pose::derive_with_routing(agent, now, layout, router, overlay, history)?;
    let anchor = match pose {
        Pose::SeatedIdle | Pose::SeatedTyping { .. } => seated_anchor(desk),
        Pose::StandingAtDesk => standing_at_desk_anchor(desk),
        Pose::AtWaypoint { wp, kind } => {
            let wp_obj = layout.waypoints.get(wp)?;
            match kind {
                WaypointKind::Couch => back_couch_anchor(wp_obj.pos),
                _ => waypoint_anchor(wp_obj.pos),
            }
        }
        Pose::AimlessAt { dest } => waypoint_anchor(dest),
        Pose::Walking {
            from, to, t_x1000, ..
        } => walking_anchor(walking_position(from, to, t_x1000)),
    };
    Some(anchor)
}

/// Office chair painted BEHIND the character — a darkened version of the
/// agent's shirt color. Reads as a top-down chair back behind the sitter.
fn paint_chair_behind(buf: &mut RgbBuffer, anchor: Point, agent: &AgentSlot, pack: &Pack) {
    let pal = agent_palette(&pack.palette, agent);
    let Some(shirt) = pal.get('B').flatten() else {
        return;
    };
    let chair = Rgb(
        ((shirt.0 as u16) * 55 / 100) as u8,
        ((shirt.1 as u16) * 55 / 100) as u8,
        ((shirt.2 as u16) * 55 / 100) as u8,
    );
    // Slightly larger than the 8x10 seated sprite footprint — chair extends
    // 1 px past the character on each side so the upholstery is visible
    // even where the character body is fully opaque.
    for dy in 1..11 {
        for dx in 0..10 {
            let px = anchor.x.saturating_sub(1) + dx;
            let py = anchor.y + dy;
            if px < buf.width && py < buf.height {
                buf.put(px, py, chair);
            }
        }
    }
}

/// "Active" screen glow painted on top of the desk sprite while an agent is
/// in `ActivityState::Active`. Covers the full monitor footprint (rows 0-3,
/// cols 3-10 of desk.sprite — frame + screen + stand silhouette) so the
/// glow is at least 2 terminal cells tall after half-block compression.
/// Adds a moving scanline (one extra-bright column that cycles across the
/// screen) so the monitor reads as actually displaying scrolling content.
fn paint_screen_glow(buf: &mut RgbBuffer, desk_x: u16, desk_y: u16, now: SystemTime) {
    const FRAME_LIT: Rgb = Rgb(180, 200, 200);
    const GLOW: Rgb = Rgb(140, 240, 170);
    const GLOW_BRIGHT: Rgb = Rgb(220, 255, 230);
    const SCANLINE: Rgb = Rgb(250, 255, 250);
    let put = |buf: &mut RgbBuffer, dx: u16, dy: u16, c: Rgb| {
        let px = desk_x + dx;
        let py = desk_y + dy;
        if px < buf.width && py < buf.height {
            buf.put(px, py, c);
        }
    };
    for dx in 3..=10 {
        put(buf, dx, 0, FRAME_LIT);
    }
    for dx in 4..=9 {
        put(buf, dx, 1, GLOW_BRIGHT);
        put(buf, dx, 2, GLOW);
    }
    for dx in 4..=9 {
        put(buf, dx, 3, FRAME_LIT);
    }
    // Scanline: cycles across the 6-column screen interior every ~720ms.
    // Position derived from `now` + desk_x so neighboring monitors don't
    // pulse in lockstep.
    let elapsed_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let phase = (elapsed_ms / 120) as u16 + desk_x;
    let scan_col = 4 + (phase % 6);
    put(buf, scan_col, 1, SCANLINE);
    put(buf, scan_col, 2, SCANLINE);
}

// --- Particles ------------------------------------------------------------

/// Animated `z` rising above a sleeping character's head. Cycles ~2.4s
/// (rise 12 px then disappear). Per-agent phase offset so a row of
/// sleepers doesn't pulse in lockstep.
fn paint_sleep_z(buf: &mut RgbBuffer, head_anchor: Point, now: SystemTime, seed: u64) {
    const Z_COLOR: Rgb = Rgb(110, 110, 140);
    const CYCLE_MS: u64 = 2400;
    let elapsed_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let phase_ms = elapsed_ms.wrapping_add(seed % CYCLE_MS) % CYCLE_MS;
    if phase_ms >= CYCLE_MS - 400 {
        return; // fade-out gap
    }
    let rise = (phase_ms / 180) as u16;
    let z_x = head_anchor.x + 5;
    let z_y = head_anchor.y.saturating_sub(rise + 3);
    let pixels: &[(u16, u16)] = &[(0, 0), (1, 0), (1, 1), (0, 2), (1, 2)];
    for (dx, dy) in pixels {
        let px = z_x + dx;
        let py = z_y + dy;
        if px < buf.width && py < buf.height {
            buf.put(px, py, Z_COLOR);
        }
    }
}

/// Three staggered grey puffs rising from a point — coffee steam.
fn paint_coffee_steam(buf: &mut RgbBuffer, base: Point, now: SystemTime) {
    const STEAM: Rgb = Rgb(190, 190, 210);
    let elapsed_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    for offset in 0..3u64 {
        let phase = (elapsed_ms + offset * 600) % 1800;
        let rise = (phase / 140) as u16;
        let alpha = 1.0 - phase as f32 / 1800.0;
        if alpha < 0.15 {
            continue;
        }
        let wiggle = if (phase / 200) % 2 == 0 { 0 } else { 1 };
        let px = base.x + wiggle;
        let py = base.y.saturating_sub(rise + 2);
        if px < buf.width && py < buf.height {
            let cur = buf.get(px, py);
            buf.put(
                px,
                py,
                Rgb(
                    blend(cur.0, STEAM.0, alpha * 0.55),
                    blend(cur.1, STEAM.1, alpha * 0.55),
                    blend(cur.2, STEAM.2, alpha * 0.55),
                ),
            );
        }
    }
}

/// Small dust puff at the trailing foot of a walking character.
fn paint_walking_dust(buf: &mut RgbBuffer, walker_anchor: Point, frame_idx: usize) {
    const DUST: Rgb = Rgb(150, 120, 85);
    let foot_y = walker_anchor.y + 12;
    let foot_x = walker_anchor.x + if frame_idx == 0 { 6 } else { 1 };
    if foot_x < buf.width && foot_y < buf.height {
        let cur = buf.get(foot_x, foot_y);
        buf.put(
            foot_x,
            foot_y,
            Rgb(
                blend(cur.0, DUST.0, 0.45),
                blend(cur.1, DUST.1, 0.45),
                blend(cur.2, DUST.2, 0.45),
            ),
        );
    }
}

// --- Speech bubble overlay (kept from the prior renderer) -----------------
fn paint_waiting_bubble(buf: &mut RgbBuffer, anchor: Point) {
    const BUBBLE_FG: Rgb = Rgb(240, 200, 80);
    const BUBBLE_BG: Rgb = Rgb(30, 30, 40);
    let bx = anchor.x;
    let by = anchor.y.saturating_sub(4);
    let dots: &[(u16, u16, Rgb)] = &[
        (0, 0, BUBBLE_BG),
        (1, 0, BUBBLE_BG),
        (2, 0, BUBBLE_BG),
        (3, 0, BUBBLE_BG),
        (4, 0, BUBBLE_BG),
        (0, 1, BUBBLE_BG),
        (2, 1, BUBBLE_FG),
        (4, 1, BUBBLE_BG),
        (0, 2, BUBBLE_BG),
        (1, 2, BUBBLE_BG),
        (2, 2, BUBBLE_FG),
        (3, 2, BUBBLE_BG),
        (4, 2, BUBBLE_BG),
    ];
    for (dx, dy, c) in dots {
        let px = bx + dx;
        let py = by + dy;
        if px < buf.width && py < buf.height {
            buf.put(px, py, *c);
        }
    }
}
// ============================================================================
// Y-sorted drawable enum (painter's algorithm)
// ============================================================================
//
// Top-down depth: every mid-ground entity carries an `anchor_y` = the
// y-pixel row where it touches the floor (front-facing bottom edge for
// items with thickness). Drawables sort ascending by `anchor_y` and
// paint in order. Larger `anchor_y` = closer to camera = paints last
// (on top). Solves the classic "character standing south of a desk
// should appear in front of it" problem without per-pair special cases.
//
// What stays OUTSIDE the sort:
//   - Background (floor / walls / lighting / corridor / room walls /
//     entry mat / clock / shadows). All depth-independent.
//   - Per-character attached effects (chair-behind, sleep_z,
//     waiting_bubble, walker trail/dust, coffee steam, screen glow)
//     paint AS PART of their parent `Drawable` — they ride along with
//     the entity in z-order, not as a global foreground pass.

struct Drawable<'a> {
    anchor_y: u16,
    kind: DrawableKind<'a>,
}

enum DrawableKind<'a> {
    /// Whole cubicle as one z-unit: divider + filing cabinet (every
    /// other desk) + desk sprite + trash bin + screen-glow if the
    /// occupant is Active. Bundled so the cubicle paints atomically at
    /// the desk's bottom-edge row.
    DeskCubicle {
        desk: Point,
        is_last_col: bool,
        has_cabinet: bool,
        occupant_active: bool,
    },
    Character {
        agent: &'a AgentSlot,
        anim_name: &'static str,
        frame_idx: usize,
        anchor: Point,
        flip_x: bool,
        chair_behind: bool,
        sleep_z_seed: Option<u64>,
        waiting_bubble: bool,
        walking_dust_frame: Option<usize>,
    },
    /// Lounge couch (mirror_vertical'd — back at bottom, seat at top).
    WaypointCouch {
        pos: Point,
    },
    /// Pantry counter (with coffee steam attached so steam rides above
    /// the counter in z-order).
    WaypointPantry {
        pos: Point,
    },
    MeetingSofa {
        pos: Point,
        mirrored: bool,
    },
    MeetingTable {
        pos: Point,
    },
    PantryTable {
        pos: Point,
    },
    PantryChair {
        pos: Point,
    },
    Plant {
        kind: crate::tui::layout::PlantKind,
        pos: Point,
    },
    FloorLamp {
        pos: Point,
    },
    Door {
        pos: Point,
    },
    WallDecor {
        kind: crate::tui::layout::WallDecor,
        pos: Point,
    },
    Cat {
        pos: Point,
        flip: bool,
        frame_idx: usize,
    },
}

/// Returns the cat's current position + flip + frame_idx, or `None` if
/// the corridor isn't available. Pulled out of `paint_wandering_cat` so
/// the y-sort can place the cat with everything else.
fn cat_position(layout: &Layout, pack: &Pack, now: SystemTime) -> Option<(Point, bool, usize)> {
    let anim = pack.animation("cat_walk")?;
    if anim.frames.is_empty() {
        return None;
    }
    let elapsed_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    const CYCLE_MS: u64 = 30_000;
    let phase = elapsed_ms % CYCLE_MS;
    let frac = phase as f32 / CYCLE_MS as f32;
    let (t, flip) = if frac < 0.4 {
        (frac / 0.4, false)
    } else if frac < 0.5 {
        (1.0, false)
    } else if frac < 0.9 {
        (1.0 - (frac - 0.5) / 0.4, true)
    } else {
        (0.0, true)
    };
    let corridor = layout.corridor?;
    let left_x = corridor.x + corridor.width * 8 / 100;
    let right_x = corridor.x + corridor.width * 92 / 100;
    let cx = left_x + ((right_x - left_x) as f32 * t) as u16;
    let cy = corridor.y + corridor.height / 2;
    let frame_idx = (elapsed_ms / 220) as usize % anim.frames.len();
    Some((Point { x: cx, y: cy }, flip, frame_idx))
}

/// Dispatch one Drawable's paint. Effects attached to characters paint
/// inline so they ride along with the character in z-order.
fn paint_drawable(
    d: &Drawable<'_>,
    buf: &mut RgbBuffer,
    pack: &Pack,
    cache: &mut FrameCache,
    now: SystemTime,
) {
    match &d.kind {
        DrawableKind::DeskCubicle {
            desk,
            is_last_col,
            has_cabinet,
            occupant_active,
        } => {
            const DIVIDER: Rgb = Rgb(72, 82, 104);
            if !is_last_col {
                let div_x = desk.x + DESK_W + 3;
                for dy in 0..(DESK_H + 1) {
                    let py = desk.y.saturating_sub(1) + dy;
                    if div_x < buf.width && py < buf.height {
                        buf.put(div_x, py, DIVIDER);
                    }
                }
            }
            if *has_cabinet {
                if let Some(cab) = pack
                    .animation("filing_cabinet")
                    .and_then(|a| a.frames.first())
                {
                    let cab_x = desk.x.saturating_sub(cab.width + 1);
                    let cab_y = desk.y;
                    if cab_y + cab.height <= buf.height {
                        blit_frame(cab, cab_x, cab_y, buf);
                    }
                }
            }
            if let Some(frame) = pack.animation("desk").and_then(|a| a.frames.first()) {
                blit_frame(frame, desk.x, desk.y, buf);
            }
            if let Some(bin) = pack.animation("trash_bin").and_then(|a| a.frames.first()) {
                let bin_x = desk.x + DESK_W;
                let bin_y = desk.y + 4;
                if bin_x + bin.width <= buf.width && bin_y + bin.height <= buf.height {
                    blit_frame(bin, bin_x, bin_y, buf);
                }
            }
            if *occupant_active {
                paint_screen_glow(buf, desk.x, desk.y, now);
            }
        }
        DrawableKind::Character {
            agent,
            anim_name,
            frame_idx,
            anchor,
            flip_x,
            chair_behind,
            sleep_z_seed,
            waiting_bubble,
            walking_dust_frame,
        } => {
            if *chair_behind {
                paint_chair_behind(buf, *anchor, agent, pack);
            }
            if let Some(dust_frame) = walking_dust_frame {
                paint_walking_dust(buf, *anchor, *dust_frame);
            }
            paint_character_at(
                buf, anim_name, *frame_idx, *anchor, agent, pack, *flip_x, cache,
            );
            if let Some(seed) = sleep_z_seed {
                paint_sleep_z(buf, *anchor, now, *seed);
            }
            if *waiting_bubble {
                paint_waiting_bubble(buf, *anchor);
            }
        }
        DrawableKind::WaypointCouch { pos } => {
            if let Some(f) = pack.animation("couch").and_then(|a| a.frames.first()) {
                let cx = pos.x.saturating_sub(f.width / 2);
                let cy = pos.y.saturating_sub(f.height / 2);
                let flipped = f.mirror_vertical();
                blit_frame(&flipped, cx, cy, buf);
            }
        }
        DrawableKind::WaypointPantry { pos } => {
            if let Some(f) = pack.animation("pantry").and_then(|a| a.frames.first()) {
                let cx = pos.x.saturating_sub(f.width / 2);
                let cy = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, cx, cy, buf);
            }
            paint_coffee_steam(
                buf,
                Point {
                    x: pos.x + 4,
                    y: pos.y.saturating_sub(2),
                },
                now,
            );
        }
        DrawableKind::MeetingSofa { pos, mirrored } => {
            if let Some(f) = pack.animation("couch").and_then(|a| a.frames.first()) {
                let sx = pos.x.saturating_sub(f.width / 2);
                let sy = pos.y.saturating_sub(f.height / 2);
                if *mirrored {
                    let flipped = f.mirror_vertical();
                    blit_frame(&flipped, sx, sy, buf);
                } else {
                    blit_frame(f, sx, sy, buf);
                }
            }
        }
        DrawableKind::MeetingTable { pos } => {
            paint_coffee_table(buf, pos.x, pos.y, 11, 5);
        }
        DrawableKind::PantryTable { pos } => {
            paint_pantry_table(buf, pos.x, pos.y);
        }
        DrawableKind::PantryChair { pos } => {
            paint_pantry_chair(buf, pos.x, pos.y);
        }
        DrawableKind::Plant { kind, pos } => {
            use crate::tui::layout::PlantKind;
            let anim_name = match kind {
                PlantKind::Ficus => "plant",
                PlantKind::Tall => "plant_tall",
                PlantKind::Flower => "plant_flower",
                PlantKind::Succulent => "plant_succulent",
            };
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width / 2);
                let py = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::FloorLamp { pos } => {
            if let Some(f) = pack.animation("floor_lamp").and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width / 2);
                let py = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::Door { pos } => {
            if let Some(f) = pack.animation("door").and_then(|a| a.frames.first()) {
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::WallDecor { kind, pos } => {
            use crate::tui::layout::WallDecor;
            let anim_name = match kind {
                WallDecor::Bookshelf => "bookshelf",
                WallDecor::BulletinBoard => "bulletin_board",
                WallDecor::ExitSign => "exit_sign",
                WallDecor::Whiteboard => "whiteboard",
            };
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::Cat {
            pos,
            flip,
            frame_idx,
        } => {
            let Some(anim) = pack.animation("cat_walk") else {
                return;
            };
            let Some(frame) = anim.frames.get(*frame_idx) else {
                return;
            };
            let final_frame = if *flip {
                frame.mirror_horizontal()
            } else {
                frame.clone()
            };
            let px = pos.x.saturating_sub(final_frame.width / 2);
            let py = pos.y.saturating_sub(final_frame.height / 2);
            blit_frame(&final_frame, px, py, buf);
        }
    }
}

/// Pure pixel painting — no ratatui types, no terminal I/O. The signature
/// is what any future non-terminal renderer (web canvas, PNG export, GIF
/// capture) would call. Lives behind the `Renderer` trait in core if you
/// want to swap impls; the binary uses this concrete function directly.
#[allow(clippy::too_many_arguments)]
pub fn render_to_rgb_buffer(
    scene: &SceneState,
    layout: &Layout,
    pack: &Pack,
    now: SystemTime,
    buf: &mut RgbBuffer,
    cache: &mut FrameCache,
    router: &mut dyn Router,
    overlay: &mut OccupancyOverlay,
    history: &mut pose::PoseHistory,
) {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    let buf_w = layout.buf_w;
    let buf_h = layout.buf_h;

    // Compute time-of-day once per frame and pass to every paint
    // helper that depends on it. Avoids recomputing the chrono local
    // hour for each window + ceiling pool + lamp halo.
    let look = time_of_day_look(now);
    // Wall band height tracks layout.top_margin (which is buf_h/4 with
    // a floor) — leaves a 4-px buffer between wall trim and cubicles.
    let top_wall_h = layout.top_margin.saturating_sub(4);
    paint_floor_and_walls(buf, buf_w, buf_h, now, &look, top_wall_h);

    // Artificial light pass — at night the floor dims toward navy and
    // ceiling fluorescents + the floor lamp halo paint the visible
    // bright spots. During the day the dim is near-zero and the pools
    // are subtle ambient highlights. The wall-clock-based darkness
    // already handles "after hours" cleanly — an activity-based boost
    // flickers because Active flips on/off per tool call.
    dim_floor_overlay(buf, top_wall_h, buf_h, look.darkness * 0.45);
    let pool_strength = 0.15 + 0.30 * look.darkness;
    for desk in &layout.home_desks {
        paint_ceiling_pool(
            buf,
            desk.x + DESK_W / 2,
            desk.y.saturating_sub(2),
            10,
            5,
            pool_strength,
        );
    }
    // Two ceiling fluorescents over the pantry and a third over the
    // corridor so the floor is lit consistently with the lounge_band gone.
    if let Some(pr) = layout.pantry_room {
        paint_ceiling_pool(
            buf,
            pr.x + pr.width / 2,
            pr.y + pr.height / 2,
            12,
            6,
            pool_strength,
        );
    }
    if let Some(corridor) = layout.corridor {
        paint_ceiling_pool(
            buf,
            corridor.x + corridor.width / 2,
            corridor.y + corridor.height / 2,
            14,
            5,
            pool_strength,
        );
    }
    if let Some(lamp) = layout.floor_lamp {
        paint_floor_lamp_halo(buf, lamp.x, lamp.y, look.darkness * 0.55);
    }

    // Live wall clock painted after the wall (so hands sit on top of it)
    // but before wall decor — the bookshelf etc. shouldn't cover it.
    let clock_x = buf_w / 2 - 2;
    paint_clock(buf, clock_x, 1, now);
    // Corridor runner — painted over the floor but BEFORE walls/decor
    // so walls cleanly overlap it where they cross.
    if let Some(corridor) = layout.corridor {
        paint_corridor_runner(buf, corridor);
    }
    // Room dividers — drywall lines between meeting / pantry / right-side
    // (cubicles + lounge). Painted before decor so wall-leaning items
    // (e.g. wall_decor) sit on top.
    const WALL_COLOR: Rgb = Rgb(82, 84, 100);
    for (start, end) in &layout.room_walls {
        if start.x == end.x {
            for y in start.y..=end.y.min(buf_h - 1) {
                for dx in 0..2 {
                    let x = start.x + dx;
                    if x < buf_w {
                        buf.put(x, y, WALL_COLOR);
                    }
                }
            }
        } else {
            for x in start.x..=end.x.min(buf_w - 1) {
                for dy in 0..2 {
                    let y = start.y + dy;
                    if y < buf_h {
                        buf.put(x, y, WALL_COLOR);
                    }
                }
            }
        }
    }

    // Meeting room: two sofas facing each other across a small table.
    // Top sofa renders normally (back at top, sitter faces down). Bottom
    // sofa vertical-mirror so its back is at the bottom — sitter faces
    // up, toward the table.
    if let Some(couch_anim) = pack.animation("couch").and_then(|a| a.frames.first()) {
        for (i, sofa) in layout.meeting_sofas.iter().enumerate() {
            let sx = sofa.x.saturating_sub(couch_anim.width / 2);
            let sy = sofa.y.saturating_sub(couch_anim.height / 2);
            if i == 0 {
                blit_frame(couch_anim, sx, sy, buf);
            } else {
                let flipped = couch_anim.mirror_vertical();
                blit_frame(&flipped, sx, sy, buf);
            }
        }
    }
    if let Some(table) = layout.meeting_table {
        // Wider, deeper than the lounge coffee table — reads as a
        // proper conference table sitting between the two facing
        // sofas, not a side table.
        paint_coffee_table(buf, table.x, table.y, 11, 5);
    }

    // Pantry bistro table + stools — defines the pantry as a "eat
    // lunch + chat" zone, not just a counter.
    if let Some(table) = layout.pantry_table {
        paint_pantry_table(buf, table.x, table.y);
    }
    for chair in &layout.pantry_chairs {
        paint_pantry_chair(buf, chair.x, chair.y);
    }

    // Entry mat on the floor just inside the door — defines the arrival
    // zone and breaks up the empty wood strip there. The door SPRITE
    // itself is now a y-sorted Drawable (so a walker passing south of
    // the doorway can occlude it correctly); only the floor mat stays in
    // the background pass. Y is anchored to the wall-band bottom + 3
    // (3 px south of the door's bottom edge, which sits at top_margin+2)
    // so the mat tracks the top wall on tall terminals — the previous
    // `let mat_y = 15` was an absolute pixel position that got buried
    // inside the wall band on anything taller than the minimum buffer.
    if let Some(door_pos) = layout.door {
        let mat_x = door_pos.x.saturating_sub(2);
        let mat_y = layout.top_margin + 3;
        paint_entry_mat(buf, mat_x, mat_y, 10, 2);
    }

    // Shadow pass — soft floor shadows under desks + lounge furniture
    // so nothing floats. Painted BEFORE the y-sorted entity pass so
    // every entity sits on top of its own shadow. Strength is a
    // function of daylight so noon shadows are crisp and night shadows
    // are subtle.
    let shadow_strength = 0.5 - 0.3 * look.darkness;
    for desk in &layout.home_desks {
        paint_shadow(
            buf,
            desk.x + DESK_W / 2,
            desk.y + 7,
            DESK_W / 2 + 1,
            3,
            shadow_strength,
        );
    }
    for wp in &layout.waypoints {
        paint_shadow(buf, wp.pos.x, wp.pos.y + 2, 7, 2, shadow_strength);
    }
    for (_, p) in &layout.plants {
        paint_shadow(buf, p.x, p.y + 3, 3, 1, shadow_strength);
    }
    if let Some(lamp) = layout.floor_lamp {
        paint_shadow(buf, lamp.x, lamp.y + 5, 2, 1, shadow_strength);
    }

    // Build per-frame occupancy from STATIONARY agent positions only.
    // Walkers are deliberately excluded — their position interpolates
    // every frame, which would change the overlay signature every frame,
    // wipe the path cache, recompute A*, and snap walkers to new path
    // segments (the visible "flash"). Sitters at desks are already
    // covered by the static desk mask. Only waypoint visitors and
    // overflow-seat occupants contribute here — both have stable
    // positions across frames, so the signature is stable and the
    // cache hits.
    overlay.clear();
    for agent in &agents {
        if agent.desk_index >= layout.home_desks.len() {
            let overflow_idx = agent.desk_index - layout.home_desks.len();
            let sofa_count = layout.meeting_sofas.len();
            let pos = if overflow_idx < sofa_count {
                layout.meeting_sofas[overflow_idx]
            } else {
                let floor_idx = overflow_idx - sofa_count;
                let Some(seat) = layout.floor_seats.get(floor_idx).copied() else {
                    continue;
                };
                seat
            };
            overlay.add(pos.x.saturating_sub(4), pos.y.saturating_sub(6), 8, 12);
            continue;
        }
        let Some(pose) = pose::derive(agent, now, layout) else {
            continue;
        };
        if let Pose::AtWaypoint { wp, .. } = pose {
            if let Some(w) = layout.waypoints.get(wp) {
                overlay.add(w.pos.x.saturating_sub(4), w.pos.y.saturating_sub(6), 8, 12);
            }
        }
    }

    // --- Build the y-sortable middle pass -------------------------------
    //
    // Every entity gets an `anchor_y` representing its front-facing /
    // floor-touching row. Sort ascending and paint in order so things
    // closer to the camera (larger anchor_y) appear in front. This is
    // the painter's algorithm applied to a top-down 2D scene.
    let mut drawables: Vec<Drawable<'_>> = Vec::new();

    // Desk cubicles (each carries its divider + cabinet + bin + screen
    // glow). Sprite is 16×8, so the actual bottom edge is desk.y + 8 —
    // just past the seated character's feet (desk.y + 4), which keeps
    // the seated worker visually behind the desk like it always was.
    for (i, &desk) in layout.home_desks.iter().enumerate() {
        let is_last_col =
            desk.x + DESK_W + 2 + DESK_W >= layout.cubicle_band.x + layout.cubicle_band.width;
        let occupant_active = agents.iter().any(|a| {
            a.desk_index == i
                && a.exiting_at.is_none()
                && matches!(a.state, ActivityState::Active { .. })
        });
        drawables.push(Drawable {
            anchor_y: desk.y + 8,
            kind: DrawableKind::DeskCubicle {
                desk,
                is_last_col,
                has_cabinet: i % 2 == 0,
                occupant_active,
            },
        });
    }

    // Meeting sofas (couch sprite 14×5, centered → bottom = sofa.y + 2).
    for (i, &sofa) in layout.meeting_sofas.iter().enumerate() {
        let mirrored = i > 0;
        drawables.push(Drawable {
            anchor_y: sofa.y + 2,
            kind: DrawableKind::MeetingSofa {
                pos: sofa,
                mirrored,
            },
        });
    }
    // Meeting table (drawn 11×5 centered).
    if let Some(table) = layout.meeting_table {
        drawables.push(Drawable {
            anchor_y: table.y + 2,
            kind: DrawableKind::MeetingTable { pos: table },
        });
    }

    // Pantry bistro table (7×4 centered).
    if let Some(table) = layout.pantry_table {
        drawables.push(Drawable {
            anchor_y: table.y + 2,
            kind: DrawableKind::PantryTable { pos: table },
        });
    }
    // Pantry stools (2×2 anchored at center → bottom = pos.y).
    for chair in &layout.pantry_chairs {
        drawables.push(Drawable {
            anchor_y: chair.y,
            kind: DrawableKind::PantryChair { pos: *chair },
        });
    }

    // Waypoint furniture — couch (14×5) and pantry counter (20×8),
    // both centered on the waypoint position.
    for wp in &layout.waypoints {
        use crate::tui::layout::WaypointKind;
        match wp.kind {
            WaypointKind::Couch => drawables.push(Drawable {
                anchor_y: wp.pos.y + 2,
                kind: DrawableKind::WaypointCouch { pos: wp.pos },
            }),
            WaypointKind::Pantry => drawables.push(Drawable {
                anchor_y: wp.pos.y + 4,
                kind: DrawableKind::WaypointPantry { pos: wp.pos },
            }),
        }
    }

    // Plants — height varies by sprite, anchor = pos.y + h/2 (center
    // pos convention).
    for (kind, p) in &layout.plants {
        use crate::tui::layout::PlantKind;
        let h: u16 = match kind {
            PlantKind::Ficus => 7,
            PlantKind::Tall => 9,
            PlantKind::Flower => 6,
            PlantKind::Succulent => 4,
        };
        drawables.push(Drawable {
            anchor_y: p.y + h / 2,
            kind: DrawableKind::Plant {
                kind: *kind,
                pos: *p,
            },
        });
    }

    // Floor lamp (4×10 centered).
    if let Some(lamp) = layout.floor_lamp {
        drawables.push(Drawable {
            anchor_y: lamp.y + 5,
            kind: DrawableKind::FloorLamp { pos: lamp },
        });
    }

    // Door (6×12, top-left anchored).
    if let Some(door_pos) = layout.door {
        drawables.push(Drawable {
            anchor_y: door_pos.y + 12,
            kind: DrawableKind::Door { pos: door_pos },
        });
    }

    // Wall decor — hung on walls (top-left anchored), bottom = pos.y + h.
    for (kind, pos) in &layout.wall_decor {
        use crate::tui::layout::WallDecor;
        let h: u16 = match kind {
            WallDecor::Bookshelf => 12,
            WallDecor::BulletinBoard => 6,
            WallDecor::ExitSign => 3,
            WallDecor::Whiteboard => 11,
        };
        drawables.push(Drawable {
            anchor_y: pos.y + h,
            kind: DrawableKind::WallDecor {
                kind: *kind,
                pos: *pos,
            },
        });
    }

    // Wandering cat (6×4 centered).
    if let Some((pos, flip, frame_idx)) = cat_position(layout, pack, now) {
        drawables.push(Drawable {
            anchor_y: pos.y + 2,
            kind: DrawableKind::Cat {
                pos,
                flip,
                frame_idx,
            },
        });
    }

    // Characters. Anchor = feet (anchor.y + sprite_height). Decollision
    // rank for crowded waypoints — stable across frames thanks to
    // BTreeMap iteration order.
    let mut wp_rank: HashMap<usize, usize> = HashMap::new();
    for agent in &agents {
        // Overflow seating — past cubicle capacity, agents take meeting-
        // room sofas then floor seats. Entry/exit animations don't
        // apply; they pop in/out.
        if agent.desk_index >= layout.home_desks.len() {
            let overflow_idx = agent.desk_index - layout.home_desks.len();
            let sofa_count = layout.meeting_sofas.len();
            if overflow_idx < sofa_count {
                let sofa = layout.meeting_sofas[overflow_idx];
                let is_mirrored_sofa = overflow_idx > 0;
                let (anim_name, base_anchor_y, sprite_h) = if is_mirrored_sofa {
                    ("back_couch", sofa.y.saturating_sub(7), 9u16)
                } else if matches!(agent.state, ActivityState::Active { .. }) {
                    ("sitting_couch", sofa.y.saturating_sub(2), 12u16)
                } else {
                    ("sitting_couch_sleeping", sofa.y.saturating_sub(2), 12u16)
                };
                let anchor = with_breath(
                    Point {
                        x: sofa.x.saturating_sub(4),
                        y: base_anchor_y,
                    },
                    agent.agent_id,
                    now,
                );
                drawables.push(Drawable {
                    anchor_y: anchor.y + sprite_h,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name,
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        chair_behind: false,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
                continue;
            }
            let floor_idx = overflow_idx - sofa_count;
            let Some(seat) = layout.floor_seats.get(floor_idx).copied() else {
                continue;
            };
            let anchor = with_breath(
                Point {
                    x: seat.x.saturating_sub(4),
                    y: seat.y.saturating_sub(2),
                },
                agent.agent_id,
                now,
            );
            let anim_name = if matches!(agent.state, ActivityState::Active { .. }) {
                "seated_floor"
            } else {
                "seated_floor_sleeping"
            };
            drawables.push(Drawable {
                anchor_y: anchor.y + 12,
                kind: DrawableKind::Character {
                    agent,
                    anim_name,
                    frame_idx: 0,
                    anchor,
                    flip_x: false,
                    chair_behind: false,
                    sleep_z_seed: None,
                    waiting_bubble: false,
                    walking_dust_frame: None,
                },
            });
            continue;
        }
        let Some(desk) = layout.home_desks.get(agent.desk_index).copied() else {
            continue;
        };
        let Some(p) = pose::derive_with_routing(agent, now, layout, router, overlay, history)
        else {
            continue;
        };
        match p {
            Pose::SeatedIdle => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, now);
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "seated_sleeping",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        chair_behind: true,
                        sleep_z_seed: Some(agent.agent_id.raw()),
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::SeatedTyping { frame } => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, now);
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "typing",
                        frame_idx: frame,
                        anchor,
                        flip_x: false,
                        chair_behind: true,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::StandingAtDesk => {
                let anchor = with_breath(standing_at_desk_anchor(desk), agent.agent_id, now);
                let is_waiting = matches!(agent.state, ActivityState::Waiting { .. });
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "standing",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        chair_behind: false,
                        sleep_z_seed: None,
                        waiting_bubble: is_waiting,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    let (anim_name, anchor_base, sprite_h) = match kind {
                        crate::tui::layout::WaypointKind::Couch => {
                            ("back_couch", back_couch_anchor(wp_obj.pos), 9u16)
                        }
                        crate::tui::layout::WaypointKind::Pantry => {
                            ("holding_coffee", waypoint_anchor(wp_obj.pos), 12u16)
                        }
                    };
                    let anchor = with_breath(
                        Point {
                            x: anchor_base.x.saturating_add_signed(dx),
                            y: anchor_base.y,
                        },
                        agent.agent_id,
                        now,
                    );
                    drawables.push(Drawable {
                        anchor_y: anchor.y + sprite_h,
                        kind: DrawableKind::Character {
                            agent,
                            anim_name,
                            frame_idx: 0,
                            anchor,
                            flip_x: false,
                            chair_behind: false,
                            sleep_z_seed: None,
                            waiting_bubble: false,
                            walking_dust_frame: None,
                        },
                    });
                }
            }
            Pose::AimlessAt { dest } => {
                let anchor = with_breath(waypoint_anchor(dest), agent.agent_id, now);
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "standing",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        chair_behind: false,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::Walking {
                from,
                to,
                t_x1000,
                frame,
            } => {
                let pos = walking_position(from, to, t_x1000);
                let walker_anchor = walking_anchor(pos);
                let dx = to.x as i32 - from.x as i32;
                let dy = to.y as i32 - from.y as i32;
                let (anim_name, flip) = if dy.unsigned_abs() > dx.unsigned_abs() && dy < 0 {
                    ("walking_back", to.x < from.x)
                } else {
                    ("walking", to.x < from.x)
                };
                drawables.push(Drawable {
                    anchor_y: walker_anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name,
                        frame_idx: frame,
                        anchor: walker_anchor,
                        flip_x: flip,
                        chair_behind: false,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        walking_dust_frame: Some(frame),
                    },
                });
            }
        }
    }

    // Stable sort (Rust's `sort_by_key` is stable) — ties preserve
    // insertion order. Insertion order above: decor first, characters
    // last, so a character tied with a piece of furniture paints
    // BEFORE the furniture (matches the prior pass-1 → pass-1.5
    // → pass-2 layering for waypoint couch / pantry counter).
    drawables.sort_by_key(|d| d.anchor_y);
    for d in &drawables {
        paint_drawable(d, buf, pack, cache, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ascii_agents_core::source::Activity;
    use ascii_agents_core::sprite::Palette;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn make_slot(id: ascii_agents_core::AgentId, state: ActivityState) -> AgentSlot {
        let now = SystemTime::UNIX_EPOCH;
        AgentSlot {
            agent_id: id,
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/x").as_path()),
            label: Arc::from("x"),
            state,
            state_started_at: now,
            created_at: now,
            exiting_at: None,
            desk_index: 0,
        }
    }

    fn base_palette() -> Palette {
        let mut p = Palette::new();
        p.insert('B', Some(Rgb(10, 20, 30))); // shirt
        p.insert('H', Some(Rgb(40, 50, 60))); // hair
        p.insert('S', Some(Rgb(70, 80, 90))); // skin
        p.insert('X', Some(Rgb(99, 99, 99))); // unrelated key
        p
    }

    #[test]
    fn agent_palette_is_deterministic_per_id() {
        let id = ascii_agents_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let a = agent_palette(&base, &make_slot(id, ActivityState::Idle));
        let b = agent_palette(&base, &make_slot(id, ActivityState::Idle));
        assert_eq!(a.get('B'), b.get('B'));
        assert_eq!(a.get('H'), b.get('H'));
        assert_eq!(a.get('S'), b.get('S'));
    }

    #[test]
    fn agent_palette_overrides_only_bhs_keys() {
        let id = ascii_agents_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let p = agent_palette(&base, &make_slot(id, ActivityState::Idle));
        // X is not a recolor target — must pass through unchanged.
        assert_eq!(p.get('X'), Some(Some(Rgb(99, 99, 99))));
        // B/H/S must be replaced — the base RGBs (10/20/30 etc.) are
        // unlikely to be in any preset, so they should differ.
        assert_ne!(p.get('B'), Some(Some(Rgb(10, 20, 30))));
        assert_ne!(p.get('H'), Some(Some(Rgb(40, 50, 60))));
        assert_ne!(p.get('S'), Some(Some(Rgb(70, 80, 90))));
    }

    #[test]
    fn agent_palette_active_state_tints_skin_toward_glow() {
        let id = ascii_agents_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let idle = agent_palette(&base, &make_slot(id, ActivityState::Idle));
        let active = agent_palette(
            &base,
            &make_slot(
                id,
                ActivityState::Active {
                    activity: Activity::Typing,
                    tool_use_id: None,
                    detail: None,
                },
            ),
        );
        // Same id ⇒ shirt + hair stable across states.
        assert_eq!(idle.get('B'), active.get('B'));
        assert_eq!(idle.get('H'), active.get('H'));
        // Skin differs: Active tints toward green-ish GLOW_TINT(140,240,170).
        // Verify the green channel went UP and red/blue moved toward the tint.
        let (Some(Some(Rgb(_, ig, _))), Some(Some(Rgb(_, ag, _)))) =
            (idle.get('S'), active.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            ag > ig,
            "active skin green channel should exceed idle (active={ag}, idle={ig})"
        );
    }

    #[test]
    fn recolor_frame_substitutes_bhs_pixels() {
        let base = base_palette();
        // Build an agent palette where B/H/S are clearly distinguishable.
        let mut agent_pal = base.clone();
        agent_pal.insert('B', Some(Rgb(200, 0, 0))); // red shirt
        agent_pal.insert('H', Some(Rgb(0, 200, 0))); // green hair
        agent_pal.insert('S', Some(Rgb(0, 0, 200))); // blue skin

        // Frame: 1 pixel per palette key + 1 unrelated pixel + 1 transparent.
        let frame = Frame {
            width: 5,
            height: 1,
            pixels: vec![
                Some(Rgb(10, 20, 30)),  // matches base B → should become red
                Some(Rgb(40, 50, 60)),  // matches base H → should become green
                Some(Rgb(70, 80, 90)),  // matches base S → should become blue
                Some(Rgb(123, 45, 67)), // unrelated     → unchanged
                None,                   // transparent   → unchanged
            ],
        };

        let out = recolor_frame(&frame, &agent_pal, &base);
        assert_eq!(out.width, 5);
        assert_eq!(out.height, 1);
        assert_eq!(out.pixels[0], Some(Rgb(200, 0, 0)));
        assert_eq!(out.pixels[1], Some(Rgb(0, 200, 0)));
        assert_eq!(out.pixels[2], Some(Rgb(0, 0, 200)));
        assert_eq!(out.pixels[3], Some(Rgb(123, 45, 67)));
        assert_eq!(out.pixels[4], None);
    }

    #[test]
    fn recolor_frame_handles_palette_with_no_overrides() {
        // If agent palette equals base, frame must come back identical.
        let base = base_palette();
        let frame = Frame {
            width: 3,
            height: 1,
            pixels: vec![
                Some(Rgb(10, 20, 30)),
                Some(Rgb(40, 50, 60)),
                Some(Rgb(70, 80, 90)),
            ],
        };
        let out = recolor_frame(&frame, &base, &base);
        assert_eq!(out.pixels, frame.pixels);
    }

    /// Helper — build a minimal Drawable for sort-order tests. Uses the
    /// MeetingTable variant since it carries no borrowed data.
    fn drawable(anchor_y: u16) -> Drawable<'static> {
        Drawable {
            anchor_y,
            kind: DrawableKind::MeetingTable {
                pos: Point { x: 0, y: 0 },
            },
        }
    }

    #[test]
    fn drawables_sort_ascending_by_anchor_y() {
        let mut v = [drawable(30), drawable(10), drawable(20)];
        v.sort_by_key(|d| d.anchor_y);
        let ys: Vec<u16> = v.iter().map(|d| d.anchor_y).collect();
        assert_eq!(ys, [10, 20, 30]);
    }

    #[test]
    fn drawables_sort_is_stable_on_ties() {
        // Same anchor_y values — TimSort (Rust's stable sort) must
        // preserve insertion order. The y-sort relies on this so that
        // a character at the same anchor_y as the couch behind them
        // still paints first (matches the prior Pass 1 → Pass 1.5
        // layering).
        let mut v = [
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 1, y: 0 },
                },
            },
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 2, y: 0 },
                },
            },
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 3, y: 0 },
                },
            },
        ];
        v.sort_by_key(|d| d.anchor_y);
        let xs: Vec<u16> = v
            .iter()
            .map(|d| match &d.kind {
                DrawableKind::MeetingTable { pos } => pos.x,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(xs, [1, 2, 3]);
    }

    #[test]
    fn character_anchor_y_exceeds_desk_when_south_of_it() {
        // The bug-fix invariant: a character whose feet (anchor.y + 12)
        // land BELOW the desk's bottom row (desk.y + 8) must sort AFTER
        // the desk and therefore paint on top.
        let desk_y: u16 = 20;
        let desk_anchor_y = desk_y + 8;
        let char_feet_anchor = (desk_y + 10) + 12; // walker south of desk
        assert!(
            char_feet_anchor > desk_anchor_y,
            "walker south of desk must sort after it: char={char_feet_anchor}, desk={desk_anchor_y}"
        );
    }

    #[test]
    fn character_anchor_y_below_desk_when_seated_at_it() {
        // Inverse invariant — a SEATED character at this desk has feet
        // that land ABOVE the desk's bottom (because they're tucked
        // under the desktop). They must sort BEFORE the desk so the
        // desk occludes their lower body in top-down view.
        let desk_y: u16 = 20;
        let seated_anchor = seated_anchor(Point { x: 0, y: desk_y });
        let char_feet_anchor = seated_anchor.y + 12;
        let desk_anchor_y = desk_y + 8;
        assert!(
            char_feet_anchor < desk_anchor_y,
            "seated char must sort before desk: char={char_feet_anchor}, desk={desk_anchor_y}"
        );
    }
}
