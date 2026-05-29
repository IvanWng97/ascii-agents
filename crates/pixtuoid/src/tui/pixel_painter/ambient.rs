//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: sun spot on wall, dust
//! motes in window spill, ceiling halos above active monitors.
//!
//! Each subroutine is independently togglable and self-contained. New
//! ambient effects go here, not in `background/` or `drawable.rs`.

use std::time::{Duration, SystemTime};

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::tui::layout::Layout;
use crate::tui::pixel_painter::background::{sun_on_wall, window_spill_columns, WallSide};
use crate::tui::pixel_painter::palette::blend;
use crate::tui::pixel_painter::PixelCtx;
use crate::tui::theme::Theme;

pub(super) struct SunbeamColumn {
    pub x: u16,
    pub top_y: u16,
    pub depth: u16,
}

const MOTES_PER_COLUMN: usize = 3;

/// Deterministic per `(floor_seed, particle_id, now)`. Returns up to
/// `MOTES_PER_COLUMN` positions inside the column: sine drift in x,
/// slow fall in y, alpha fades in/out at the top/bottom 15% bands so
/// motes don't pop on/off at the spill boundary.
pub(super) fn dust_mote_positions(
    floor_seed: u64,
    now: SystemTime,
    col: &SunbeamColumn,
) -> Vec<(u16, u16, f32)> {
    let t_ms = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;
    let mut out = Vec::with_capacity(MOTES_PER_COLUMN);
    for i in 0..MOTES_PER_COLUMN {
        // Mix seed + particle id with a Fibonacci-hash constant so each
        // mote in the column has independent phase / speed without
        // visible regularity across columns.
        let seed = floor_seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(i as u64);
        let phase = (seed % 6283) as f32 / 1000.0;
        let speed_y = 0.6 + ((seed >> 12) & 0x3) as f32 * 0.2;
        let speed_x = 0.4 + ((seed >> 14) & 0x3) as f32 * 0.15;
        let cycle = col.depth as f32;
        let y_offset = ((t_ms as f32 / 1000.0) * speed_y + ((seed >> 4) & 0xFF) as f32) % cycle;
        let y = col.top_y + y_offset as u16;
        let sx = (phase + (t_ms as f32 / 1000.0) * speed_x).sin();
        let x = (col.x as f32 + sx * 2.5).round() as u16;
        let norm = y_offset / cycle.max(1.0);
        let alpha = if norm < 0.15 {
            norm / 0.15
        } else if norm > 0.85 {
            (1.0 - norm) / 0.15
        } else {
            1.0
        };
        out.push((x, y, alpha));
    }
    out
}

pub(super) fn paint_ambient(ctx: &mut PixelCtx<'_>) {
    paint_sun_spot(ctx.buf, ctx.theme, ctx.layout, ctx.now);
    paint_dust_motes(
        ctx.buf,
        ctx.theme,
        ctx.layout,
        ctx.floor.floor_seed,
        ctx.now,
    );
}

/// Drift 1-pixel warm specks through each window's sunbeam spill column.
/// Only paints when `sun_on_wall(now)` reports the sun is visible —
/// otherwise there's no sunbeam for motes to ride. Cheap: 3 motes per
/// column × ~6-8 columns × 1 px each.
pub(super) fn paint_dust_motes(
    buf: &mut RgbBuffer,
    theme: &Theme,
    layout: &Layout,
    floor_seed: u64,
    now: SystemTime,
) {
    if sun_on_wall(now).is_none() {
        return;
    }
    let warm = theme.lighting.sun_spill;
    for col in window_spill_columns(layout) {
        for (x, y, alpha) in dust_mote_positions(floor_seed, now, &col) {
            if x >= buf.width || y >= buf.height {
                continue;
            }
            let cur = buf.get(x, y);
            let strength = alpha * 0.7;
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

pub(super) fn paint_sun_spot(buf: &mut RgbBuffer, theme: &Theme, layout: &Layout, now: SystemTime) {
    let Some(spot) = sun_on_wall(now) else {
        return;
    };
    // South wall is the window wall — paint_window_light_spill already
    // conveys midday sun via warm spill on the floor under the glass.
    // Painting on the glass itself would ghost-glow over the skyline.
    if matches!(spot.wall, WallSide::South) {
        return;
    }
    let warm = theme.lighting.sun_spill;
    // Blend warm toward white as the sun climbs (warmth → 0 at noon).
    let cool = 1.0 - spot.warmth;
    let color = Rgb(
        blend(warm.0, 255, cool * 0.6),
        blend(warm.1, 255, cool * 0.6),
        blend(warm.2, 255, cool * 0.6),
    );

    let base_w = 8u16;
    let base_h = 3u16;
    let w = ((base_w as f32) * spot.intensity).round() as u16;
    let h = ((base_h as f32) * spot.intensity).round() as u16;
    let w = w.max(4);
    let h = h.max(2);

    // The top wall band is the visible window wall; East/West sun spots
    // project onto the outer 1-px column at the left/right edge of that band.
    let wall_band_h = layout.top_margin.saturating_sub(4);
    if wall_band_h == 0 {
        return;
    }

    let (rx, ry) = match spot.wall {
        WallSide::East => {
            let along_px = (wall_band_h.saturating_sub(h)) as f32 * spot.along.min(1.0);
            let cx = layout.buf_w.saturating_sub(w);
            (cx, along_px as u16)
        }
        WallSide::West => {
            let along_px = (wall_band_h.saturating_sub(h)) as f32 * spot.along.min(1.0);
            (0u16, along_px as u16)
        }
        WallSide::South => unreachable!("guarded above"),
    };

    let tint_strength = 0.35 * spot.intensity;
    let max_x = (rx + w).min(buf.width);
    let max_y = (ry + h).min(buf.height);
    for y in ry..max_y {
        for x in rx..max_x {
            // Quadratic radial falloff so the spot reads round, not boxy.
            let nx = (x as f32 - (rx as f32 + w as f32 / 2.0)) / (w as f32 / 2.0).max(1.0);
            let ny = (y as f32 - (ry as f32 + h as f32 / 2.0)) / (h as f32 / 2.0).max(1.0);
            let r2 = nx * nx + ny * ny;
            if r2 > 1.0 {
                continue;
            }
            let t = (1.0 - r2) * tint_strength;
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, color.0, t),
                    blend(cur.1, color.1, t),
                    blend(cur.2, color.2, t),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dust_mote_positions_deterministic_per_seed() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(12 * 3600 + 5);
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 12,
        };
        let a = dust_mote_positions(42, now, &col);
        let b = dust_mote_positions(42, now, &col);
        assert_eq!(a, b, "same seed + time → same positions");
        assert_eq!(a.len(), MOTES_PER_COLUMN);
    }

    #[test]
    fn dust_motes_drift_over_time() {
        let now1 = SystemTime::UNIX_EPOCH + Duration::from_secs(12 * 3600);
        let now2 = now1 + Duration::from_millis(500);
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 12,
        };
        let a = dust_mote_positions(7, now1, &col);
        let b = dust_mote_positions(7, now2, &col);
        assert_ne!(a, b, "positions should advance over time");
    }

    #[test]
    fn dust_motes_alpha_fades_at_edges() {
        let col = SunbeamColumn {
            x: 100,
            top_y: 12,
            depth: 20,
        };
        let mut saw_partial = false;
        'outer: for ms in 0..5000u64 {
            let now = SystemTime::UNIX_EPOCH + Duration::from_millis(ms * 50);
            for (_, _, alpha) in dust_mote_positions(123, now, &col) {
                if alpha < 0.5 {
                    saw_partial = true;
                    break 'outer;
                }
            }
        }
        assert!(
            saw_partial,
            "expected at least one frame where a mote is in its fade band"
        );
    }
}
