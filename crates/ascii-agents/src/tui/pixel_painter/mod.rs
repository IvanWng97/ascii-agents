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
    dim_floor_overlay, paint_ceiling_pool, paint_clock, paint_corridor_runner, paint_entry_mat,
    paint_floor_and_walls, paint_floor_lamp_halo, paint_shadow, time_of_day_look,
};
use drawable::{cat_position, paint_drawable, Drawable, DrawableKind};
use palette::{agent_palette, recolor_frame};

/// Paint a character at an arbitrary anchor with per-agent recolor. `flip_x`
/// mirrors the sprite horizontally — used to make walkers face the direction
/// they're moving. `face_lit` should be set only when the character is at a
/// lit monitor (currently SeatedTyping); tints the skin toward the monitor
/// glow color so the eye reads "the monitor is lighting their face".
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &'static str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
    face_lit: bool,
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
        face_lit,
        || {
            let pal = agent_palette(&pack.palette, agent, face_lit);
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
pub(super) fn paint_coffee_table(buf: &mut RgbBuffer, cx: u16, cy: u16, w: u16, h: u16) {
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
pub(super) fn paint_pantry_table(buf: &mut RgbBuffer, cx: u16, cy: u16) {
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
pub(super) fn paint_pantry_chair(buf: &mut RgbBuffer, cx: u16, cy: u16) {
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
                        face_lit: false,
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
                    face_lit: false,
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
                        face_lit: false,
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
                        // SeatedTyping == Active state at desk → screen
                        // glow is painting on the monitor → reflect that
                        // glow on the agent's skin too.
                        face_lit: true,
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
                        face_lit: false,
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
                            face_lit: false,
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
                        face_lit: false,
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
                        face_lit: false,
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
            exiting_at: None,
            pending_idle_at: None,
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
        let a = agent_palette(&base, &make_slot(id, ActivityState::Idle), false);
        let b = agent_palette(&base, &make_slot(id, ActivityState::Idle), false);
        assert_eq!(a.get('B'), b.get('B'));
        assert_eq!(a.get('H'), b.get('H'));
        assert_eq!(a.get('S'), b.get('S'));
    }

    #[test]
    fn agent_palette_overrides_only_bhs_keys() {
        let id = ascii_agents_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let p = agent_palette(&base, &make_slot(id, ActivityState::Idle), false);
        // X is not a recolor target — must pass through unchanged.
        assert_eq!(p.get('X'), Some(Some(Rgb(99, 99, 99))));
        // B/H/S must be replaced — the base RGBs (10/20/30 etc.) are
        // unlikely to be in any preset, so they should differ.
        assert_ne!(p.get('B'), Some(Some(Rgb(10, 20, 30))));
        assert_ne!(p.get('H'), Some(Some(Rgb(40, 50, 60))));
        assert_ne!(p.get('S'), Some(Some(Rgb(70, 80, 90))));
    }

    #[test]
    fn agent_palette_face_lit_tints_skin_toward_glow() {
        // Skin only tints toward green when `face_lit=true` — the
        // SeatedTyping (Active at desk) pose sets this so the monitor
        // light reads on the face. All other poses pass false, so
        // wandering / idle agents stay naturally skin-toned.
        let id = ascii_agents_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let slot = make_slot(id, ActivityState::Idle);
        let unlit = agent_palette(&base, &slot, false);
        let lit = agent_palette(&base, &slot, true);
        // Shirt / hair / pants are state-independent of face_lit.
        assert_eq!(unlit.get('B'), lit.get('B'));
        assert_eq!(unlit.get('H'), lit.get('H'));
        assert_eq!(unlit.get('P'), lit.get('P'));
        // Skin differs: lit tints toward green-ish GLOW_TINT(140,240,170).
        let (Some(Some(Rgb(_, ug, _))), Some(Some(Rgb(_, lg, _)))) = (unlit.get('S'), lit.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            lg > ug,
            "face-lit skin green channel should exceed unlit (lit={lg}, unlit={ug})"
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
