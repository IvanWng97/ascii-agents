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
use std::time::{Duration, SystemTime};

use ascii_agents_core::sprite::blit::blit_frame;
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::walkable::OccupancyOverlay;
use ascii_agents_core::{AgentSlot, SceneState};

use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{Layout, Point, DESK_W};
use crate::tui::pathfind::Router;
use crate::tui::pose::{self, Pose};

mod anchors;
mod background;
mod drawable;
mod effects;
mod palette;

use anchors::{
    back_couch_anchor, seated_anchor, standing_at_desk_anchor, walking_anchor, walking_position,
    waypoint_anchor, waypoint_rank_offset_x, with_breath,
};
use background::{
    dim_floor_overlay, paint_ceiling_pool, paint_clock, paint_corridor_runner,
    paint_floor_and_walls, paint_floor_lamp_halo, paint_neon_panel, paint_shadow, time_of_day_look,
};
use drawable::{cat_position, paint_drawable, Drawable, DrawableKind};
use palette::{agent_palette, recolor_frame};

/// Paint a character at an arbitrary anchor with per-agent recolor. `flip_x`
/// mirrors the sprite horizontally — used to make walkers face the direction
/// they're moving. `glow_tint` should carry the tool-derived monitor color
/// when the character is at a lit screen (SeatedTyping); tints the skin
/// toward that color so the eye reads "the monitor is lighting their face."
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &'static str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
    glow_tint: Option<Rgb>,
    cache: &mut FrameCache,
) {
    let Some(anim) = pack.animation(anim_name) else {
        return;
    };
    let Some(frame) = anim.frames.get(frame_idx).or_else(|| anim.frames.first()) else {
        return;
    };
    let cached = cache.get_or_make(
        agent.agent_id,
        anim_name,
        frame_idx,
        flip_x,
        glow_tint,
        || {
            let pal = agent_palette(&pack.palette, agent, glow_tint);
            let recolored = recolor_frame(frame, &pal, &pack.palette);
            if flip_x {
                recolored.mirror_horizontal()
            } else {
                recolored
            }
        },
    );
    blit_frame(cached, anchor.x, anchor.y, buf);
}

/// Low coffee table in front of the lounge couch. Wood top with darker
/// trim along the front edge so it reads as a real piece of furniture,
/// not just a brown rectangle.
pub(super) fn paint_coffee_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::tui::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let min_x = cx.saturating_sub(w / 2);
    let max_x = (cx + w / 2 + (w & 1)).min(buf.width);
    let min_y = cy.saturating_sub(h / 2);
    let max_y = (cy + h / 2 + (h & 1)).min(buf.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let on_front = y + 1 == max_y;
            buf.put(x, y, if on_front { trim } else { top });
        }
    }
}

/// Meeting-room area rug — warm Persian-tone rectangle painted under
/// the coffee table. Border ring in a darker shade so the rug reads as
/// having a fringe/binding rather than a flat blob. Centred on `cx,cy`.
pub(super) fn paint_area_rug(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::tui::theme::Theme,
) {
    let rug_field = theme.furniture.rug_field;
    let rug_trim = theme.furniture.rug_trim;
    let rug_accent = theme.furniture.rug_accent;
    let half_w = w as i32 / 2;
    let half_h = h as i32 / 2;
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            let px = cx as i32 - half_w + dx;
            let py = cy as i32 - half_h + dy;
            if px < 0 || py < 0 || px >= buf.width as i32 || py >= buf.height as i32 {
                continue;
            }
            let on_border = dx == 0 || dx == w as i32 - 1 || dy == 0 || dy == h as i32 - 1;
            let on_inner_border = dx == 1 || dx == w as i32 - 2 || dy == 1 || dy == h as i32 - 2;
            let color = if on_border {
                rug_trim
            } else if on_inner_border {
                rug_accent
            } else {
                rug_field
            };
            buf.put(px as u16, py as u16, color);
        }
    }
}

/// Lounge side table — 7×4 wood block next to the viewing couch
/// (opposite side from the floor lamp). Bumped from 5×3 to clear the
/// skill's ~5-cell-wide subzone threshold. Carries a 3-cell magazine
/// stack on top so the silhouette reads as "side table with a book".
pub(super) fn paint_side_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::tui::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let mag = theme.furniture.magazine;
    let mag_trim = theme.furniture.magazine_trim;
    let w: i32 = 7;
    let h: i32 = 4;
    for dy in 0..h {
        for dx in 0..w {
            let px = cx as i32 - w / 2 + dx;
            let py = cy as i32 - h / 2 + dy;
            if px < 0 || py < 0 || px >= buf.width as i32 || py >= buf.height as i32 {
                continue;
            }
            let on_bottom = dy == h - 1;
            buf.put(px as u16, py as u16, if on_bottom { trim } else { top });
        }
    }
    let mag_pixels: &[((i32, i32), Rgb)] = &[
        ((-1, -1), mag),
        ((0, -1), mag),
        ((1, -1), mag),
        ((-1, 0), mag_trim),
        ((0, 0), mag_trim),
        ((1, 0), mag_trim),
    ];
    for ((dx, dy), c) in mag_pixels {
        let px = cx as i32 + dx;
        let py = cy as i32 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width && (py as u16) < buf.height {
            buf.put(px as u16, py as u16, *c);
        }
    }
}

/// Pantry bistro table — round-ish wood top (rounded corners by skipping
/// the 4 corner pixels) painted with the same warm wood palette as the
/// coffee table so they read as the same furniture family.
pub(super) fn paint_pantry_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::tui::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
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
            buf.put(px as u16, py as u16, if on_edge { trim } else { top });
        }
    }
}

pub(super) fn paint_pantry_chair(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::tui::theme::Theme,
) {
    let seat = theme.furniture.chair_seat;
    let trim = theme.furniture.chair_trim;
    let put = |buf: &mut RgbBuffer, dx: i32, dy: i32, c: Rgb| {
        let px = cx as i32 + dx;
        let py = cy as i32 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width && (py as u16) < buf.height {
            buf.put(px as u16, py as u16, c);
        }
    };
    put(buf, -1, -1, seat);
    put(buf, 0, -1, seat);
    put(buf, -1, 0, trim);
    put(buf, 0, 0, trim);
}

/// How long the elevator's open/close transition takes. Used as both
/// the opening ramp at the START of an agent's entry/exit window and
/// the closing ramp at the END. 200 ms feels snappy without being
/// abrupt — the half-open frame is visible for ~70 ms each way.
const DOOR_TRANSITION_MS: u64 = 200;

/// Compute the elevator door frame (0=closed, 1=half, 2=open) from
/// the agents currently in flight. Stateless: each agent contributes
/// a per-frame value based on how far through their entry/exit window
/// they are; we take the MAX across all agents so the door is at
/// least as open as the most-in-progress agent needs.
fn compute_door_frame_idx(agents: &[AgentSlot], now: SystemTime) -> usize {
    fn frame_for_progress(elapsed_ms: u64, total_ms: u64) -> usize {
        // 0..200ms: opening (0 → 1 → 2)
        if elapsed_ms < DOOR_TRANSITION_MS {
            if elapsed_ms < DOOR_TRANSITION_MS / 2 {
                1
            } else {
                2
            }
        } else if elapsed_ms + DOOR_TRANSITION_MS > total_ms {
            // last 200ms: closing (2 → 1 → 0)
            let remaining = total_ms.saturating_sub(elapsed_ms);
            if remaining < DOOR_TRANSITION_MS / 2 {
                0
            } else {
                1
            }
        } else {
            // middle: fully open
            2
        }
    }
    let mut max_frame: usize = 0;
    for a in agents {
        if a.exiting_at.is_none() {
            if let Ok(d) = now.duration_since(a.created_at) {
                let ms = d.as_millis() as u64;
                if ms < pose::ENTRY_ANIMATION_MS {
                    max_frame = max_frame.max(frame_for_progress(ms, pose::ENTRY_ANIMATION_MS));
                }
            }
        }
        if let Some(exit_at) = a.exiting_at {
            if let Ok(d) = now.duration_since(exit_at) {
                let ms = d.as_millis() as u64;
                // Use the same window the reducer uses to GC exiting
                // slots so the door closes right as the agent's slot
                // disappears.
                let exit_window_ms =
                    ascii_agents_core::state::reducer::EXIT_GRACE_WINDOW.as_millis() as u64;
                if ms < exit_window_ms {
                    max_frame = max_frame.max(frame_for_progress(ms, exit_window_ms));
                }
            }
        }
    }
    max_frame
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
        Pose::SeatedIdle | Pose::SeatedThinking | Pose::SeatedTyping { .. } => seated_anchor(desk),
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
    theme: &crate::tui::theme::Theme,
) {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    let buf_w = layout.buf_w;
    let buf_h = layout.buf_h;

    // Compute time-of-day once per frame and pass to every paint
    // helper that depends on it. Avoids recomputing the chrono local
    // hour for each window + ceiling pool + lamp halo.
    let look = time_of_day_look(now, theme);
    // Wall band height tracks layout.top_margin (which is buf_h/4 with
    // a floor) — leaves a 4-px buffer between wall trim and cubicles.
    let top_wall_h = layout.top_margin.saturating_sub(4);
    // The elevator door replaces the rightmost window — pass its x-range
    // so `paint_floor_and_walls` skips drawing a window that would
    // otherwise bleed through behind the elevator frame.
    let door_x_range = layout.door.map(|d| (d.x, d.x + 16));
    paint_floor_and_walls(
        buf,
        buf_w,
        buf_h,
        now,
        &look,
        top_wall_h,
        door_x_range,
        theme,
    );

    // Artificial light pass — at night the floor dims toward navy and
    // ceiling fluorescents + the floor lamp halo paint the visible
    // bright spots. During the day the dim is near-zero and the pools
    // are subtle ambient highlights. The wall-clock-based darkness
    // already handles "after hours" cleanly — an activity-based boost
    // flickers because Active flips on/off per tool call.
    dim_floor_overlay(buf, top_wall_h, buf_h, look.darkness * 0.45, theme);
    let pool_strength = 0.15 + 0.30 * look.darkness;
    for desk in &layout.home_desks {
        paint_ceiling_pool(
            buf,
            desk.x + DESK_W / 2,
            desk.y.saturating_sub(2),
            10,
            5,
            pool_strength,
            theme,
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
            theme,
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
            theme,
        );
    }
    if let Some(lamp) = layout.floor_lamp {
        paint_floor_lamp_halo(buf, lamp.x, lamp.y, look.darkness * 0.55, theme);
    }

    // Neon sign panel in the wall band — dark bg with glow border.
    // Text overlay (branding, dots, star link) is rendered by the ratatui
    // widget pass in renderer.rs::paint_wall_display.
    let neon_w = 30u16;
    let neon_h = 8u16;
    paint_neon_panel(buf, 1, 1, neon_w, neon_h, now, theme);

    // Live wall clock painted after the wall (so hands sit on top of it)
    // but before wall decor — the bookshelf etc. shouldn't cover it.
    let clock_x = buf_w / 2 - 2;
    paint_clock(buf, clock_x, 1, now, theme);
    // Corridor runner — painted over the floor but BEFORE walls/decor
    // so walls cleanly overlap it where they cross.
    if let Some(corridor) = layout.corridor {
        paint_corridor_runner(buf, corridor, theme);
    }
    // Room dividers. Stardew-style fake-3D perspective:
    //   • horizontal walls (E-W) show the wall face — 4 px tall with
    //     a light top trim (lit cap) and dark bottom trim (shadow).
    //   • vertical walls (N-S) are seen edge-on — drawn as a single
    //     1-px partition line.
    // Must match `WALL_THICK_V` / `WALL_THICK_H` in build_walkable_mask.
    const WALL_THICK_V_PX: u16 = 1;
    const WALL_THICK_H_PX: u16 = 4;
    let wall_body = theme.office.room_wall_body;
    let wall_trim_light = theme.office.room_wall_trim_light;
    let wall_trim_dark = theme.office.room_wall_trim_dark;
    for (start, end) in &layout.room_walls {
        if start.x == end.x {
            for y in start.y..=end.y.min(buf_h - 1) {
                for dx in 0..WALL_THICK_V_PX {
                    let x = start.x + dx;
                    if x < buf_w {
                        buf.put(x, y, wall_body);
                    }
                }
            }
        } else {
            for x in start.x..=end.x.min(buf_w - 1) {
                for dy in 0..WALL_THICK_H_PX {
                    let y = start.y + dy;
                    if y >= buf_h {
                        continue;
                    }
                    let color = if dy == 0 {
                        wall_trim_light
                    } else if dy == WALL_THICK_H_PX - 1 {
                        wall_trim_dark
                    } else {
                        wall_body
                    };
                    buf.put(x, y, color);
                }
            }
        }
    }

    // Meeting sofas + table, pantry table + chairs are all painted by
    // the y-sorted Drawable pass below (MeetingSofa / MeetingTable /
    // PantryTable / PantryChair variants). They used to be painted
    // here in the background pass too — leftover from before the
    // y-sort refactor; the duplicate paints were dead pixels
    // overwritten 50 lines later. Removed.
    //
    // Entry mat was also painted here (a small blue rug just south of
    // the door). The old wooden-door era used it to define the arrival
    // zone, but the elevator already defines that visually + the blue
    // rectangle looked out of place under the elevator.

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
            theme,
        );
    }
    for wp in &layout.waypoints {
        paint_shadow(buf, wp.pos.x, wp.pos.y + 2, 7, 2, shadow_strength, theme);
    }
    for (_, p) in &layout.plants {
        paint_shadow(buf, p.x, p.y + 3, 3, 1, shadow_strength, theme);
    }
    if let Some(lamp) = layout.floor_lamp {
        paint_shadow(buf, lamp.x, lamp.y + 5, 2, 1, shadow_strength, theme);
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
    let seated_agents: HashMap<usize, bool> = agents
        .iter()
        .filter(|a| a.desk_index < layout.home_desks.len() && a.exiting_at.is_none())
        .map(|a| {
            let p = pose::derive_with_routing(a, now, layout, router, overlay, history);
            let seated = matches!(p, Some(Pose::SeatedTyping { .. } | Pose::SeatedThinking));
            (a.desk_index, seated)
        })
        .collect();
    for (i, &desk) in layout.home_desks.iter().enumerate() {
        let is_last_col =
            desk.x + DESK_W + 2 + DESK_W >= layout.cubicle_band.x + layout.cubicle_band.width;
        let occupant = agents
            .iter()
            .find(|a| a.desk_index == i && a.exiting_at.is_none());
        let screen_glow = occupant
            .filter(|_| seated_agents.get(&i).copied().unwrap_or(false))
            .and_then(|a| palette::tool_glow_tint(a, &theme.tool_glow));
        let session_age_secs = occupant
            .and_then(|a| now.duration_since(a.created_at).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        drawables.push(Drawable {
            anchor_y: desk.y + 8,
            kind: DrawableKind::DeskCubicle {
                desk,
                is_last_col,
                has_cabinet: i % 2 == 0,
                screen_glow,
                session_age_secs,
            },
        });
    }

    // Meeting-room area rug — sized to span both sofas + the coffee
    // table with a small margin. Anchored at the TOP so y-sort paints
    // it before the furniture sitting on top of it.
    if let (Some(table), Some(&top_sofa), Some(&bot_sofa)) = (
        layout.meeting_table,
        layout.meeting_sofas.first(),
        layout.meeting_sofas.get(1),
    ) {
        let rug_w = 18u16;
        let rug_h = (bot_sofa.y - top_sofa.y + 8).min(layout.buf_h - table.y + 8);
        drawables.push(Drawable {
            anchor_y: table.y.saturating_sub(rug_h / 2),
            kind: DrawableKind::AreaRug {
                pos: table,
                width: rug_w,
                height: rug_h,
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
    // both centered on the waypoint position. PhoneBooth and
    // StandingDesk waypoints are visually rendered via the
    // `pod_decor` drawables below (they ARE the decor); they don't
    // get a duplicate Drawable here.
    for wp in &layout.waypoints {
        use crate::tui::layout::WaypointKind;
        match wp.kind {
            WaypointKind::Couch => {
                // Small area rug under the lounge couch — anchored
                // BEHIND (north of) the couch so the rug spans the
                // floor in front of it (south side) where someone
                // standing/walking would step. y-sort anchor at the
                // top so couch sits on top.
                drawables.push(Drawable {
                    anchor_y: wp.pos.y.saturating_sub(2),
                    kind: DrawableKind::AreaRug {
                        pos: Point {
                            x: wp.pos.x,
                            y: wp.pos.y + 3,
                        },
                        width: 18,
                        height: 7,
                    },
                });
                drawables.push(Drawable {
                    anchor_y: wp.pos.y + 3,
                    kind: DrawableKind::WaypointCouch { pos: wp.pos },
                });
                if let Some(table) = layout.lounge_side_table {
                    drawables.push(Drawable {
                        anchor_y: table.y + 1,
                        kind: DrawableKind::LoungeSideTable { pos: table },
                    });
                }
            }
            WaypointKind::Pantry => {
                let (cw, ch) = layout.pantry_counter_size;
                drawables.push(Drawable {
                    anchor_y: wp.pos.y + ch / 2,
                    kind: DrawableKind::WaypointPantry {
                        pos: wp.pos,
                        use_large: cw >= 32,
                    },
                });
            }
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {}
        }
    }

    // Pod-aisle decor (plant / whiteboard / TV / phone booth /
    // standing desk). All centered at `pos`; anchor at the bottom of
    // the sprite footprint so y-sort places them correctly against
    // walkers and characters in the aisles.
    for (kind, pos) in &layout.pod_decor {
        let (_, h) = kind.size();
        drawables.push(Drawable {
            anchor_y: pos.y + h / 2,
            kind: DrawableKind::PodDecorItem {
                kind: *kind,
                pos: *pos,
            },
        });
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

    // Elevator door (16×14, top-left anchored). Frame is computed
    // stateless from agents in their entry/exit window: door opens
    // (0→1→2) over the first DOOR_TRANSITION_MS of the agent's
    // transit, holds open (2) in the middle, then closes (2→1→0)
    // over the final DOOR_TRANSITION_MS. With multiple agents in
    // flight we take the MAX frame so the door is at least as open
    // as the most-in-progress agent needs.
    if let Some(door_pos) = layout.door {
        let frame_idx = compute_door_frame_idx(&agents, now);
        drawables.push(Drawable {
            anchor_y: door_pos.y + 14,
            kind: DrawableKind::Door {
                pos: door_pos,
                frame_idx,
            },
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
            WallDecor::MeetingScreen => 6,
        };
        drawables.push(Drawable {
            anchor_y: pos.y + h,
            kind: DrawableKind::WallDecor {
                kind: *kind,
                pos: *pos,
            },
        });
    }

    let idle_desk_indices: Vec<usize> = agents
        .iter()
        .filter(|a| {
            matches!(a.state, ActivityState::Idle)
                && a.desk_index < layout.home_desks.len()
                && a.exiting_at.is_none()
        })
        .map(|a| a.desk_index)
        .collect();
    let all_idle = agents
        .iter()
        .all(|a| matches!(a.state, ActivityState::Idle));

    if let Some((pos, flip, anim_name, frame_idx)) =
        cat_position(layout, pack, now, &idle_desk_indices, all_idle)
    {
        drawables.push(Drawable {
            anchor_y: pos.y + 3,
            kind: DrawableKind::Cat {
                pos,
                flip,
                anim_name,
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
        // room sofas then floor seats. Short 2s entry/exit walk.
        if agent.desk_index >= layout.home_desks.len() {
            const OVERFLOW_ANIM_MS: u64 = 2000;
            let entry_elapsed = now
                .duration_since(agent.created_at)
                .unwrap_or(Duration::ZERO)
                .as_millis() as u64;
            let is_entering = entry_elapsed < OVERFLOW_ANIM_MS && agent.exiting_at.is_none();
            let is_exiting = agent.exiting_at.is_some();
            if is_entering || is_exiting {
                if let Some(threshold) = layout.door_threshold {
                    let target = if agent.desk_index - layout.home_desks.len()
                        < layout.meeting_sofas.len()
                    {
                        let sofa = layout.meeting_sofas[agent.desk_index - layout.home_desks.len()];
                        Point {
                            x: sofa.x.saturating_sub(4),
                            y: sofa.y.saturating_sub(2),
                        }
                    } else {
                        let floor_idx =
                            agent.desk_index - layout.home_desks.len() - layout.meeting_sofas.len();
                        let seat = match layout.floor_seats.get(floor_idx).copied() {
                            Some(s) => s,
                            None => threshold,
                        };
                        Point {
                            x: seat.x.saturating_sub(4),
                            y: seat.y.saturating_sub(2),
                        }
                    };
                    let (from, to, t) = if is_exiting {
                        let exit_elapsed = agent
                            .exiting_at
                            .and_then(|e| now.duration_since(e).ok())
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let t = (exit_elapsed as f32 / OVERFLOW_ANIM_MS as f32).min(1.0);
                        (target, threshold, t)
                    } else {
                        let t = (entry_elapsed as f32 / OVERFLOW_ANIM_MS as f32).min(1.0);
                        (threshold, target, t)
                    };
                    let x = from.x as f32 + (to.x as f32 - from.x as f32) * t;
                    let y = from.y as f32 + (to.y as f32 - from.y as f32) * t;
                    let anchor = Point {
                        x: x as u16,
                        y: y as u16,
                    };
                    let dy = to.y as i32 - from.y as i32;
                    let dx = to.x as i32 - from.x as i32;
                    let anim_name = if dy.abs() > dx.abs() && dy < 0 {
                        "walking_back"
                    } else {
                        "walking"
                    };
                    let frame_idx = (entry_elapsed / 220) as usize % 2;
                    drawables.push(Drawable {
                        anchor_y: anchor.y + 12,
                        kind: DrawableKind::Character {
                            agent,
                            anim_name,
                            frame_idx,
                            anchor,
                            flip_x: dx < 0,
                            glow_tint: None,
                            sleep_z_seed: None,
                            waiting_bubble: false,
                            thinking_dots: false,
                            walking_dust_frame: Some(frame_idx),
                        },
                    });
                    continue;
                }
            }
            let overflow_idx = agent.desk_index - layout.home_desks.len();
            let sofa_count = layout.meeting_sofas.len();
            // 400 ms screen-pulse period for active overflow agents.
            // Out of phase with typing (140 ms) and walking (220 ms) so
            // adjacent agents don't visibly strobe together.
            const OVERFLOW_PULSE_MS: u64 = 400;
            let pulse_frame: usize = now
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| (d.as_millis() as u64 / OVERFLOW_PULSE_MS) as usize % 2)
                .unwrap_or(0);
            let is_active = matches!(agent.state, ActivityState::Active { .. });

            if overflow_idx < sofa_count {
                let sofa = layout.meeting_sofas[overflow_idx];
                let is_mirrored_sofa = overflow_idx > 0;
                let (anim_name, base_anchor_y, sprite_h, frame_idx) = if is_mirrored_sofa {
                    // Back-facing sofa — viewer sees the agent's back, no
                    // laptop to show. Keep the static back_couch sprite.
                    ("back_couch", sofa.y.saturating_sub(7), 9u16, 0)
                } else if is_active {
                    (
                        "working_couch",
                        sofa.y.saturating_sub(2),
                        12u16,
                        pulse_frame,
                    )
                } else {
                    ("sitting_couch_sleeping", sofa.y.saturating_sub(2), 12u16, 0)
                };
                let anchor = with_breath(
                    Point {
                        x: sofa.x.saturating_sub(4),
                        y: base_anchor_y,
                    },
                    agent.agent_id,
                    now,
                );
                let sleeping = !is_active && !is_mirrored_sofa;
                drawables.push(Drawable {
                    anchor_y: anchor.y + sprite_h,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name,
                        frame_idx,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: if sleeping {
                            Some(agent.agent_id.raw())
                        } else {
                            None
                        },
                        waiting_bubble: false,
                        thinking_dots: false,
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
            let (anim_name, frame_idx) = if is_active {
                ("working_floor", pulse_frame)
            } else {
                ("seated_floor_sleeping", 0)
            };
            drawables.push(Drawable {
                anchor_y: anchor.y + 12,
                kind: DrawableKind::Character {
                    agent,
                    anim_name,
                    frame_idx,
                    anchor,
                    flip_x: false,
                    glow_tint: None,
                    sleep_z_seed: None,
                    waiting_bubble: false,
                    thinking_dots: false,
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
                let sleep_variant = if agent.agent_id.raw() % 2 == 0 {
                    "seated_sleeping"
                } else {
                    "seated_sleeping_alt"
                };
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: sleep_variant,
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: Some(agent.agent_id.raw()),
                        waiting_bubble: false,
                        thinking_dots: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::SeatedThinking => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, now);
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "seated",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: Some(theme.tool_glow.default),
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: true,
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
                        glow_tint: palette::tool_glow_tint(agent, &theme.tool_glow),
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: false,
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
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: is_waiting,
                        thinking_dots: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    use crate::tui::layout::WaypointKind;
                    let (anim_name, anchor_base, sprite_h) = match kind {
                        WaypointKind::Couch => ("back_couch", back_couch_anchor(wp_obj.pos), 9u16),
                        WaypointKind::Pantry => {
                            ("holding_coffee", waypoint_anchor(wp_obj.pos), 12u16)
                        }
                        // PhoneBooth + StandingDesk → agent just stands at the
                        // decor. waypoint_anchor positions them directly above
                        // the decor centre (sprite footprint sits just north
                        // of the decor's centre, head visible above).
                        WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {
                            ("standing", waypoint_anchor(wp_obj.pos), 12u16)
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
                            glow_tint: None,
                            sleep_z_seed: None,
                            waiting_bubble: false,
                            thinking_dots: false,
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
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: false,
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
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: false,
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
        paint_drawable(d, buf, pack, cache, now, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ascii_agents_core::sprite::{Frame, Palette};
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
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: 0,
            tool_call_count: 0,
            active_ms: 0,
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
        let a = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        let b = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        assert_eq!(a.get('B'), b.get('B'));
        assert_eq!(a.get('H'), b.get('H'));
        assert_eq!(a.get('S'), b.get('S'));
    }

    #[test]
    fn agent_palette_overrides_only_bhs_keys() {
        let id = ascii_agents_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let p = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        // X is not a recolor target — must pass through unchanged.
        assert_eq!(p.get('X'), Some(Some(Rgb(99, 99, 99))));
        // B/H/S must be replaced — the base RGBs (10/20/30 etc.) are
        // unlikely to be in any preset, so they should differ.
        assert_ne!(p.get('B'), Some(Some(Rgb(10, 20, 30))));
        assert_ne!(p.get('H'), Some(Some(Rgb(40, 50, 60))));
        assert_ne!(p.get('S'), Some(Some(Rgb(70, 80, 90))));
    }

    #[test]
    fn agent_palette_glow_tint_shifts_skin_toward_given_color() {
        let id = ascii_agents_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let slot = make_slot(id, ActivityState::Idle);
        let unlit = agent_palette(&base, &slot, None);
        let green_glow = agent_palette(&base, &slot, Some(Rgb(140, 240, 170)));
        let blue_glow = agent_palette(&base, &slot, Some(Rgb(100, 160, 255)));
        // Shirt / hair / pants are unaffected by glow.
        assert_eq!(unlit.get('B'), green_glow.get('B'));
        assert_eq!(unlit.get('H'), green_glow.get('H'));
        assert_eq!(unlit.get('P'), green_glow.get('P'));
        // Green glow pushes skin's green channel up.
        let (Some(Some(Rgb(_, ug, _))), Some(Some(Rgb(_, gg, _)))) =
            (unlit.get('S'), green_glow.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            gg > ug,
            "green glow should push skin green (lit={gg}, unlit={ug})"
        );
        // Blue glow pushes skin's blue channel up.
        let (Some(Some(Rgb(_, _, ub))), Some(Some(Rgb(_, _, bb)))) =
            (unlit.get('S'), blue_glow.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            bb > ub,
            "blue glow should push skin blue (lit={bb}, unlit={ub})"
        );
    }

    #[test]
    fn tool_glow_tint_maps_known_tools() {
        use ascii_agents_core::source::Activity;
        let id = ascii_agents_core::AgentId::from_transcript_path("/t.jsonl");
        let edit_slot = make_slot(
            id,
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: None,
                detail: Some(Arc::from("Edit src/main.rs")),
            },
        );
        let bash_slot = make_slot(
            id,
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: None,
                detail: Some(Arc::from("Bash: ls")),
            },
        );
        let idle_slot = make_slot(id, ActivityState::Idle);
        let glow = &crate::tui::theme::NORMAL.tool_glow;
        let edit_tint = palette::tool_glow_tint(&edit_slot, glow);
        let bash_tint = palette::tool_glow_tint(&bash_slot, glow);
        let idle_tint = palette::tool_glow_tint(&idle_slot, glow);
        assert!(edit_tint.is_some(), "Edit should produce glow");
        assert!(bash_tint.is_some(), "Bash should produce glow");
        assert_eq!(idle_tint, None, "Idle should produce no glow");
        // Edit and Bash should be different colors.
        assert_ne!(edit_tint, bash_tint, "Edit and Bash should differ");
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

    // --- compute_door_frame_idx -------------------------------------------

    fn entry_slot(created_at_ms_ago: u64, now: SystemTime) -> AgentSlot {
        let id = ascii_agents_core::AgentId::from_transcript_path("/door.jsonl");
        let mut s = make_slot(id, ActivityState::Idle);
        s.created_at = now - std::time::Duration::from_millis(created_at_ms_ago);
        s
    }

    fn exit_slot(exit_ms_ago: u64, now: SystemTime) -> AgentSlot {
        let id = ascii_agents_core::AgentId::from_transcript_path("/exit.jsonl");
        let mut s = make_slot(id, ActivityState::Idle);
        s.created_at = now - std::time::Duration::from_secs(300);
        s.exiting_at = Some(now - std::time::Duration::from_millis(exit_ms_ago));
        s
    }

    #[test]
    fn door_frame_closed_when_no_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(compute_door_frame_idx(&[], now), 0);
    }

    #[test]
    fn door_frame_just_spawned_is_half_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 50 ms into the 200 ms opening ramp — first half = frame 1.
        let slot = entry_slot(50, now);
        assert_eq!(compute_door_frame_idx(&[slot], now), 1);
    }

    #[test]
    fn door_frame_after_opening_ramp_is_fully_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 150 ms (still inside opening ramp but past midpoint) → frame 2.
        let s1 = entry_slot(150, now);
        assert_eq!(compute_door_frame_idx(&[s1], now), 2);
        // 2 s into the 4 s window → fully open.
        let s2 = entry_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[s2], now), 2);
    }

    #[test]
    fn door_frame_closing_then_closed_at_end_of_entry() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 150 ms left in the entry window → closing ramp first half → frame 1.
        let mid_close = entry_slot(pose::ENTRY_ANIMATION_MS - 150, now);
        assert_eq!(compute_door_frame_idx(&[mid_close], now), 1);
        // 50 ms left → closing ramp final half → frame 0 (closed).
        let near_end = entry_slot(pose::ENTRY_ANIMATION_MS - 50, now);
        assert_eq!(compute_door_frame_idx(&[near_end], now), 0);
    }

    #[test]
    fn door_frame_expired_entry_contributes_nothing() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Older than the 4 s entry window → no contribution.
        let old = entry_slot(pose::ENTRY_ANIMATION_MS + 1, now);
        assert_eq!(compute_door_frame_idx(&[old], now), 0);
    }

    #[test]
    fn door_frame_exit_window_uses_4500ms_total() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 2 s into a 4.5 s exit window → mid-flight → fully open.
        let exiting = exit_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[exiting], now), 2);
    }

    #[test]
    fn door_frame_takes_max_across_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let opening = entry_slot(50, now); // frame 1
        let open = entry_slot(2_000, now); // frame 2
        assert_eq!(compute_door_frame_idx(&[opening, open], now), 2);
    }

    #[test]
    fn weather_state_covers_all_variants() {
        let mut seen = std::collections::HashSet::new();
        let base = SystemTime::UNIX_EPOCH;
        for cycle in 0..100u64 {
            let now = base + std::time::Duration::from_secs(cycle * 600);
            seen.insert(std::mem::discriminant(&background::weather_state(now)));
        }
        assert!(
            seen.len() >= 5,
            "expected at least 5 weather variants in 100 cycles, got {}",
            seen.len()
        );
    }

    #[test]
    fn weather_state_deterministic() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
        let a = background::weather_state(now);
        let b = background::weather_state(now);
        assert_eq!(a, b);
    }

    #[test]
    fn weather_state_changes_across_cycles() {
        let mut states = Vec::new();
        let base = SystemTime::UNIX_EPOCH;
        for cycle in 0..20u64 {
            states.push(background::weather_state(
                base + std::time::Duration::from_secs(cycle * 600),
            ));
        }
        let unique: std::collections::HashSet<_> =
            states.iter().map(std::mem::discriminant).collect();
        assert!(unique.len() >= 2, "weather should vary across cycles");
    }

    #[test]
    fn sunset_strength_varies_across_day() {
        let mut strengths = Vec::new();
        let base = SystemTime::UNIX_EPOCH;
        for hour in 0..24u64 {
            strengths.push(background::sunset_strength(
                base + std::time::Duration::from_secs(hour * 3600),
            ));
        }
        let has_zero = strengths.iter().any(|s| *s < 0.05);
        let has_nonzero = strengths.iter().any(|s| *s > 0.1);
        assert!(has_zero, "sunset should be ~0 at some hours");
        assert!(has_nonzero, "sunset should be >0 at dawn/dusk hours");
    }
}
