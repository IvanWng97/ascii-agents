//! Y-sorted drawable enum (painter's algorithm).
//!
//! Top-down depth: every mid-ground entity carries an `anchor_y` = the
//! y-pixel row where it touches the floor (front-facing bottom edge for
//! items with thickness). Drawables sort ascending by `anchor_y` and
//! paint in order. Larger `anchor_y` = closer to camera = paints last
//! (on top). Solves the classic "character standing south of a desk
//! should appear in front of it" problem without per-pair special cases.
//!
//! What stays OUTSIDE the sort:
//!   - Background (floor / walls / lighting / corridor / room walls /
//!     entry mat / clock / shadows). All depth-independent.
//!   - Per-character attached effects (chair-behind, sleep_z,
//!     waiting_bubble, walking dust, coffee steam, screen glow)
//!     paint AS PART of their parent `Drawable` — they ride along with
//!     the entity in z-order, not as a global foreground pass.

use std::time::SystemTime;

use ascii_agents_core::sprite::blit::blit_frame;
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::AgentSlot;

use super::effects::{
    paint_coffee_steam, paint_screen_glow, paint_sleep_z, paint_waiting_bubble, paint_walking_dust,
};
use super::{paint_character_at, paint_coffee_table, paint_pantry_chair, paint_pantry_table};
use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{Layout, Point, DESK_H, DESK_W};

pub(super) struct Drawable<'a> {
    pub(super) anchor_y: u16,
    pub(super) kind: DrawableKind<'a>,
}

pub(super) enum DrawableKind<'a> {
    /// Whole cubicle as one z-unit: divider + filing cabinet (every
    /// other desk) + desk sprite + trash bin + screen-glow if the
    /// occupant is Active. Bundled so the cubicle paints atomically at
    /// the desk's bottom-edge row.
    DeskCubicle {
        desk: Point,
        is_last_col: bool,
        has_cabinet: bool,
        occupant_active: bool,
    },
    Character {
        agent: &'a AgentSlot,
        anim_name: &'static str,
        frame_idx: usize,
        anchor: Point,
        flip_x: bool,
        /// True when the character is at a lit monitor (currently only
        /// the SeatedTyping pose). Skin tints toward green to read as
        /// "monitor light on the face". Off for every other pose.
        face_lit: bool,
        sleep_z_seed: Option<u64>,
        waiting_bubble: bool,
        walking_dust_frame: Option<usize>,
    },
    /// Lounge couch (mirror_vertical'd — back at bottom, seat at top).
    WaypointCouch {
        pos: Point,
    },
    /// Pantry counter (with coffee steam attached so steam rides above
    /// the counter in z-order).
    WaypointPantry {
        pos: Point,
    },
    MeetingSofa {
        pos: Point,
        mirrored: bool,
    },
    MeetingTable {
        pos: Point,
    },
    PantryTable {
        pos: Point,
    },
    PantryChair {
        pos: Point,
    },
    Plant {
        kind: crate::tui::layout::PlantKind,
        pos: Point,
    },
    FloorLamp {
        pos: Point,
    },
    Door {
        pos: Point,
    },
    WallDecor {
        kind: crate::tui::layout::WallDecor,
        pos: Point,
    },
    Cat {
        pos: Point,
        flip: bool,
        frame_idx: usize,
    },
}

/// Returns the cat's current position + flip + frame_idx, or `None` if
/// the corridor isn't available. Pulled out of `paint_wandering_cat` so
/// the y-sort can place the cat with everything else.
pub(super) fn cat_position(
    layout: &Layout,
    pack: &Pack,
    now: SystemTime,
) -> Option<(Point, bool, usize)> {
    let anim = pack.animation("cat_walk")?;
    if anim.frames.is_empty() {
        return None;
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
    let corridor = layout.corridor?;
    let left_x = corridor.x + corridor.width * 8 / 100;
    let right_x = corridor.x + corridor.width * 92 / 100;
    let cx = left_x + ((right_x - left_x) as f32 * t) as u16;
    let cy = corridor.y + corridor.height / 2;
    let frame_idx = (elapsed_ms / 220) as usize % anim.frames.len();
    Some((Point { x: cx, y: cy }, flip, frame_idx))
}

/// Dispatch one Drawable's paint. Effects attached to characters paint
/// inline so they ride along with the character in z-order.
pub(super) fn paint_drawable(
    d: &Drawable<'_>,
    buf: &mut RgbBuffer,
    pack: &Pack,
    cache: &mut FrameCache,
    now: SystemTime,
) {
    match &d.kind {
        DrawableKind::DeskCubicle {
            desk,
            is_last_col,
            has_cabinet,
            occupant_active,
        } => {
            const DIVIDER: Rgb = Rgb(72, 82, 104);
            if !is_last_col {
                let div_x = desk.x + DESK_W + 3;
                for dy in 0..(DESK_H + 1) {
                    let py = desk.y.saturating_sub(1) + dy;
                    if div_x < buf.width && py < buf.height {
                        buf.put(div_x, py, DIVIDER);
                    }
                }
            }
            if *has_cabinet {
                if let Some(cab) = pack
                    .animation("filing_cabinet")
                    .and_then(|a| a.frames.first())
                {
                    let cab_x = desk.x.saturating_sub(cab.width + 1);
                    let cab_y = desk.y;
                    if cab_y + cab.height <= buf.height {
                        blit_frame(cab, cab_x, cab_y, buf);
                    }
                }
            }
            if let Some(frame) = pack.animation("desk").and_then(|a| a.frames.first()) {
                blit_frame(frame, desk.x, desk.y, buf);
            }
            if let Some(bin) = pack.animation("trash_bin").and_then(|a| a.frames.first()) {
                let bin_x = desk.x + DESK_W;
                let bin_y = desk.y + 4;
                if bin_x + bin.width <= buf.width && bin_y + bin.height <= buf.height {
                    blit_frame(bin, bin_x, bin_y, buf);
                }
            }
            if *occupant_active {
                paint_screen_glow(buf, desk.x, desk.y, now);
            }
        }
        DrawableKind::Character {
            agent,
            anim_name,
            frame_idx,
            anchor,
            flip_x,
            face_lit,
            sleep_z_seed,
            waiting_bubble,
            walking_dust_frame,
        } => {
            if let Some(dust_frame) = walking_dust_frame {
                paint_walking_dust(buf, *anchor, *dust_frame);
            }
            paint_character_at(
                buf, anim_name, *frame_idx, *anchor, agent, pack, *flip_x, *face_lit, cache,
            );
            if let Some(seed) = sleep_z_seed {
                paint_sleep_z(buf, *anchor, now, *seed);
            }
            if *waiting_bubble {
                paint_waiting_bubble(buf, *anchor);
            }
        }
        DrawableKind::WaypointCouch { pos } => {
            if let Some(f) = pack.animation("couch").and_then(|a| a.frames.first()) {
                let cx = pos.x.saturating_sub(f.width / 2);
                let cy = pos.y.saturating_sub(f.height / 2);
                let flipped = f.mirror_vertical();
                blit_frame(&flipped, cx, cy, buf);
            }
        }
        DrawableKind::WaypointPantry { pos } => {
            if let Some(f) = pack.animation("pantry").and_then(|a| a.frames.first()) {
                let cx = pos.x.saturating_sub(f.width / 2);
                let cy = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, cx, cy, buf);
            }
            paint_coffee_steam(
                buf,
                Point {
                    x: pos.x + 4,
                    y: pos.y.saturating_sub(2),
                },
                now,
            );
        }
        DrawableKind::MeetingSofa { pos, mirrored } => {
            if let Some(f) = pack.animation("couch").and_then(|a| a.frames.first()) {
                let sx = pos.x.saturating_sub(f.width / 2);
                let sy = pos.y.saturating_sub(f.height / 2);
                if *mirrored {
                    let flipped = f.mirror_vertical();
                    blit_frame(&flipped, sx, sy, buf);
                } else {
                    blit_frame(f, sx, sy, buf);
                }
            }
        }
        DrawableKind::MeetingTable { pos } => {
            paint_coffee_table(buf, pos.x, pos.y, 11, 5);
        }
        DrawableKind::PantryTable { pos } => {
            paint_pantry_table(buf, pos.x, pos.y);
        }
        DrawableKind::PantryChair { pos } => {
            paint_pantry_chair(buf, pos.x, pos.y);
        }
        DrawableKind::Plant { kind, pos } => {
            use crate::tui::layout::PlantKind;
            let anim_name = match kind {
                PlantKind::Ficus => "plant",
                PlantKind::Tall => "plant_tall",
                PlantKind::Flower => "plant_flower",
                PlantKind::Succulent => "plant_succulent",
            };
            if let Some(f) = pack.animation(anim_name).and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width / 2);
                let py = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::FloorLamp { pos } => {
            if let Some(f) = pack.animation("floor_lamp").and_then(|a| a.frames.first()) {
                let px = pos.x.saturating_sub(f.width / 2);
                let py = pos.y.saturating_sub(f.height / 2);
                blit_frame(f, px, py, buf);
            }
        }
        DrawableKind::Door { pos } => {
            if let Some(f) = pack.animation("door").and_then(|a| a.frames.first()) {
                blit_frame(f, pos.x, pos.y, buf);
            }
        }
        DrawableKind::WallDecor { kind, pos } => {
            use crate::tui::layout::WallDecor;
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
        DrawableKind::Cat {
            pos,
            flip,
            frame_idx,
        } => {
            let Some(anim) = pack.animation("cat_walk") else {
                return;
            };
            let Some(frame) = anim.frames.get(*frame_idx) else {
                return;
            };
            let final_frame = if *flip {
                frame.mirror_horizontal()
            } else {
                frame.clone()
            };
            let px = pos.x.saturating_sub(final_frame.width / 2);
            let py = pos.y.saturating_sub(final_frame.height / 2);
            blit_frame(&final_frame, px, py, buf);
        }
    }
}
