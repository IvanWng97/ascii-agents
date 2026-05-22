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
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

use crate::tui::layout::{Layout, Point, DESK_W};
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
/// Warm / extroverted shirt palette — used for higher-trip-chance agents.
const SHIRT_PRESETS_WARM: &[Rgb] = &[
    Rgb(0x9c, 0x27, 0x27),  // crimson
    Rgb(0xc6, 0x6a, 0x1e),  // burnt orange
    Rgb(0xb0, 0x32, 0xa8),  // magenta
    Rgb(0xd0, 0x9c, 0x32),  // mustard
];
/// Cool / homebody shirt palette — used for lower-trip-chance agents.
const SHIRT_PRESETS_COOL: &[Rgb] = &[
    Rgb(0x2e, 0x62, 0xcf),  // royal blue
    Rgb(0x16, 0xa0, 0x6e),  // forest green
    Rgb(0x32, 0x82, 0x9b),  // teal
    Rgb(0x6c, 0x4f, 0x9e),  // violet
];
const HAIR_PRESETS: &[Rgb] = &[
    Rgb(0x2a, 0x1a, 0x0e),  // near-black
    Rgb(0x52, 0x32, 0x10),  // dark brown
    Rgb(0xc7, 0xa3, 0x4a),  // blond
    Rgb(0x7a, 0x32, 0x10),  // auburn
    Rgb(0x3a, 0x3a, 0x3a),  // dark grey
];
const SKIN_PRESETS: &[Rgb] = &[
    Rgb(0xf4, 0xc7, 0x9a),  // light peach (matches base palette S)
    Rgb(0xe0, 0xa8, 0x70),  // medium
    Rgb(0xb8, 0x80, 0x50),  // tan
    Rgb(0x8a, 0x5a, 0x36),  // deep brown
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
fn paint_floor_and_walls(buf: &mut RgbBuffer, buf_w: u16, buf_h: u16, now: SystemTime) {
    // Smaller planks (4 px tall × 10 px wide) read as tile/parquet rather
    // than the previous oversized 6×16 boards that dominated the scene.
    const PLANK_H: u16 = 4;
    const PLANK_W: u32 = 10;
    const TOP_WALL_H: u16 = 14;
    const BASEBOARD_H: u16 = 3;
    const WINDOW_FRAME: Rgb = Rgb(24, 24, 32);
    // Glass + spill scale with local hour — dark windows at night, full
    // daylight 8..18, smooth ramps at dawn/dusk.
    let look = time_of_day_look(now);

    for y in 0..buf_h {
        let band = y / PLANK_H;
        let seam_offset = (band as u32 * 7) % PLANK_W;
        for x in 0..buf_w {
            let in_seam = y % PLANK_H == PLANK_H - 1
                || ((x as u32).wrapping_add(seam_offset)) % PLANK_W == 0;
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

    // Floor-to-ceiling windows: 落地窗 — wide AND tall. Each panel spans
    // most of the wall band height and is wider than the cubicle pitch
    // so the wall reads as glass-dominated, not brick-dominated.
    const WINDOW_W: u16 = 22;
    const WINDOW_H: u16 = 12;
    const WINDOW_Y: u16 = 1;
    const WINDOW_GAP: u16 = 3;
    let mut x = 3u16;
    let mut idx: u32 = 0;
    while x + WINDOW_W + 2 <= buf_w {
        paint_floor_to_ceiling_window(
            buf,
            x,
            WINDOW_Y,
            WINDOW_W,
            WINDOW_H,
            WINDOW_FRAME,
            &look,
            idx as u16,
        );
        if look.spill_strength > 0.0 {
            paint_window_light_spill(
                buf,
                x,
                WINDOW_W,
                TOP_WALL_H,
                look.spill_strength,
                look.spill_slant,
            );
        }
        x += WINDOW_W + WINDOW_GAP;
        idx += 1;
    }

    // Wall clock — painted by paint_clock() in a separate pass so it can
    // take `now` and render real hand positions.

    // Wall trim line at the bottom of the wall band.
    let trim_y = TOP_WALL_H - 1;
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
    let unix_now = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    let h = local.hour() as f32 + local.minute() as f32 / 60.0;

    // Daylight intensity: full from 8 to 17, smooth ramp 5..8 and 17..20.
    let day = if h < 5.0 || h >= 20.0 {
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
    let twilight =
        bell(h, 6.5, 1.5).max(bell(h, 18.5, 1.5));

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
    ((a as f32) * (1.0 - t) + (b as f32) * t).round().clamp(0.0, 255.0) as u8
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
/// (1-pixel dots) light up at night.
fn paint_floor_to_ceiling_window(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    frame: Rgb,
    look: &TimeOfDayLook,
    window_idx: u16,
) {
    const BUILDING_DARK: Rgb = Rgb(20, 22, 32);
    const BUILDING_LIGHT: Rgb = Rgb(60, 65, 82);
    const LIT_WINDOW: Rgb = Rgb(252, 215, 110);
    const DARK_WINDOW: Rgb = Rgb(30, 32, 44);

    let lit_strength = look.darkness.clamp(0.0, 1.0);
    let lit_color = lerp_rgb(DARK_WINDOW, LIT_WINDOW, lit_strength);
    let building = lerp_rgb(BUILDING_LIGHT, BUILDING_DARK, look.darkness);

    // Building skyline heights per glass column. Six-pattern repeated,
    // offset by window_idx so adjacent windows don't show identical skylines.
    let skyline: &[u16] = &[3, 5, 4, 6, 3, 5, 4, 5, 3, 6, 4, 5];
    // Lit-window dot positions inside each building (dx_in_glass, dy_from_top_of_building).
    let lit_dots: &[(u16, u16)] = &[(1, 1), (3, 0), (5, 2), (7, 1), (9, 2), (2, 3), (6, 3)];

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
            let glass_h = h - 2;
            // Sky gradient — light top, deeper bottom — uses time-of-day colors.
            let sky_t = (glass_dy as f32) / (glass_h as f32 * 0.7);
            let sky_color = lerp_rgb(look.glass_b, look.glass_a, sky_t.min(1.0));

            // Building silhouette at the bottom of the glass.
            let building_h = skyline[((glass_dx + window_idx * 3) % skyline.len() as u16) as usize];
            let in_building = glass_dy >= glass_h.saturating_sub(building_h);

            if in_building {
                // Building wall (dark slab) with occasional lit window dots.
                let bldg_y = glass_dy - (glass_h - building_h);
                let is_lit = lit_dots.iter().any(|&(lx, ly)| lx == glass_dx && ly == bldg_y);
                buf.put(px, py, if is_lit { lit_color } else { building });
            } else {
                buf.put(px, py, sky_color);
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
        (1, 0, RIM), (2, 0, RIM), (3, 0, RIM),
        (0, 1, RIM), (1, 1, FACE), (2, 1, FACE), (3, 1, FACE), (4, 1, RIM),
        (0, 2, RIM), (1, 2, FACE), (2, 2, FACE), (3, 2, FACE), (4, 2, RIM),
        (0, 3, RIM), (1, 3, FACE), (2, 3, FACE), (3, 3, FACE), (4, 3, RIM),
        (1, 4, RIM), (2, 4, RIM), (3, 4, RIM),
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
    let local = chrono::DateTime::<chrono::Local>::from(
        std::time::UNIX_EPOCH + unix_now,
    );
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
    use crate::tui::layout::WaypointKind;

    // Decorative rug under each Couch waypoint — defines the seating zone,
    // adds a warm accent against the wood floor. Painted BEFORE the couch
    // sprite so the couch sits on it.
    for wp in &layout.waypoints {
        if wp.kind == WaypointKind::Couch {
            paint_lounge_rug(buf, wp.pos.x, wp.pos.y + 1, 9, 4);
        }
    }

    // Waypoint furniture (the wander destinations) painted centered on each
    // waypoint position. Coffee station also emits animated steam.
    for wp in &layout.waypoints {
        let anim_name = match wp.kind {
            WaypointKind::Couch => "couch",
            WaypointKind::Coffee => "coffee",
            WaypointKind::WaterCooler => "water_cooler",
        };
        if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
            let cx = wp.pos.x.saturating_sub(f.width / 2);
            let cy = wp.pos.y.saturating_sub(f.height / 2);
            blit_frame(f, cx, cy, buf);
        }
        if wp.kind == WaypointKind::Coffee {
            paint_coffee_steam(buf, Point { x: wp.pos.x, y: wp.pos.y.saturating_sub(4) }, now);
        }
    }

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
}

/// Wall-leaning furniture (bookshelf + whiteboard). Painted *after* the
/// wall band so it sits in front of the wall trim, leaning against it.
fn paint_wall_decor(buf: &mut RgbBuffer, layout: &Layout, pack: &Pack) {
    use crate::tui::layout::WallDecor;
    for (kind, pos) in &layout.wall_decor {
        let anim_name = match kind {
            WallDecor::Bookshelf => "bookshelf",
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
    if phase < CYCLE_MS / 2 { 0 } else { 1 }
}

fn with_breath(anchor: Point, agent_id: ascii_agents_core::AgentId, now: SystemTime) -> Point {
    Point {
        x: anchor.x,
        y: anchor.y.saturating_sub(breath_offset_y(agent_id, now)),
    }
}

/// Anchor for the seated-on-couch pose. Sits the character on the couch
/// surface (couch is ~5px tall) so the body overlaps the cushion. Sprite is
/// 8 wide → centered on the waypoint by offsetting x by 4.
fn couch_seat_anchor(wp: Point) -> Point {
    Point {
        x: wp.x.saturating_sub(4),
        y: wp.y.saturating_sub(4),
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
    Point {
        x: (from.x as i32 + dx * t / 1000) as u16,
        y: (from.y as i32 + dy * t / 1000) as u16,
    }
}

/// Paint a character at an arbitrary anchor with per-agent recolor. `flip_x`
/// mirrors the sprite horizontally — used to make walkers face the direction
/// they're moving.
fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
) {
    let base_pal = pack.palette.clone();
    let pal = agent_palette(&base_pal, agent);
    let Some(anim) = pack.animation(anim_name) else { return };
    let Some(frame) = anim.frames.get(frame_idx).or_else(|| anim.frames.first()) else { return };
    let recolored = recolor_frame(frame, &pal, &base_pal);
    let final_frame = if flip_x { recolored.mirror_horizontal() } else { recolored };
    blit_frame(&final_frame, anchor.x, anchor.y, buf);
}

/// Decorative lounge rug under the couch. Warm rust-red with a slightly
/// lighter fringe so it reads as a real textile, not just a flat tile.
fn paint_lounge_rug(buf: &mut RgbBuffer, cx: u16, cy: u16, half_w: u16, half_h: u16) {
    const RUG_BASE: Rgb = Rgb(122, 56, 48);
    const RUG_FRINGE: Rgb = Rgb(168, 92, 76);
    let min_x = cx.saturating_sub(half_w);
    let max_x = (cx + half_w).min(buf.width);
    let min_y = cy.saturating_sub(half_h);
    let max_y = (cy + half_h).min(buf.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let on_border =
                x == min_x || x + 1 == max_x || y == min_y || y + 1 == max_y;
            buf.put(x, y, if on_border { RUG_FRINGE } else { RUG_BASE });
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

/// Office chair painted BEHIND the character — a darkened version of the
/// agent's shirt color, so the character's transparent edge pixels reveal
/// the chair as a "halo" of upholstery. Brings back the per-agent color
/// identifier that the removed rug used to provide, in a more naturalistic
/// way (top-down chair back behind the sitter).
fn paint_chair_behind(buf: &mut RgbBuffer, anchor: Point, agent: &AgentSlot, pack: &Pack) {
    let pal = agent_palette(&pack.palette, agent);
    let Some(shirt) = pal.get('B').flatten() else { return };
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
fn paint_sleep_z(
    buf: &mut RgbBuffer,
    head_anchor: Point,
    now: SystemTime,
    seed: u64,
) {
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
            Span::raw(" ascii-agents — "),
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

        // Size the cubicle grid to fit each agent's actual desk_index, not
        // just the count of live agents. After SessionEnd, desk_indexes can
        // be sparse (e.g. {0,1,3,4} with 4 agents) — sizing to .len() would
        // truncate the agent at the highest index.
        let needed_desks = agents
            .iter()
            .map(|a| a.desk_index + 1)
            .max()
            .unwrap_or(0);
        let Some(layout) = Layout::compute(buf_w, buf_h, needed_desks) else {
            return;
        };

        paint_floor_and_walls(buf, buf_w, buf_h, now);

        // Artificial light pass — at night the floor dims toward navy and
        // ceiling fluorescents + the floor lamp halo paint the visible
        // bright spots. During the day the dim is near-zero and the pools
        // are subtle ambient highlights.
        let look = time_of_day_look(now);
        dim_floor_overlay(buf, 14, buf_h, look.darkness * 0.45);
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
        let lounge_y = layout.lounge_band.y + layout.lounge_band.height / 2;
        paint_ceiling_pool(buf, buf_w * 28 / 100, lounge_y, 12, 6, pool_strength);
        paint_ceiling_pool(buf, buf_w * 62 / 100, lounge_y, 12, 6, pool_strength);
        if let Some(lamp) = layout.floor_lamp {
            paint_floor_lamp_halo(buf, lamp.x, lamp.y, look.darkness * 0.55);
        }

        // Live wall clock painted after the wall (so hands sit on top of it)
        // but before wall decor — the bookshelf etc. shouldn't cover it.
        let clock_x = buf_w / 2 - 2;
        paint_clock(buf, clock_x, 1, now);
        paint_wall_decor(buf, &layout, pack);
        if let Some(door_pos) = layout.door {
            if let Some(frame) = pack.animation("door").and_then(|a| a.frames.first()) {
                blit_frame(frame, door_pos.x, door_pos.y, buf);
            }
        }
        paint_lounge_decor(buf, &layout, pack, now);

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
            let Some(desk) = layout.home_desks.get(agent.desk_index).copied() else { continue };
            let Some(p) = pose::derive(agent, now, &layout) else { continue };
            match p {
                Pose::SeatedIdle => {
                    let anchor = with_breath(seated_anchor(desk), agent.agent_id, now);
                    paint_chair_behind(buf, anchor, agent, pack);
                    paint_character_at(buf, "seated_sleeping", 0, anchor, agent, pack, false);
                    paint_sleep_z(buf, anchor, now, agent.agent_id.raw());
                }
                Pose::SeatedTyping { frame } => {
                    let anchor = with_breath(seated_anchor(desk), agent.agent_id, now);
                    paint_chair_behind(buf, anchor, agent, pack);
                    paint_character_at(buf, "typing", frame, anchor, agent, pack, false);
                }
                Pose::StandingAtDesk => {
                    let anchor = with_breath(standing_at_desk_anchor(desk), agent.agent_id, now);
                    paint_character_at(buf, "standing", 0, anchor, agent, pack, false);
                    if matches!(agent.state, ActivityState::Waiting { .. }) {
                        paint_waiting_bubble(buf, anchor);
                    }
                }
                Pose::AtWaypoint { wp, kind } => {
                    if let Some(wp_obj) = layout.waypoints.get(wp) {
                        let rank = *wp_rank.entry(wp).or_insert(0);
                        wp_rank.insert(wp, rank + 1);
                        let dx = waypoint_rank_offset_x(kind, rank);
                        let (anim_name, anchor_base) = match kind {
                            crate::tui::layout::WaypointKind::Couch => {
                                ("sitting_couch_sleeping", couch_seat_anchor(wp_obj.pos))
                            }
                            crate::tui::layout::WaypointKind::Coffee => {
                                ("holding_coffee", waypoint_anchor(wp_obj.pos))
                            }
                            crate::tui::layout::WaypointKind::WaterCooler => {
                                ("standing", waypoint_anchor(wp_obj.pos))
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
                        paint_character_at(buf, anim_name, 0, anchor, agent, pack, false);
                        if matches!(kind, crate::tui::layout::WaypointKind::Couch) {
                            paint_sleep_z(buf, anchor, now, agent.agent_id.raw());
                        }
                    }
                }
                Pose::AimlessAt { dest } => {
                    let anchor = with_breath(waypoint_anchor(dest), agent.agent_id, now);
                    paint_character_at(buf, "standing", 0, anchor, agent, pack, false);
                }
                Pose::Walking { from, to, t_x1000, frame } => {
                    let pos = walking_position(from, to, t_x1000);
                    let flip = to.x < from.x;
                    let walker_anchor = walking_anchor(pos);
                    paint_walking_dust(buf, walker_anchor, frame);
                    paint_character_at(buf, "walking", frame, walker_anchor, agent, pack, flip);
                }
            }
        }

        // Pass 2: desks (+ trash bin + screen glow). Painted AFTER the
        // character so the desk occludes the character's lower body — top-
        // down POV reads as "person sitting BEHIND the desk", not "person
        // standing on the desk top". The screen glow sits on top of
        // everything, so it's a fully visible "this workstation is active"
        // cue. Trash bin tucked next to each desk for cubicle realism.
        let desk_anim = pack.animation("desk");
        let bin_anim = pack.animation("trash_bin");
        for agent in &agents {
            let Some(desk) = layout.home_desks.get(agent.desk_index) else { continue };
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
            if matches!(agent.state, ActivityState::Active { .. }) {
                paint_screen_glow(buf, desk.x, desk.y, now);
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

        // Labels above each home desk. Disambiguate ONLY when multiple
        // visible agents share a label (e.g. two CC sessions in the same
        // project directory). Unique sessions get the bare name — no
        // noisy `·xxxx` suffix.
        let mut label_counts: HashMap<&str, usize> = HashMap::new();
        for agent in &agents {
            *label_counts.entry(agent.label.as_str()).or_insert(0) += 1;
        }
        for agent in &agents {
            let Some(desk) = layout.home_desks.get(agent.desk_index) else { continue };
            let lx = scene_rect.x + desk.x;
            let ly = scene_rect.y + (desk.y / 2).saturating_sub(1);
            let needs_disambig =
                label_counts.get(agent.label.as_str()).copied().unwrap_or(0) > 1
                    && agent.session_id.len() >= 4;
            let raw: std::borrow::Cow<'_, str> = if needs_disambig {
                std::borrow::Cow::Owned(format!(
                    "{}·{}",
                    agent.label,
                    &agent.session_id[..4]
                ))
            } else {
                std::borrow::Cow::Borrowed(agent.label.as_str())
            };
            let display = truncate_label(&raw, (DESK_W + 4) as usize);
            let para = Paragraph::new(Span::styled(
                display.into_owned(),
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
