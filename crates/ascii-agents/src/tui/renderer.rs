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
    base.with_override('B', Some(shirt))
        .with_override('H', Some(hair))
        .with_override('S', Some(skin))
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
fn paint_floor_and_walls(buf: &mut RgbBuffer, buf_w: u16, buf_h: u16) {
    const PLANK_H: u16 = 6;
    const TOP_WALL_H: u16 = 14;
    const BASEBOARD_H: u16 = 3;
    const WINDOW_FRAME: Rgb = Rgb(24, 24, 32);
    const WINDOW_GLASS: Rgb = Rgb(120, 160, 200);
    const WINDOW_GLASS_2: Rgb = Rgb(160, 190, 220);

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

    // Window panels every 18 px along the wall, leaving gaps for variation.
    const WINDOW_W: u16 = 10;
    const WINDOW_H: u16 = 6;
    const WINDOW_Y: u16 = 3;
    let mut x = 4u16;
    let mut idx: u32 = 0;
    while x + WINDOW_W + 2 <= buf_w {
        // Skip every 4th window slot to vary the rhythm.
        if idx % 4 != 3 {
            paint_window(buf, x, WINDOW_Y, WINDOW_W, WINDOW_H, WINDOW_FRAME, WINDOW_GLASS, WINDOW_GLASS_2);
        }
        x += WINDOW_W + 8;
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

fn paint_window(
    buf: &mut RgbBuffer,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    frame: Rgb,
    glass_a: Rgb,
    glass_b: Rgb,
) {
    // Solid frame
    for dy in 0..h {
        for dx in 0..w {
            let px = x + dx;
            let py = y + dy;
            if px >= buf.width || py >= buf.height {
                continue;
            }
            let on_edge = dx == 0 || dx == w - 1 || dy == 0 || dy == h - 1;
            // Mullion in the middle horizontally and vertically.
            let on_mullion = dx == w / 2 || dy == h / 2;
            let color = if on_edge || on_mullion {
                frame
            } else if (dx + dy) % 2 == 0 {
                glass_a
            } else {
                glass_b
            };
            buf.put(px, py, color);
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
    use crate::tui::layout::WaypointKind;

    // Waypoint furniture (the wander destinations) painted centered on each
    // waypoint position.
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
    }

    // Plants — pure decor, scattered around the lounge.
    if let Some(plant) = pack.animation("plant").and_then(|a| a.frames.first()) {
        for p in &layout.plants {
            let px = p.x.saturating_sub(plant.width / 2);
            let py = p.y.saturating_sub(plant.height / 2);
            blit_frame(plant, px, py, buf);
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

/// "Active" screen glow painted on top of the desk sprite while an agent is
/// in `ActivityState::Active`. Reads instantly from across the screen as
/// "this workstation is busy", which the typing-frame animation alone is too
/// small (8 px head) to convey.
fn paint_screen_glow(buf: &mut RgbBuffer, desk_x: u16, desk_y: u16) {
    const GLOW: Rgb = Rgb(140, 240, 170);
    for dy in 1..=2 {
        for dx in 4..=9 {
            let px = desk_x + dx;
            let py = desk_y + dy;
            if px < buf.width && py < buf.height {
                buf.put(px, py, GLOW);
            }
        }
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

        paint_floor_and_walls(buf, buf_w, buf_h);
        // Live wall clock painted after the wall (so hands sit on top of it)
        // but before wall decor — the bookshelf etc. shouldn't cover it.
        let clock_x = buf_w / 2 - 2;
        paint_clock(buf, clock_x, 1, now);
        paint_wall_decor(buf, &layout, pack);
        paint_lounge_decor(buf, &layout, pack);

        // Pass 1: rugs only. Each agent's home desk is at
        // home_desks[agent.desk_index] — NOT at the agent's BTreeMap position.
        // Iterating by agent (and looking up the desk) keeps rugs, desks,
        // characters, and labels co-located.
        for agent in &agents {
            let Some(desk) = layout.home_desks.get(agent.desk_index) else { continue };
            let rug = RUG_PALETTE[(agent.agent_id.raw() as usize / 11) % RUG_PALETTE.len()];
            paint_rug(
                buf,
                desk.x.saturating_sub(1),
                desk.y.saturating_sub(10),
                DESK_W + 2,
                DESK_H + 12,
                rug,
            );
        }

        // Pass 2: characters by pose. Painted BEFORE the desk so the desk
        // can occlude the character's lower body — from a top-down POV the
        // viewer sees head + shoulders sticking up above the desk back
        // edge, and the body is hidden behind the desk.
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
                    paint_character_at(buf, "seated", 0, seated_anchor(desk), agent, pack, false);
                }
                Pose::SeatedTyping { frame } => {
                    paint_character_at(buf, "typing", frame, seated_anchor(desk), agent, pack, false);
                }
                Pose::StandingAtDesk => {
                    let anchor = standing_at_desk_anchor(desk);
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
                                ("sitting_couch", couch_seat_anchor(wp_obj.pos))
                            }
                            crate::tui::layout::WaypointKind::Coffee => {
                                ("holding_coffee", waypoint_anchor(wp_obj.pos))
                            }
                            crate::tui::layout::WaypointKind::WaterCooler => {
                                ("standing", waypoint_anchor(wp_obj.pos))
                            }
                        };
                        let anchor = Point {
                            x: anchor_base.x.saturating_add_signed(dx),
                            y: anchor_base.y,
                        };
                        paint_character_at(buf, anim_name, 0, anchor, agent, pack, false);
                    }
                }
                Pose::AimlessAt { dest } => {
                    paint_character_at(buf, "standing", 0, waypoint_anchor(dest), agent, pack, false);
                }
                Pose::Walking { from, to, t_x1000, frame } => {
                    let pos = walking_position(from, to, t_x1000);
                    let flip = to.x < from.x;
                    paint_character_at(buf, "walking", frame, walking_anchor(pos), agent, pack, flip);
                }
            }
        }

        // Pass 3: desks (+ screen glow). Painted AFTER the character so the
        // desk occludes the character's lower body — top-down POV reads as
        // "person sitting BEHIND the desk", not "person standing on the
        // desk top". The screen glow now sits on top of everything, so it's
        // a fully visible "this workstation is active" cue.
        let desk_anim = pack.animation("desk");
        for agent in &agents {
            let Some(desk) = layout.home_desks.get(agent.desk_index) else { continue };
            if let Some(frame) = desk_anim.and_then(|a| a.frames.first()) {
                blit_frame(frame, desk.x, desk.y, buf);
            }
            if matches!(agent.state, ActivityState::Active { .. }) {
                paint_screen_glow(buf, desk.x, desk.y);
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

        // Labels above each home desk. Same indexing fix as the passes
        // above. State is conveyed visually (screen glow for Active,
        // waiting bubble for Waiting) so the label only carries identity —
        // budget the whole 16-char widget for the name, and keep the
        // session-id disambiguation suffix when the base would overflow.
        for agent in &agents {
            let Some(desk) = layout.home_desks.get(agent.desk_index) else { continue };
            let lx = scene_rect.x + desk.x;
            let ly = scene_rect.y + (desk.y / 2).saturating_sub(1);
            let display = truncate_label(&agent.label, (DESK_W + 4) as usize);
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
