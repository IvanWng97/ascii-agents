//! Live debug layer toggled by `w`: the walkable mask (red), each furniture's
//! allowed approach sides (green) / on-furniture seat cells (magenta), and the
//! live A* route polylines of walking agents (cyan), composited over the
//! finished scene before the half-block flush.
//!
//! This is a debug VIEW over the SAME data the renderer + router already use —
//! `layout.is_walkable` (the one walkable mask), `furniture_def(_).approach`
//! (the one approach model, rotated by facing), and each agent's frozen
//! `walk_path` — never a second source. Off by default; transient (not config).

use std::collections::HashMap;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::{AgentId, SceneState};

use super::palette::blend;
use crate::tui::layout::{desk_walk_anchor, furniture_def, Layout, Point, WaypointKind};
use crate::tui::motion::MotionState;

const BLOCKED: Rgb = Rgb(220, 60, 60); // walkable mask — blocked ground
const APPROACH: Rgb = Rgb(70, 220, 110); // allowed approach cell (off a side)
const SEAT: Rgb = Rgb(235, 80, 215); // occupies_pos cell (sprite sits ON it)
const ROUTE: Rgb = Rgb(70, 210, 235); // live A* route polyline

/// N, S, E, W unit dirs (same axes `ApproachSides::allows` expects).
const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (1, 0), (-1, 0)];

pub(super) fn paint(
    buf: &mut RgbBuffer,
    layout: &Layout,
    scene: &SceneState,
    motion: &HashMap<AgentId, MotionState>,
) {
    paint_mask(buf, layout);
    paint_approach(buf, layout);
    paint_routes(buf, scene, motion);
}

fn tint(buf: &mut RgbBuffer, x: i32, y: i32, c: Rgb, t: f32) {
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as u16, y as u16);
    if x >= buf.width || y >= buf.height {
        return;
    }
    let bg = buf.get(x, y);
    buf.put(
        x,
        y,
        Rgb(
            blend(bg.0, c.0, t),
            blend(bg.1, c.1, t),
            blend(bg.2, c.2, t),
        ),
    );
}

/// 3×3 marker centred on `(cx, cy)`.
fn blob(buf: &mut RgbBuffer, cx: i32, cy: i32, c: Rgb, t: f32) {
    for dy in -1..=1 {
        for dx in -1..=1 {
            tint(buf, cx + dx, cy + dy, c, t);
        }
    }
}

fn paint_mask(buf: &mut RgbBuffer, layout: &Layout) {
    for y in 0..layout.buf_h {
        for x in 0..layout.buf_w {
            if !layout.is_walkable(x, y) {
                tint(buf, x as i32, y as i32, BLOCKED, 0.38);
            }
        }
    }
}

fn paint_approach(buf: &mut RgbBuffer, layout: &Layout) {
    for wp in &layout.waypoints {
        let def = furniture_def(wp.kind.furniture());
        if def.occupies_pos {
            // Seat / stand-on cell — the sprite sits ON `pos`, no side approach.
            blob(buf, wp.pos.x as i32, wp.pos.y as i32, SEAT, 0.7);
            continue;
        }
        // Obstacle: mark the cell just off each ALLOWED side (facing-rotated).
        // Pantry's footprint is runtime-sized.
        let fp = if wp.kind == WaypointKind::Pantry {
            Some(layout.pantry_counter_size)
        } else {
            def.footprint
        };
        let Some((w, h)) = fp else {
            continue;
        };
        let (hx, hy) = ((w / 2) as i32, (h / 2) as i32);
        for (dx, dy) in DIRS {
            if def.approach.allows(wp.facing, (dx, dy)) {
                blob(
                    buf,
                    wp.pos.x as i32 + dx * (hx + 1),
                    wp.pos.y as i32 + dy * (hy + 1),
                    APPROACH,
                    0.7,
                );
            }
        }
    }
    // Home desks: the agent's fixed stand/seat cell is a bespoke anchor (not a
    // side-probe), so mark `desk_walk_anchor` directly.
    for desk in &layout.home_desks {
        let a = desk_walk_anchor(*desk);
        blob(buf, a.x as i32, a.y as i32, APPROACH, 0.7);
    }
}

fn paint_routes(buf: &mut RgbBuffer, scene: &SceneState, motion: &HashMap<AgentId, MotionState>) {
    for agent in scene.agents.values() {
        let Some(ms) = motion.get(&agent.agent_id) else {
            continue;
        };
        let Some(wp) = &ms.walk_path else {
            continue;
        };
        for seg in wp.path.windows(2) {
            line(buf, seg[0], seg[1], ROUTE);
        }
    }
}

/// Integer Bresenham line between two pixel points.
fn line(buf: &mut RgbBuffer, a: Point, b: Point, c: Rgb) {
    let (mut x0, mut y0) = (a.x as i32, a.y as i32);
    let (x1, y1) = (b.x as i32, b.y as i32);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        tint(buf, x0, y0, c, 0.8);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}
