//! Top-down coworking-lounge renderer.
//!
//! Zone-based layout via `tui::layout`, state→pose derivation via `tui::pose`.
//! This file owns the actual pixel painting (floor, walls, decor, character
//! sprites, terminal flush). Layout and pose are pure functions tested in
//! isolation; this file is the integrator.

use std::collections::HashMap;
use std::io::{stdout, Stdout};
use std::time::SystemTime;

use anyhow::Result;
use ascii_agents_core::sprite::blit::blit_frame;
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentSlot, SceneState};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

use ascii_agents_core::walkable::OccupancyOverlay;

use crate::tui::layout::{Layout, Point, DESK_H, DESK_W};
use crate::tui::pathfind::Router;
use crate::tui::pose::{self, Pose};

pub type Term = Terminal<CrosstermBackend<Stdout>>;

// --- Colors ---------------------------------------------------------------
const BG: Rgb = Rgb(28, 32, 40);
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
const SHIRT_PRESETS_WARM: &[Rgb] = &[
    Rgb(0x9c, 0x27, 0x27), // crimson
    Rgb(0xc6, 0x6a, 0x1e), // burnt orange
    Rgb(0xb0, 0x32, 0xa8), // magenta
    Rgb(0xd0, 0x9c, 0x32), // mustard
];
/// Cool / homebody shirt palette — used for lower-trip-chance agents.
const SHIRT_PRESETS_COOL: &[Rgb] = &[
    Rgb(0x2e, 0x62, 0xcf), // royal blue
    Rgb(0x16, 0xa0, 0x6e), // forest green
    Rgb(0x32, 0x82, 0x9b), // teal
    Rgb(0x6c, 0x4f, 0x9e), // violet
];
const HAIR_PRESETS: &[Rgb] = &[
    Rgb(0x2a, 0x1a, 0x0e), // near-black
    Rgb(0x52, 0x32, 0x10), // dark brown
    Rgb(0xc7, 0xa3, 0x4a), // blond
    Rgb(0x7a, 0x32, 0x10), // auburn
    Rgb(0x3a, 0x3a, 0x3a), // dark grey
];
const SKIN_PRESETS: &[Rgb] = &[
    Rgb(0xf4, 0xc7, 0x9a), // light peach (matches base palette S)
    Rgb(0xe0, 0xa8, 0x70), // medium
    Rgb(0xb8, 0x80, 0x50), // tan
    Rgb(0x8a, 0x5a, 0x36), // deep brown
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
use crate::tui::frame_cache::FrameCache;
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

    // Sky gradient precomputed once per row instead of per pixel —
    // mix_lab is an sRGB→Lab→Mix→sRGB roundtrip and was the single
    // largest per-frame cost (a 22×12 window × 5 windows = 1320 calls /
    // frame). Now N=glass_h calls total per window.
    let skyline: &[u16] = &[3, 5, 4, 6, 3, 5, 4, 5, 3, 6, 4, 5];
    let lit_dots: &[(u16, u16)] = &[(1, 1), (3, 0), (5, 2), (7, 1), (9, 2), (2, 3), (6, 3)];
    let glass_h = h.saturating_sub(2);
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
            let building_h = skyline[((glass_dx + window_idx * 3) % skyline.len() as u16) as usize];
            let in_building = glass_dy >= glass_h.saturating_sub(building_h);

            if in_building {
                let bldg_y = glass_dy - (glass_h - building_h);
                let is_dot = lit_dots
                    .iter()
                    .any(|&(lx, ly)| lx == glass_dx && ly == bldg_y);
                if is_dot && city_dot_twinkle(window_idx, glass_dx, bldg_y, now) {
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

fn paint_lounge_decor(buf: &mut RgbBuffer, layout: &Layout, pack: &Pack, now: SystemTime) {
    // Plants — pure decor, scattered around the lounge. Each plant picks
    // a sprite per kind so the lounge has variety instead of one repeated
    // ficus.
    use crate::tui::layout::PlantKind;
    for (kind, p) in &layout.plants {
        let anim_name = match kind {
            PlantKind::Ficus => "plant",
            PlantKind::Tall => "plant_tall",
            PlantKind::Flower => "plant_flower",
            PlantKind::Succulent => "plant_succulent",
        };
        if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
            let px = p.x.saturating_sub(f.width / 2);
            let py = p.y.saturating_sub(f.height / 2);
            blit_frame(f, px, py, buf);
        }
    }

    // Floor lamp in the lounge corner.
    if let Some(lamp_pos) = layout.floor_lamp {
        if let Some(f) = pack.animation("floor_lamp").and_then(|a| a.frames.first()) {
            let px = lamp_pos.x.saturating_sub(f.width / 2);
            let py = lamp_pos.y.saturating_sub(f.height / 2);
            blit_frame(f, px, py, buf);
        }
    }

    // Wandering office cat — bounces between the left and right ends of
    // the lounge band on a 30 s cycle, with brief pauses at each end.
    // Pure whimsy, fills floor space.
    paint_wandering_cat(buf, layout, pack, now);
}

/// Waypoint furniture (couch + pantry counter) painted AFTER characters in
/// Pass 1 — so a visitor seated on the couch or standing at the counter
/// has their lower body occluded by the furniture sprite. Top-down 3/4
/// semantics: head + shoulders visible above the back rest / counter
/// edge, body hidden behind it. Same trick as desks, but in a separate
/// pass because the couch / counter need to occlude waypoint VISITORS,
/// not desk-seated workers.
fn paint_waypoint_furniture(buf: &mut RgbBuffer, layout: &Layout, pack: &Pack, now: SystemTime) {
    use crate::tui::layout::WaypointKind;
    for wp in &layout.waypoints {
        let anim_name = match wp.kind {
            WaypointKind::Couch => "couch",
            WaypointKind::Pantry => "pantry",
        };
        if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
            let cx = wp.pos.x.saturating_sub(f.width / 2);
            let cy = wp.pos.y.saturating_sub(f.height / 2);
            if wp.kind == WaypointKind::Couch {
                let flipped = f.mirror_vertical();
                blit_frame(&flipped, cx, cy, buf);
            } else {
                blit_frame(f, cx, cy, buf);
            }
        }
        // Coffee steam wisps emit OVER the counter — paint them last so
        // the animation reads as steam rising up from the machine.
        if wp.kind == WaypointKind::Pantry {
            paint_coffee_steam(
                buf,
                Point {
                    x: wp.pos.x + 4,
                    y: wp.pos.y.saturating_sub(2),
                },
                now,
            );
        }
    }
}

/// Office cat that paces between the lounge band's left and right edges
/// on a 30 s cycle (12 s walk + 3 s pause + 12 s walk back + 3 s pause).
/// Sprite mirrors horizontally for the return leg.
fn paint_wandering_cat(buf: &mut RgbBuffer, layout: &Layout, pack: &Pack, now: SystemTime) {
    let Some(anim) = pack.animation("cat_walk") else {
        return;
    };
    if anim.frames.is_empty() {
        return;
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
    // Cat now paces the full-width corridor instead of the old lounge_band.
    let corridor = match layout.corridor {
        Some(c) => c,
        None => return,
    };
    let left_x = corridor.x + corridor.width * 8 / 100;
    let right_x = corridor.x + corridor.width * 92 / 100;
    let cx = left_x + ((right_x - left_x) as f32 * t) as u16;
    let cy = corridor.y + corridor.height / 2;
    let frame_idx = (elapsed_ms / 220) as usize % anim.frames.len();
    let Some(frame) = anim.frames.get(frame_idx) else {
        return;
    };
    let final_frame = if flip {
        frame.mirror_horizontal()
    } else {
        frame.clone()
    };
    let px = cx.saturating_sub(final_frame.width / 2);
    let py = cy.saturating_sub(final_frame.height / 2);
    blit_frame(&final_frame, px, py, buf);
}

/// Wall-leaning furniture (bookshelf + whiteboard). Painted *after* the
/// wall band so it sits in front of the wall trim, leaning against it.
fn paint_wall_decor(buf: &mut RgbBuffer, layout: &Layout, pack: &Pack) {
    use crate::tui::layout::WallDecor;
    for (kind, pos) in &layout.wall_decor {
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
fn character_anchor(
    agent: &AgentSlot,
    layout: &Layout,
    now: SystemTime,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
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
    let pose = pose::derive_with_routing(agent, now, layout, router, overlay)?;
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

/// Clip a widget rect to fit inside `bounds`. Returns `None` if the rect
/// falls fully outside or has zero width/height after clipping — caller
/// uses that to skip the render entirely. Prevents ratatui's
/// "index outside of buffer" panic when label/notice widgets land near
/// the right or bottom edge.
fn clip_widget_rect(rect: Rect, bounds: Rect) -> Option<Rect> {
    if rect.x >= bounds.x + bounds.width || rect.y >= bounds.y + bounds.height {
        return None;
    }
    if rect.x + rect.width <= bounds.x || rect.y + rect.height <= bounds.y {
        return None;
    }
    let x = rect.x.max(bounds.x);
    let y = rect.y.max(bounds.y);
    let right = (rect.x + rect.width).min(bounds.x + bounds.width);
    let bot = (rect.y + rect.height).min(bounds.y + bounds.height);
    if right <= x || bot <= y {
        return None;
    }
    Some(Rect {
        x,
        y,
        width: right - x,
        height: bot - y,
    })
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

// --- draw_scene ----------------------------------------------------------
//
// `draw_scene` is the orchestrator: get terminal geometry, compute the
// layout, run the pure pixel pass, then flush to the terminal. The two
// helpers below are deliberately split:
//
//   * `render_to_rgb_buffer` — pure RGB output. No ratatui types, no
//     terminal I/O. Can be called by any renderer (web canvas, PNG
//     snapshot, GIF capture).
//   * `flush_to_terminal` — ratatui half-block compression + label overlay
//     + bulletin notice + footer. Terminal-specific, runs inside
//     `term.draw`.
#[allow(clippy::too_many_arguments)]
pub fn draw_scene<B: Backend>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    pack: &Pack,
    now: SystemTime,
    buf: &mut RgbBuffer,
    cache: &mut FrameCache,
    router: &mut dyn Router,
    overlay: &mut OccupancyOverlay,
) -> Result<()> {
    let term_size = term.size()?;
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
        term.draw(|f| paint_footer(f, full_rect))?;
        return Ok(());
    }

    let buf_w = scene_rect.width;
    let buf_h = scene_rect.height * 2;
    buf.ensure_size(buf_w, buf_h, BG);
    let Some(layout) = Layout::compute(buf_w, buf_h, scene.max_desks) else {
        term.draw(|f| paint_footer(f, full_rect))?;
        return Ok(());
    };

    // Bias the router toward the corridor (the office "main aisle") so
    // walkers naturally use the hallway instead of cutting diagonally
    // across the cubicle floor. Cheap call — invalidates the cache only
    // when the zone actually changes (layout resize).
    router.set_preferred_zone(layout.corridor);

    // Pure pixel pass — no ratatui types touched.
    render_to_rgb_buffer(scene, &layout, pack, now, buf, cache, router, overlay);

    // Terminal-flush pass — half-block + widgets, inside ratatui's draw.
    term.draw(|f| {
        paint_footer(f, full_rect);
        flush_buffer_to_term(f, buf, scene_rect);
        paint_label_widgets(f, scene, &layout, now, router, overlay, scene_rect);
        paint_bulletin_notice(f, scene, &layout, scene_rect);
    })?;
    Ok(())
}

fn paint_footer(f: &mut ratatui::Frame<'_>, full_rect: Rect) {
    let footer =
        Paragraph::new(Span::raw(" [q] quit ")).style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        footer,
        Rect {
            x: full_rect.x,
            y: full_rect.y + full_rect.height.saturating_sub(1),
            width: full_rect.width,
            height: 1,
        },
    );
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
    // are subtle ambient highlights.
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

    paint_wall_decor(buf, layout, pack);
    if let Some(door_pos) = layout.door {
        if let Some(frame) = pack.animation("door").and_then(|a| a.frames.first()) {
            blit_frame(frame, door_pos.x, door_pos.y, buf);
        }
        // Entry mat on the floor just inside the door. Defines the
        // arrival zone and breaks up the empty wood strip there.
        let mat_x = door_pos.x.saturating_sub(2);
        let mat_y = 15;
        paint_entry_mat(buf, mat_x, mat_y, 10, 2);
    }
    paint_lounge_decor(buf, layout, pack, now);

    // Shadow pass — soft floor shadows under desks + lounge furniture
    // so nothing floats. Painted AFTER decor (so they don't get
    // covered) and BEFORE characters (chairs/characters paint on
    // top). Strength is a function of daylight so noon shadows are
    // crisp and night shadows are subtle.
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
        // Static overflow seating (sofa workstation / floor seat).
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
        // Waypoint visitors (Couch / Coffee / Pantry) — block the
        // standing area so a passing walker routes around them.
        let Some(pose) = pose::derive(agent, now, layout) else {
            continue;
        };
        if let Pose::AtWaypoint { wp, .. } = pose {
            if let Some(w) = layout.waypoints.get(wp) {
                overlay.add(w.pos.x.saturating_sub(4), w.pos.y.saturating_sub(6), 8, 12);
            }
        }
        // Walking / AimlessAt / Seated / Standing — skipped here.
    }

    // Pass 1: characters by pose. Painted BEFORE the desk so the desk
    // can occlude the character's lower body — from a top-down POV the
    // viewer sees head + shoulders sticking up above the desk back
    // edge, and the body is hidden behind the desk. Each agent's home
    // desk is at home_desks[agent.desk_index] — NOT at the agent's
    // BTreeMap position.
    //
    // Waypoint de-collision: when multiple Idle agents pick the same
    // wander destination in the same cycle, fan them out spatially so
    // they don't stack into a single sprite. BTreeMap iteration order
    // gives a stable rank per (wp_idx) across frames.
    let mut wp_rank: HashMap<usize, usize> = HashMap::new();
    for agent in &agents {
        // Overflow seating: past cubicle capacity, next agents take
        // meeting-room sofas, then floor seats. Entry/exit animations
        // don't apply to these — they pop in/out.
        if agent.desk_index >= layout.home_desks.len() {
            let overflow_idx = agent.desk_index - layout.home_desks.len();
            let sofa_count = layout.meeting_sofas.len();
            if overflow_idx < sofa_count {
                let sofa = layout.meeting_sofas[overflow_idx];
                // Top sofa (idx 0) — forward-facing sitter on a couch
                // with back at TOP. Bottom sofa (idx 1+, sprite is
                // mirror_vertical'd with back at BOTTOM) — back-facing
                // sitter so the two meeting-room agents visually face
                // each other across the conference table.
                let is_mirrored_sofa = overflow_idx > 0;
                let (anim, anchor_y) = if is_mirrored_sofa {
                    ("back_couch", sofa.y.saturating_sub(7))
                } else if matches!(agent.state, ActivityState::Active { .. }) {
                    ("sitting_couch", sofa.y.saturating_sub(2))
                } else {
                    ("sitting_couch_sleeping", sofa.y.saturating_sub(2))
                };
                let anchor = with_breath(
                    Point {
                        x: sofa.x.saturating_sub(4),
                        y: anchor_y,
                    },
                    agent.agent_id,
                    now,
                );
                paint_character_at(buf, anim, 0, anchor, agent, pack, false, cache);
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
            let anim = match &agent.state {
                ActivityState::Active { .. } => "seated_floor",
                _ => "seated_floor_sleeping",
            };
            paint_character_at(buf, anim, 0, anchor, agent, pack, false, cache);
            continue;
        }
        let Some(desk) = layout.home_desks.get(agent.desk_index).copied() else {
            continue;
        };
        let Some(p) = pose::derive_with_routing(agent, now, layout, router, overlay) else {
            continue;
        };
        match p {
            Pose::SeatedIdle => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, now);
                paint_chair_behind(buf, anchor, agent, pack);
                paint_character_at(buf, "seated_sleeping", 0, anchor, agent, pack, false, cache);
                paint_sleep_z(buf, anchor, now, agent.agent_id.raw());
            }
            Pose::SeatedTyping { frame } => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, now);
                paint_chair_behind(buf, anchor, agent, pack);
                paint_character_at(buf, "typing", frame, anchor, agent, pack, false, cache);
            }
            Pose::StandingAtDesk => {
                let anchor = with_breath(standing_at_desk_anchor(desk), agent.agent_id, now);
                paint_character_at(buf, "standing", 0, anchor, agent, pack, false, cache);
                if matches!(agent.state, ActivityState::Waiting { .. }) {
                    paint_waiting_bubble(buf, anchor);
                }
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    // Couch sitter faces UP toward the city-view windows
                    // (back-view sprite, no face) since the couch is now
                    // mirror_vertical'd with back at the bottom.
                    let (anim_name, anchor_base) = match kind {
                        crate::tui::layout::WaypointKind::Couch => {
                            ("back_couch", back_couch_anchor(wp_obj.pos))
                        }
                        // Pantry visitors hold a coffee — the pantry sprite
                        // has the coffee machine on its counter and emits
                        // steam, so the visit doubles as "coffee break".
                        crate::tui::layout::WaypointKind::Pantry => {
                            ("holding_coffee", waypoint_anchor(wp_obj.pos))
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
                    paint_character_at(buf, anim_name, 0, anchor, agent, pack, false, cache);
                }
            }
            Pose::AimlessAt { dest } => {
                let anchor = with_breath(waypoint_anchor(dest), agent.agent_id, now);
                paint_character_at(buf, "standing", 0, anchor, agent, pack, false, cache);
            }
            Pose::Walking {
                from,
                to,
                t_x1000,
                frame,
            } => {
                let pos = walking_position(from, to, t_x1000);
                let walker_anchor = walking_anchor(pos);
                paint_walking_dust(buf, walker_anchor, frame);
                // Direction-aware sprite: when the walker is moving UP
                // more than horizontally (|dy| > |dx| and dy < 0), use
                // the back-view sprite so the viewer sees the back of
                // the head as they walk toward the windows. Otherwise
                // use the front-view sprite with horizontal flip to
                // face the direction of motion. Diagonals fall back on
                // whichever axis dominates.
                let dx = to.x as i32 - from.x as i32;
                let dy = to.y as i32 - from.y as i32;
                let (anim_name, flip) = if dy.unsigned_abs() > dx.unsigned_abs() && dy < 0 {
                    ("walking_back", to.x < from.x)
                } else {
                    ("walking", to.x < from.x)
                };
                paint_character_at(
                    buf,
                    anim_name,
                    frame,
                    walker_anchor,
                    agent,
                    pack,
                    flip,
                    cache,
                );
            }
        }
    }

    // Pass 1.5: waypoint furniture (couch + pantry counter). Painted
    // AFTER characters so a visitor at a waypoint has their lower body
    // occluded by the furniture — same top-down trick as desks.
    paint_waypoint_furniture(buf, layout, pack, now);

    // Pass 2: desks (+ trash bin + screen glow). Painted AFTER the
    // character so the desk occludes the character's lower body — top-
    // down POV reads as "person sitting BEHIND the desk", not "person
    // standing on the desk top". The screen glow sits on top of
    // everything, so it's a fully visible "this workstation is active"
    // cue. Trash bin tucked next to each desk for cubicle realism.
    const DIVIDER: Rgb = Rgb(72, 82, 104);
    let desk_anim = pack.animation("desk");
    let bin_anim = pack.animation("trash_bin");
    let cab_anim = pack.animation("filing_cabinet");
    // Iterate ALL desks (even unoccupied ones) so the cubicle furniture
    // is always painted. Then look up whether an agent is sitting here
    // to decide on the screen-glow / active-monitor overlay.
    for (i, desk) in layout.home_desks.iter().enumerate() {
        // 1 px partition (was 2 px) limited to the desk's vertical span —
        // half-height partial dividers, more "modern office", less "cube
        // farm". Skip the last column's divider so the rightmost cubicle
        // doesn't paint a divider into empty space.
        let is_last_col =
            desk.x + DESK_W + 2 + DESK_W >= layout.cubicle_band.x + layout.cubicle_band.width;
        if !is_last_col {
            let div_x = desk.x + DESK_W + 3;
            for dy in 0..(DESK_H + 1) {
                let px = div_x;
                let py = desk.y.saturating_sub(1) + dy;
                if px < buf_w && py < buf_h {
                    buf.put(px, py, DIVIDER);
                }
            }
        }
        if i % 2 == 0 {
            if let Some(cab) = cab_anim.and_then(|a| a.frames.first()) {
                let cab_x = desk.x.saturating_sub(cab.width + 1);
                let cab_y = desk.y;
                if cab_y + cab.height <= buf_h {
                    blit_frame(cab, cab_x, cab_y, buf);
                }
            }
        }
        if let Some(frame) = desk_anim.and_then(|a| a.frames.first()) {
            blit_frame(frame, desk.x, desk.y, buf);
        }
        if let Some(bin) = bin_anim.and_then(|a| a.frames.first()) {
            let bin_x = desk.x + DESK_W;
            let bin_y = desk.y + 4;
            if bin_x + bin.width <= buf_w && bin_y + bin.height <= buf_h {
                blit_frame(bin, bin_x, bin_y, buf);
            }
        }
        let occupant = agents
            .iter()
            .find(|a| a.desk_index == i && a.exiting_at.is_none());
        if let Some(agent) = occupant {
            if matches!(agent.state, ActivityState::Active { .. }) {
                paint_screen_glow(buf, desk.x, desk.y, now);
            }
        }
    }

    let _ = agents; // pixel pass uses `agents` above; flush passes get fresh
                    // `scene` refs.
}

/// Half-block flush — compresses two vertical pixel rows into one terminal
/// cell using the ▀ glyph. Pure ratatui write; no scene knowledge.
fn flush_buffer_to_term(f: &mut ratatui::Frame<'_>, buf: &RgbBuffer, scene_rect: Rect) {
    let term_buf = f.buffer_mut();
    let w = buf.width as usize;
    let cell_rows = (buf.height / 2) as usize;
    for cy in 0..cell_rows {
        for cx in 0..(buf.width as usize) {
            let x = scene_rect.x + cx as u16;
            let y = scene_rect.y + cy as u16;
            if x >= scene_rect.x + scene_rect.width || y >= scene_rect.y + scene_rect.height {
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

/// Labels above each character — uses `character_anchor` to follow the
/// agent along its current path, color-codes by activity, falls back to
/// disambiguating session-id suffix only when multiple agents share a label.
fn paint_label_widgets(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    scene_rect: Rect,
) {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    let mut label_counts: HashMap<&str, usize> = HashMap::new();
    for agent in &agents {
        *label_counts.entry(&*agent.label).or_insert(0) += 1;
    }
    for agent in &agents {
        let Some(anchor) = character_anchor(agent, layout, now, router, overlay) else {
            continue;
        };
        let lx = scene_rect.x + anchor.x.saturating_sub(2);
        let ly = scene_rect.y + (anchor.y / 2).saturating_sub(1);
        let needs_disambig = label_counts.get(&*agent.label).copied().unwrap_or(0) > 1
            && agent.session_id.len() >= 4;
        let raw: std::borrow::Cow<'_, str> = if needs_disambig {
            std::borrow::Cow::Owned(format!("{}·{}", agent.label, &agent.session_id[..4]))
        } else {
            std::borrow::Cow::Borrowed(&*agent.label)
        };
        let display = truncate_label(&raw, (DESK_W + 4) as usize);
        let label_color = if agent.exiting_at.is_some() {
            Color::Rgb(100, 110, 130)
        } else {
            match &agent.state {
                ActivityState::Active { .. } => Color::Rgb(140, 240, 170),
                ActivityState::Waiting { .. } => Color::Rgb(240, 200, 80),
                ActivityState::Idle => Color::Rgb(160, 160, 160),
            }
        };
        let para = Paragraph::new(Span::styled(
            display.into_owned(),
            Style::default().fg(label_color),
        ));
        if let Some(r) = clip_widget_rect(
            Rect {
                x: lx,
                y: ly,
                width: DESK_W + 4,
                height: 1,
            },
            scene_rect,
        ) {
            f.render_widget(para, r);
        }
    }
}

/// Live agent count painted as a sticky on the bulletin board sprite.
fn paint_bulletin_notice(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    layout: &Layout,
    scene_rect: Rect,
) {
    use crate::tui::layout::WallDecor;
    let Some((_, bb_pos)) = layout
        .wall_decor
        .iter()
        .find(|(k, _)| *k == WallDecor::BulletinBoard)
    else {
        return;
    };
    let cell_x = scene_rect.x + bb_pos.x;
    let cell_y = scene_rect.y + (bb_pos.y / 2).saturating_sub(1);
    let n = scene
        .agents
        .values()
        .filter(|a| a.exiting_at.is_none())
        .count();
    let label = format!("{} live", n);
    let notice = Paragraph::new(Span::styled(label, Style::default().fg(Color::Yellow)));
    if let Some(r) = clip_widget_rect(
        Rect {
            x: cell_x,
            y: cell_y,
            width: 8,
            height: 1,
        },
        scene_rect,
    ) {
        f.render_widget(notice, r);
    }
}

/// Fit a label into `budget` chars without losing the `·xxxx` session-id
/// disambiguation suffix that the reducer appends to colliding cwds.
/// Truncates from the base (left side of the `·`), not from the suffix —
/// otherwise the disambig becomes useless ("TikTok-Android·a" tells us
/// nothing the base alone wouldn't).
fn truncate_label(label: &str, budget: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if label.chars().count() <= budget {
        return Cow::Borrowed(label);
    }
    if let Some(sep_byte) = label.rfind('·') {
        let suffix = &label[sep_byte..];
        let suffix_len = suffix.chars().count();
        if suffix_len < budget {
            let base = &label[..sep_byte];
            let base_take = budget - suffix_len;
            let truncated: String = base.chars().take(base_take).collect();
            return Cow::Owned(format!("{truncated}{suffix}"));
        }
    }
    Cow::Owned(label.chars().take(budget).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_label_passes_short_labels_through() {
        assert_eq!(truncate_label("hello", 16), "hello");
    }

    #[test]
    fn truncate_label_preserves_disambig_suffix() {
        // 19 chars > 16 budget → must drop chars from the base, NOT the suffix.
        let out = truncate_label("TikTok-Android·a09a", 16);
        assert_eq!(out.chars().count(), 16);
        assert!(out.ends_with("·a09a"), "suffix lost: {out}");
        assert!(out.starts_with("TikTok"), "base over-truncated: {out}");
    }

    #[test]
    fn truncate_label_falls_back_to_plain_truncate_when_no_separator() {
        let out = truncate_label("a-very-long-project-name", 8);
        assert_eq!(out, "a-very-l");
    }
}
