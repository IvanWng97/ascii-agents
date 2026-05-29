//! Ambient pass — non-character, non-furniture effects painted between
//! the background and the y-sorted drawables: sun spot on wall, dust
//! motes in window spill, ceiling halos above active monitors.
//!
//! Each subroutine is independently togglable and self-contained. New
//! ambient effects go here, not in `background/` or `drawable.rs`.

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::tui::layout::Layout;
use crate::tui::pixel_painter::background::{sun_on_wall, WallSide};
use crate::tui::pixel_painter::palette::blend;
use crate::tui::pixel_painter::PixelCtx;
use crate::tui::theme::Theme;

pub(super) fn paint_ambient(ctx: &mut PixelCtx<'_>) {
    paint_sun_spot(ctx.buf, ctx.theme, ctx.layout, ctx.now);
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
