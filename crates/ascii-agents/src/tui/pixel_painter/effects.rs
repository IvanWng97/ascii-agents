//! Per-character / per-desk overlay effects: screen glow, sleep z,
//! coffee steam, walker dust, waiting bubble.
//!
//! These all paint relative to an anchor (character feet / desk corner /
//! pantry counter) and read state from the agent + clock. They're called
//! from the drawable dispatch so they ride along with their parent in
//! z-order, not as a global foreground pass.

use std::time::SystemTime;

use ascii_agents_core::sprite::{Rgb, RgbBuffer};

use super::palette::blend;
use crate::tui::layout::Point;

/// "Active" screen glow painted on top of the desk sprite while an agent is
/// in `ActivityState::Active`. Covers the full monitor footprint (rows 0-3,
/// cols 3-10 of desk.sprite — frame + screen + stand silhouette) so the
/// glow is at least 2 terminal cells tall after half-block compression.
/// Adds a moving scanline (one extra-bright column that cycles across the
/// screen) so the monitor reads as actually displaying scrolling content.
pub(super) fn paint_screen_glow(buf: &mut RgbBuffer, desk_x: u16, desk_y: u16, now: SystemTime) {
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

/// Animated `z` rising above a sleeping character's head. Cycles ~2.4s
/// (rise 12 px then disappear). Per-agent phase offset so a row of
/// sleepers doesn't pulse in lockstep.
pub(super) fn paint_sleep_z(buf: &mut RgbBuffer, head_anchor: Point, now: SystemTime, seed: u64) {
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
pub(super) fn paint_coffee_steam(buf: &mut RgbBuffer, base: Point, now: SystemTime) {
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
pub(super) fn paint_walking_dust(buf: &mut RgbBuffer, walker_anchor: Point, frame_idx: usize) {
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

/// Small "..." speech bubble painted above a Waiting character (typically
/// a permission prompt). Yellow dots on a dark pill.
pub(super) fn paint_waiting_bubble(buf: &mut RgbBuffer, anchor: Point) {
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
