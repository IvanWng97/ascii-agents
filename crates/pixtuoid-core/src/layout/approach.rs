//! Per-furniture approach geometry: where does an agent stand when it
//! visits a waypoint?
//!
//! The waypoint's `pos` is the furniture's geometric CENTER, which for
//! obstacle furniture (pantry counter, vending machine, …) is a blocked
//! cell — an agent can't stand on it. [`stand_point`] resolves the actual
//! stand cell: the walkable cell just off the footprint, on the side
//! nearest the agent's home desk. Walls naturally constrain it (a machine
//! flush against a wall only exposes its open side), so a counter is
//! approachable from any open side ("360°") while a recessed appliance is
//! approached from the front — matching the real world.
//!
//! Pure geometry over [`WalkableMask`] — no A*, no terminal deps — so the
//! stateless `core::pose::idle_pose` and the stateful `tui::motion` walk
//! destinations stay in lockstep with the render anchor (all three call
//! this with the same `origin = home desk`).

use super::decor::WaypointKind;
use super::Point;
use crate::walkable::WalkableMask;

/// First clear pixel beyond a footprint half-extent. `mask.rs` stamps
/// waypoint furniture with `pad = 1`, so `half + 2` is the first
/// guaranteed-unblocked pixel; we then scan a few more out for robustness.
const STAND_CLEARANCE: u16 = 2;
/// How far past `half + STAND_CLEARANCE` to keep probing for a walkable
/// cell before giving up on a side.
const STAND_SCAN: i32 = 4;

/// Ground-footprint `(w, h)` the walkable mask stamps for a waypoint, or
/// `None` for slots that add no obstacle (meeting sofa/stand sit on the
/// sofa/table furniture, already stamped elsewhere). **Single source of
/// truth** — `mask::build_walkable_mask` and [`stand_point`] both read it,
/// so the footprint can't drift between the two.
pub(super) fn obstacle_footprint(
    kind: WaypointKind,
    pantry_counter_size: (u16, u16),
) -> Option<(u16, u16)> {
    Some(match kind {
        // 3 seat-waypoints (dx ∈ {-6,0,+6}); 8px each overlaps to the exact
        // 20px sofa ground footprint. Ground footprint only (top-down rule).
        WaypointKind::Couch => (8, 7),
        WaypointKind::Pantry => pantry_counter_size,
        WaypointKind::PhoneBooth => (6, 12),
        WaypointKind::StandingDesk => (8, 8),
        WaypointKind::VendingMachine => (4, 6),
        WaypointKind::Printer => (5, 4),
        WaypointKind::MeetingSofa | WaypointKind::MeetingStand => return None,
    })
}

/// Footprint half-extents `(hx, hy)` for stand-cell resolution, or `None` for
/// seat furniture (couch and meeting slots) whose `pos` is the seat cell the
/// sprite sits ON — those pass through `pos` unchanged, no stand resolution.
/// This `None` set is a superset of `obstacle_footprint`'s: the couch HAS an
/// obstacle footprint yet is still treated as a seat here.
fn half_extents(kind: WaypointKind, pantry_counter_size: (u16, u16)) -> Option<(u16, u16)> {
    if matches!(
        kind,
        WaypointKind::Couch | WaypointKind::MeetingSofa | WaypointKind::MeetingStand
    ) {
        return None;
    }
    obstacle_footprint(kind, pantry_counter_size).map(|(w, h)| (w / 2, h / 2))
}

/// The walkable cell where an agent should stand to use the furniture of
/// `kind` centered at `pos`, given the agent's `origin` (home desk). Among
/// the four sides, returns the first walkable cell on the side nearest
/// `origin`. Falls back to `pos` for seat furniture or a degenerate mask
/// with no walkable neighbour (no worse than targeting the center directly,
/// which the router then snaps).
pub fn stand_point(
    kind: WaypointKind,
    pos: Point,
    pantry_counter_size: (u16, u16),
    mask: &WalkableMask,
    origin: Point,
) -> Point {
    let Some((hx, hy)) = half_extents(kind, pantry_counter_size) else {
        return pos;
    };

    // N, S, W, E unit axes.
    const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
    let mut best: Option<(u64, Point)> = None;
    for (dx, dy) in DIRS {
        let half = if dx != 0 { hx } else { hy } as i32;
        for step in 0..=STAND_SCAN {
            let dist = half + STAND_CLEARANCE as i32 + step;
            let cx = pos.x as i32 + dx * dist;
            let cy = pos.y as i32 + dy * dist;
            if cx < 0 || cy < 0 {
                break;
            }
            let (cx, cy) = (cx as u16, cy as u16);
            if mask.is_walkable(cx, cy) {
                let ex = cx as i64 - origin.x as i64;
                let ey = cy as i64 - origin.y as i64;
                let d2 = (ex * ex + ey * ey) as u64;
                if best.map_or(true, |(bd, _)| d2 < bd) {
                    best = Some((d2, Point { x: cx, y: cy }));
                }
                break; // first walkable cell on this side wins
            }
        }
    }
    best.map(|(_, p)| p).unwrap_or(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an open mask with a centred obstacle stamped exactly like
    /// `mask.rs` does for a waypoint (pad = 1).
    fn mask_with_obstacle(w: u16, h: u16, pos: Point, fw: u16, fh: u16) -> WalkableMask {
        let mut m = WalkableMask::new_open(w, h);
        m.mark_blocked(pos.x - fw / 2, pos.y - fh / 2, fw, fh, 1);
        m
    }

    #[test]
    fn pantry_stand_is_walkable_and_off_center() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 32, 10);
        // Origin to the north → expect a north-side stand cell.
        let s = stand_point(
            WaypointKind::Pantry,
            pos,
            (32, 10),
            &m,
            Point { x: 50, y: 8 },
        );
        assert!(m.is_walkable(s.x, s.y), "stand cell must be walkable");
        assert_ne!(s, pos, "must move off the blocked center");
        assert!(s.y < pos.y, "north origin → north stand, got {s:?}");
    }

    #[test]
    fn pantry_stand_follows_origin_side() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 32, 10);
        let south = stand_point(
            WaypointKind::Pantry,
            pos,
            (32, 10),
            &m,
            Point { x: 50, y: 95 },
        );
        assert!(south.y > pos.y, "south origin → south stand, got {south:?}");
        let east = stand_point(
            WaypointKind::Pantry,
            pos,
            (32, 10),
            &m,
            Point { x: 98, y: 50 },
        );
        assert!(east.x > pos.x, "east origin → east stand, got {east:?}");
    }

    #[test]
    fn seat_kinds_pass_through_pos() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 20, 7);
        for kind in [
            WaypointKind::Couch,
            WaypointKind::MeetingSofa,
            WaypointKind::MeetingStand,
        ] {
            assert_eq!(
                stand_point(kind, pos, (0, 0), &m, Point { x: 10, y: 10 }),
                pos,
                "{kind:?} pos is already the seat cell"
            );
        }
    }

    #[test]
    fn real_layout_stand_points_are_walkable() {
        // Guards against a floor whose obstacle furniture has no walkable
        // approach side (the real geometry, not a synthetic mask). Seat kinds
        // (couch/meeting) intentionally return their on-furniture seat cell —
        // which IS blocked (the sprite sits on it; A* snaps the walk adjacent)
        // — so they're excluded from the walkability assertion.
        use crate::layout::SceneLayout;
        let seat = |k| {
            matches!(
                k,
                WaypointKind::Couch | WaypointKind::MeetingSofa | WaypointKind::MeetingStand
            )
        };
        for seed in 0..5u64 {
            let l = SceneLayout::compute_with_seed(160, 120, 6, seed).unwrap();
            let origin = l
                .home_desks
                .first()
                .copied()
                .unwrap_or(Point { x: 0, y: 0 });
            for wp in &l.waypoints {
                let s =
                    super::stand_point(wp.kind, wp.pos, l.pantry_counter_size, &l.walkable, origin);
                if seat(wp.kind) {
                    assert_eq!(s, wp.pos, "seed {seed}: {:?} should pass through", wp.kind);
                } else {
                    assert!(
                        l.walkable.is_walkable(s.x, s.y) && s != wp.pos,
                        "seed {seed}: {:?} stand {s:?} not a walkable off-center cell (center {:?})",
                        wp.kind,
                        wp.pos
                    );
                }
            }
        }
    }

    #[test]
    fn real_pantry_stand_responds_to_desk_side() {
        // On the real standard floor, a desk to the NORTH must not yield a
        // stand cell *south* of one chosen for a desk to the SOUTH — the side
        // tracks the origin (the whole point: a desk above the pantry → a
        // top-down approach, not a detour below).
        use crate::layout::SceneLayout;
        let l = SceneLayout::compute(120, 96, 4).unwrap();
        let p = l
            .waypoints
            .iter()
            .find(|w| w.kind == WaypointKind::Pantry)
            .expect("pantry")
            .pos;
        let cs = l.pantry_counter_size;
        let north = stand_point(
            WaypointKind::Pantry,
            p,
            cs,
            &l.walkable,
            Point { x: p.x, y: 0 },
        );
        let south = stand_point(
            WaypointKind::Pantry,
            p,
            cs,
            &l.walkable,
            Point {
                x: p.x,
                y: l.buf_h - 1,
            },
        );
        assert!(
            north.y <= south.y,
            "north-desk stand {north:?} should be no lower than south-desk stand {south:?}"
        );
    }

    #[test]
    fn picks_only_open_side_when_walled() {
        // Vending machine recessed into a wall — everything down to y=54 is
        // blocked, so N/E/W are walled and only the south side is open. Even a
        // north origin must stand south.
        let pos = Point { x: 50, y: 50 };
        let mut m = WalkableMask::new_open(100, 100);
        m.mark_blocked(0, 0, 100, 54, 0);
        let s = stand_point(
            WaypointKind::VendingMachine,
            pos,
            (0, 0),
            &m,
            Point { x: 50, y: 5 },
        );
        assert!(m.is_walkable(s.x, s.y));
        assert!(s.y > pos.y, "only south is open, got {s:?}");
    }
}
