//! Background pass — depth-independent floor, walls, windows, skyline,
//! clock, corridor runner, entry mat, time-of-day overlays, ceiling
//! light pools, lamp halo, floor shadows, and weather effects.
//!
//! Everything here paints BEFORE the y-sorted entity pass. Helpers are
//! `pub(super)` so the orchestrator (`pixel_painter/mod.rs`) can call
//! them in the order it wants.

mod lighting;
mod time_of_day;

// Re-export everything the parent pixel_painter/mod.rs imports.
pub(super) use lighting::{
    paint_ceiling_pool, paint_clock, paint_corridor_runner, paint_floor_lamp_halo,
    paint_neon_panel, paint_shadow,
};
pub(super) use time_of_day::{
    dim_floor_overlay, sunset_strength, time_of_day_look, weather_state, TimeOfDayLook, Weather,
};

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use super::palette::{blend, lerp_rgb};

use crate::tui::theme::Theme;

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_floor_and_walls(
    buf: &mut RgbBuffer,
    buf_w: u16,
    buf_h: u16,
    now: SystemTime,
    look: &TimeOfDayLook,
    top_wall_h: u16,
    skip_window_x_range: Option<(u16, u16)>,
    theme: &Theme,
    altitude: f32,
) {
    let window_frame = theme.surface.window_frame;
    let carpet_base = theme.surface.carpet_base;
    let carpet_light = theme.surface.carpet_light;
    let carpet_dark = theme.surface.carpet_dark;
    let wall = theme.surface.wall;
    let wall_trim_color = theme.surface.wall_trim;

    for y in 0..buf_h {
        for x in 0..buf_w {
            let hash = (x as u32)
                .wrapping_mul(73)
                .wrapping_add((y as u32).wrapping_mul(151))
                ^ ((x as u32).wrapping_mul(11) ^ (y as u32).wrapping_mul(37));
            let color = match hash % 17 {
                0 | 1 => carpet_light,
                2 | 3 => carpet_dark,
                _ => carpet_base,
            };
            buf.put(x, y, color);
        }
    }
    for y in 0..top_wall_h.min(buf_h) {
        for x in 0..buf_w {
            buf.put(x, y, wall);
        }
    }

    // Floor-to-ceiling windows: 落地窗 — height grows with the wall band so
    // taller terminals get dramatic floor-to-ceiling glass. Width stays
    // fixed (mullion every 22 px) so the skyline detail reads consistently.
    const WINDOW_W: u16 = 22;
    const WINDOW_GAP: u16 = 3;
    let window_y: u16 = 1;
    let window_h: u16 = top_wall_h.saturating_sub(2).max(8);
    let weather = weather_state(now);
    let mut x = 3u16;
    let mut idx: u32 = 0;
    while x + WINDOW_W + 2 <= buf_w {
        // Skip any window whose x-range overlaps the elevator door —
        // the elevator sits in the wall and would otherwise show the
        // window's glass + skyline behind its frame.
        let overlaps_door =
            skip_window_x_range.is_some_and(|(dx0, dx1)| x < dx1 && x + WINDOW_W > dx0);
        if !overlaps_door {
            paint_floor_to_ceiling_window(
                buf,
                x,
                window_y,
                WINDOW_W,
                window_h,
                window_frame,
                look,
                idx as u16,
                now,
                theme,
                weather,
                altitude,
            );
            if look.spill_strength > 0.0 {
                paint_window_light_spill(
                    buf,
                    x,
                    WINDOW_W,
                    top_wall_h,
                    look.spill_strength,
                    look.spill_slant,
                    theme,
                );
            }
        }
        x += WINDOW_W + WINDOW_GAP;
        idx += 1;
    }

    // Wall trim line at the bottom of the wall band.
    let trim_y = top_wall_h.saturating_sub(1);
    if trim_y < buf_h {
        for x in 0..buf_w {
            buf.put(x, trim_y, wall_trim_color);
        }
    }
}

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
    let cycle_ms = 6000 + (dot_seed % 8000);
    let phase = now_ms / cycle_ms;
    let hash = dot_seed
        .wrapping_add(phase)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    (hash % 10) < 7
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
    theme: &Theme,
) {
    let warm = theme.lighting.sun_spill;
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
                    blend(cur.0, warm.0, strength),
                    blend(cur.1, warm.1, strength),
                    blend(cur.2, warm.2, strength),
                ),
            );
        }
    }
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
    theme: &Theme,
    weather: Weather,
    altitude: f32,
) {
    let building_dark = theme.office.building_dark;
    let building_light = theme.office.building_light;
    let cw = theme.office.city_lit_windows;
    let dark_window = theme.office.city_dark_window;

    let lit_strength = look.darkness.max(0.5).clamp(0.0, 1.0);
    let lit_colors: [Rgb; 3] = [
        lerp_rgb(dark_window, cw[0], lit_strength),
        lerp_rgb(dark_window, cw[1], lit_strength),
        lerp_rgb(dark_window, cw[2], lit_strength),
    ];
    let building = lerp_rgb(building_light, building_dark, look.darkness);

    // Skyline silhouette as a 0..15 PATTERN; the actual pixel height is
    // computed per-window so the skyline auto-scales with the glass
    // height. On a 12-px-tall window the buildings are 3..7 px, on a
    // 50-px-tall window they fill 12..24 px — same visual proportion.
    const SKYLINE_PATTERN: &[u8] = &[8, 14, 11, 15, 6, 13, 9, 12, 7, 15, 10, 13];
    const PATTERN_MAX: u16 = 15;
    let glass_h = h.saturating_sub(2);
    let alt_shrink = (glass_h as f32 * 0.3 * altitude) as u16;
    let min_bh = (glass_h / 5).saturating_sub(alt_shrink).max(2);
    let max_bh = (glass_h * 50 / 100)
        .saturating_sub(alt_shrink)
        .max(min_bh + 3);
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
                    let dot_color = match (glass_dx.wrapping_add(bldg_y)) % 5 {
                        0 => lit_colors[1],
                        1 => lit_colors[2],
                        _ => lit_colors[0],
                    };
                    buf.put(px, py, dot_color);
                } else {
                    buf.put(px, py, building);
                }
            } else {
                buf.put(px, py, sky_row[glass_dy as usize]);
            }
        }
    }

    let elapsed_ms = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    match weather {
        Weather::Rain => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for streak in 0..4u64 {
                let seed = window_idx as u64 * 7 + streak;
                let sx = (seed.wrapping_mul(0x9e37_79b9) % gw as u64) as u16;
                let speed = 60 + (seed.wrapping_mul(0x4f6c_dd1d) % 50);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let len = 3 + (seed % 2) as u16;
                let px = glass_x0 + sx;
                for dy in 0..len {
                    let py = glass_y0 + ((phase as u16 + dy) % gh);
                    if px < buf.width && py < buf.height {
                        let alpha = 0.35 - (dy as f32 / len as f32) * 0.15;
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            Rgb(
                                blend(cur.0, 210, alpha),
                                blend(cur.1, 220, alpha),
                                blend(cur.2, 240, alpha),
                            ),
                        );
                    }
                }
            }
        }
        Weather::Storm => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for streak in 0..6u64 {
                let seed = window_idx as u64 * 7 + streak;
                let sx = (seed.wrapping_mul(0x9e37_79b9) % gw as u64) as u16;
                let speed = 40 + (seed.wrapping_mul(0x4f6c_dd1d) % 40);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let len = 4 + (seed % 3) as u16;
                let px = glass_x0 + sx;
                for dy in 0..len {
                    let py = glass_y0 + ((phase as u16 + dy) % gh);
                    if px < buf.width && py < buf.height {
                        let alpha = 0.6 - (dy as f32 / len as f32) * 0.3;
                        buf.put(
                            px,
                            py,
                            Rgb(
                                blend(buf.get(px, py).0, 210, alpha),
                                blend(buf.get(px, py).1, 220, alpha),
                                blend(buf.get(px, py).2, 245, alpha),
                            ),
                        );
                    }
                }
            }
            let flash_phase = elapsed_ms % 6000;
            if flash_phase < 50 {
                for dy in 1..h.saturating_sub(1) {
                    for dx in 1..w.saturating_sub(1) {
                        let px = x + dx;
                        let py = y + dy;
                        if px < buf.width && py < buf.height {
                            let cur = buf.get(px, py);
                            buf.put(
                                px,
                                py,
                                Rgb(
                                    blend(cur.0, 255, 0.35),
                                    blend(cur.1, 255, 0.35),
                                    blend(cur.2, 255, 0.35),
                                ),
                            );
                        }
                    }
                }
            }
        }
        Weather::Snow => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for flake in 0..3u64 {
                let seed = window_idx as u64 * 11 + flake;
                let sx = (seed.wrapping_mul(0x517c_c1b7) % gw as u64) as u16;
                let speed = 150 + (seed.wrapping_mul(0x4f6c_dd1d) % 100);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let wiggle = if (elapsed_ms / 400 + seed.wrapping_mul(0x9e37)) % 2 == 0 {
                    0
                } else {
                    1
                };
                let px = glass_x0 + (sx + wiggle) % gw;
                let py = glass_y0 + phase as u16;
                if px < buf.width && py < buf.height {
                    buf.put(px, py, Rgb(240, 240, 250));
                }
            }
        }
        Weather::Fog => {
            for dy in 1..h.saturating_sub(1) {
                for dx in 1..w.saturating_sub(1) {
                    let px = x + dx;
                    let py = y + dy;
                    if px < buf.width && py < buf.height {
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            Rgb(
                                blend(cur.0, 160, 0.25),
                                blend(cur.1, 165, 0.25),
                                blend(cur.2, 175, 0.25),
                            ),
                        );
                    }
                }
            }
        }
        Weather::Overcast => {
            for dy in 1..h.saturating_sub(1) {
                for dx in 1..w.saturating_sub(1) {
                    let px = x + dx;
                    let py = y + dy;
                    if px < buf.width && py < buf.height {
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            Rgb(
                                blend(cur.0, 100, 0.2),
                                blend(cur.1, 105, 0.2),
                                blend(cur.2, 110, 0.2),
                            ),
                        );
                    }
                }
            }
        }
        Weather::Windy => {
            let glass_x0 = x + 1;
            let glass_y0 = y + 1;
            let gw = w.saturating_sub(2);
            let gh = h.saturating_sub(2);
            for streak in 0..5u64 {
                let seed = window_idx as u64 * 7 + streak;
                let sx = (seed.wrapping_mul(0x9e37_79b9) % gw as u64) as u16;
                let speed = 50 + (seed.wrapping_mul(0x4f6c_dd1d) % 40);
                let offset = seed.wrapping_mul(0x85eb_ca6b) % (gh as u64).max(1);
                let phase = (elapsed_ms / speed + offset) % gh as u64;
                let len = 3 + (seed % 2) as u16;
                for dy in 0..len {
                    let drift = dy / 2;
                    let px = glass_x0 + (sx + drift) % gw;
                    let py = glass_y0 + ((phase as u16 + dy) % gh);
                    if px < buf.width && py < buf.height {
                        let alpha = 0.35 - (dy as f32 / len as f32) * 0.15;
                        let cur = buf.get(px, py);
                        buf.put(
                            px,
                            py,
                            Rgb(
                                blend(cur.0, 210, alpha),
                                blend(cur.1, 220, alpha),
                                blend(cur.2, 240, alpha),
                            ),
                        );
                    }
                }
            }
        }
        Weather::Clear => {}
    }

    let raw_sunset = sunset_strength(now);
    let twilight_now = {
        use chrono::Timelike;
        let unix_now = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
        let hf = local.hour() as f32 + local.minute() as f32 / 60.0;
        super::palette::bell(hf, 6.5, 1.5).max(super::palette::bell(hf, 18.5, 1.5))
    };
    let sunset = (raw_sunset * (1.0 - twilight_now * 0.8)).max(0.0);
    if sunset > 0.05 {
        let min_building_h = (glass_h / 5).max(3);
        for dy in 1..h.saturating_sub(1) {
            let glass_dy = dy.saturating_sub(1);
            if glass_dy >= glass_h.saturating_sub(min_building_h) {
                continue;
            }
            for dx in 1..w.saturating_sub(1) {
                let px = x + dx;
                let py = y + dy;
                if px < buf.width && py < buf.height {
                    let cur = buf.get(px, py);
                    let s = sunset * 0.35;
                    buf.put(
                        px,
                        py,
                        Rgb(
                            blend(cur.0, 255, s * 0.4),
                            blend(cur.1, 160, s * 0.25),
                            blend(cur.2, 60, s * 0.1),
                        ),
                    );
                }
            }
        }
    }
}
