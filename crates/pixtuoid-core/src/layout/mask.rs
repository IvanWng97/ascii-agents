//! Walkable-mask construction. Stamps every obstacle (walls, desks,
//! sofas, plants, decor) into a `WalkableMask` so A* knows where
//! characters can route. The padding constant `OBSTACLE_PAD_PX` adds a
//! clearance band around each obstacle so walkers don't scrape along
//! edges.

use super::{PodDecor, Point, WallDecor, Waypoint, WaypointKind, DESK_H, DESK_W, OBSTACLE_PAD_PX};
use crate::walkable::WalkableMask;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_walkable_mask(
    buf_w: u16,
    buf_h: u16,
    top_margin: u16,
    door: Option<Point>,
    home_desks: &[Point],
    meeting_sofas: &[Point],
    meeting_tables: &[Point],
    pantry_table: Option<Point>,
    pantry_chairs: &[Point],
    waypoints: &[Waypoint],
    plants: &[(super::PlantKind, Point)],
    floor_lamp: Option<Point>,
    lounge_side_table: Option<Point>,
    wall_decor: &[(WallDecor, Point)],
    pod_decor: &[(PodDecor, Point)],
    room_walls: &[(Point, Point)],
    pantry_counter_size: (u16, u16),
) -> WalkableMask {
    let mut mask = WalkableMask::new_open(buf_w, buf_h);

    mask.mark_blocked(0, 0, buf_w, top_margin, 0);
    if let Some(d) = door {
        let cut_x = d.x.saturating_sub(2);
        let cut_h = top_margin.saturating_add(OBSTACLE_PAD_PX);
        mask.mark_walkable(cut_x, 0, 8, cut_h);
    }

    let baseboard_top = buf_h.saturating_sub(3);
    mask.mark_blocked(0, baseboard_top, buf_w, 3, 0);

    // Interior walls. Stardew-style fake-3D perspective:
    //   • horizontal walls (E-W) show their FACE — 4 px tall so the
    //     wall reads as having mass when viewed from the north room.
    //   • vertical walls (N-S) are seen EDGE-ON — 1 px thin partition.
    // Render thicknesses must stay in sync; see `WALL_THICK_*_PX` in
    // the renderer.
    const WALL_THICK_V: u16 = 1;
    const WALL_THICK_H: u16 = 4;
    for (start, end) in room_walls {
        if start.x == end.x {
            mask.mark_blocked(
                start.x,
                start.y.min(end.y),
                WALL_THICK_V,
                start.y.abs_diff(end.y) + 1,
                OBSTACLE_PAD_PX,
            );
        } else {
            mask.mark_blocked(
                start.x.min(end.x),
                start.y,
                start.x.abs_diff(end.x) + 1,
                WALL_THICK_H,
                OBSTACLE_PAD_PX,
            );
        }
    }

    for desk in home_desks {
        // Block ONLY the desk surface — not the 8-px-above seated-character
        // zone. In a top-down 3/4 view a walker passing "behind" a desk row
        // is fine: the seated character paints in Pass 1, the walker also
        // paints in Pass 1 (occasional sprite overlap is acceptable), and
        // the desk paints in Pass 2 on top of both. Routes become much
        // shorter — walkers can cut diagonally between desk rows instead
        // of weaving around each one.
        mask.mark_blocked(desk.x, desk.y, DESK_W + 2, DESK_H, OBSTACLE_PAD_PX);
    }

    for sofa in meeting_sofas {
        // The sofa sprite is 20px wide. The width arg is 16 ON PURPOSE, not a
        // stale 16px sprite size: `16 + 2·OBSTACLE_PAD = 20` reproduces the
        // exact 20px X footprint while the pad provides the 7→11px VERTICAL
        // approach clearance (sit access between sofa and table). The narrowest
        // meeting room (~26px wide) can't fit horizontal clearance — a literal
        // `20, pad` block is 24px wide and disconnects the room (the
        // walkable_mask_is_fully_connected test catches it). Top-down rule:
        // characters walk right up to the sofa's sides, clear above/below.
        mask.mark_blocked(
            sofa.x.saturating_sub(8),
            sofa.y.saturating_sub(3),
            16,
            7,
            OBSTACLE_PAD_PX,
        );
    }

    for t in meeting_tables {
        mask.mark_blocked(
            t.x.saturating_sub(6),
            t.y.saturating_sub(3),
            12,
            6,
            OBSTACLE_PAD_PX,
        );
    }

    if let Some(t) = pantry_table {
        mask.mark_blocked(
            t.x.saturating_sub(4),
            t.y.saturating_sub(2),
            8,
            5,
            OBSTACLE_PAD_PX,
        );
    }
    for chair in pantry_chairs {
        mask.mark_blocked(
            chair.x.saturating_sub(2),
            chair.y.saturating_sub(2),
            3,
            3,
            1,
        );
    }

    for wp in waypoints {
        let (w, h) = match wp.kind {
            // The couch is 3 seat-waypoints (dx ∈ {-6,0,+6}). An 8px-wide
            // footprint per seat overlaps its neighbours (8 > 6 spacing) so the
            // union is the exact 20px sofa ground footprint (couch_x-10..+10) —
            // ground footprint only, never the visual width over-blocked.
            WaypointKind::Couch => (8, 7),
            WaypointKind::Pantry => pantry_counter_size,
            WaypointKind::PhoneBooth => (6, 12),
            WaypointKind::StandingDesk => (8, 8),
            WaypointKind::VendingMachine => (4, 6),
            WaypointKind::Printer => (5, 4),
            // Meeting slots sit/stand on the sofa/table furniture, which is
            // already stamped above by the meeting_sofas / meeting_tables
            // loops. The slot adds no new obstacle.
            WaypointKind::MeetingSofa | WaypointKind::MeetingStand => continue,
        };
        // Pad=1 (not OBSTACLE_PAD_PX=2) — waypoint furniture paints in
        // Pass 1.5 (after characters) so a visitor's body is occluded
        // by the sprite. We don't need extra clearance around the
        // sprite footprint; the render order handles overlap correctly.
        mask.mark_blocked(
            wp.pos.x.saturating_sub(w / 2),
            wp.pos.y.saturating_sub(h / 2),
            w,
            h,
            1,
        );
    }

    for (_, p) in plants {
        mask.mark_blocked(p.x.saturating_sub(3), p.y.saturating_sub(3), 6, 6, 1);
    }

    if let Some(lamp) = floor_lamp {
        mask.mark_blocked(lamp.x.saturating_sub(2), lamp.y.saturating_sub(3), 4, 6, 1);
    }

    if let Some(t) = lounge_side_table {
        // 7×4 footprint centred on `t`; pad=1 since it's small and
        // sits in the wide open lounge floor with plenty of clearance.
        mask.mark_blocked(t.x.saturating_sub(3), t.y.saturating_sub(2), 7, 4, 1);
    }

    for (kind, pos) in wall_decor {
        if matches!(kind, WallDecor::Whiteboard) {
            mask.mark_blocked(pos.x, pos.y, 14, 11, OBSTACLE_PAD_PX);
        }
    }

    // Pod-aisle decor is centred at `pos`. All variants are obstacles.
    // PhoneBooth + StandingDesk are also waypoints — those entries
    // appear above in `waypoints` and double-block the same area;
    // mark_blocked is idempotent. Use pad=1 (not OBSTACLE_PAD_PX=2)
    // because aisles are tight (14×16) and an extra pixel of pad on
    // each side disconnects the routing grid through the aisle.
    for (kind, pos) in pod_decor {
        let (w, h) = kind.size();
        mask.mark_blocked(
            pos.x.saturating_sub(w / 2),
            pos.y.saturating_sub(h / 2),
            w,
            h,
            1,
        );
    }

    mask
}
