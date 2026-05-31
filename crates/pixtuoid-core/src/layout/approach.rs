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

use super::decor::{furniture_def, Facing, WaypointKind};
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
/// sofa/table furniture, already stamped elsewhere). Reads [`furniture_def`]
/// — the single source of truth for furniture shape — and special-cases the
/// one runtime-sized kind (`Pantry`, whose counter scales with terminal width
/// and so isn't a static table row). Consumed by `mask::build_walkable_mask`
/// and [`stand_point`], so the footprint can't drift between them.
pub(super) fn obstacle_footprint(
    kind: WaypointKind,
    pantry_counter_size: (u16, u16),
) -> Option<(u16, u16)> {
    if matches!(kind, WaypointKind::Pantry) {
        return Some(pantry_counter_size);
    }
    furniture_def(kind.furniture()).footprint
}

/// Footprint half-extents `(hx, hy)` for stand-cell resolution, or `None` for
/// `occupies_pos` furniture (couch and meeting slots) whose `pos` is the cell
/// the agent occupies ON the furniture — those pass through `pos` unchanged,
/// no stand resolution. Gated on [`furniture_def`]`.occupies_pos`, a superset
/// of `footprint.is_none()`: the couch HAS a footprint yet occupies its `pos`.
fn half_extents(kind: WaypointKind, pantry_counter_size: (u16, u16)) -> Option<(u16, u16)> {
    if furniture_def(kind.furniture()).occupies_pos {
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
    facing: Facing,
) -> Point {
    let Some((hx, hy)) = half_extents(kind, pantry_counter_size) else {
        return pos;
    };
    let approach = furniture_def(kind.furniture()).approach;

    // N, S, W, E unit axes.
    const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
    let mut best: Option<(u64, Point)> = None;
    for (dx, dy) in DIRS {
        // Honor the per-furniture approach allowlist (rotated to facing).
        // All obstacle furniture is `ALL` today, so this is a no-op for them;
        // editing a kind's `approach` constrains both walk + render here.
        if !approach.allows(facing, (dx, dy)) {
            continue;
        }
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

/// A seat body (sofa) is wider than an appliance footprint, so its side
/// approach cells sit farther out than [`STAND_SCAN`] would reach. Scan this
/// far from the seat centre to clear the furniture and land on the floor.
const SEAT_APPROACH_SCAN: i32 = 14;

/// Where an agent WALKS to when visiting `kind` at `pos` with the given
/// `facing`. For obstacle furniture this is exactly [`stand_point`] (the side
/// stand cell). For `occupies_pos` seats — whose sprite RENDERS on `pos` — it
/// is an allowed-side walkable cell ADJACENT to the seat, nearest `origin`, so
/// the agent approaches from the front/sides and never paths in through the
/// back. Arrival is therefore a short settle from the approach cell onto the
/// seat (the render anchor stays `pos`). Falls back to `pos` (router snaps) if
/// no allowed side is walkable.
pub fn walk_target(
    kind: WaypointKind,
    pos: Point,
    pantry_counter_size: (u16, u16),
    mask: &WalkableMask,
    origin: Point,
    facing: Facing,
) -> Point {
    let def = furniture_def(kind.furniture());
    if !def.occupies_pos {
        return stand_point(kind, pos, pantry_counter_size, mask, origin, facing);
    }
    const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
    let mut best: Option<(u64, Point)> = None;
    for (dx, dy) in DIRS {
        if !def.approach.allows(facing, (dx, dy)) {
            continue;
        }
        for dist in 1..=SEAT_APPROACH_SCAN {
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
            Facing::South,
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
            Facing::South,
        );
        assert!(south.y > pos.y, "south origin → south stand, got {south:?}");
        let east = stand_point(
            WaypointKind::Pantry,
            pos,
            (32, 10),
            &m,
            Point { x: 98, y: 50 },
            Facing::South,
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
                stand_point(kind, pos, (0, 0), &m, Point { x: 10, y: 10 }, Facing::South),
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
        // 160×120 (walkway 8 → no vending/printer) AND 160×150 (walkway ≥10 →
        // vending + printer spawn) so the appliance stand cells are covered too.
        for (bw, bh) in [(160u16, 120u16), (160, 150)] {
            for seed in 0..5u64 {
                let l = SceneLayout::compute_with_seed(bw, bh, 6, seed).unwrap();
                let origin = l
                    .home_desks
                    .first()
                    .copied()
                    .unwrap_or(Point { x: 0, y: 0 });
                for wp in &l.waypoints {
                    let s = super::stand_point(
                        wp.kind,
                        wp.pos,
                        l.pantry_counter_size,
                        &l.walkable,
                        origin,
                        wp.facing,
                    );
                    if seat(wp.kind) {
                        assert_eq!(s, wp.pos, "seed {seed}: {:?} should pass through", wp.kind);
                    } else {
                        assert!(
                            l.walkable.is_walkable(s.x, s.y) && s != wp.pos,
                            "{bw}x{bh} seed {seed}: {:?} stand {s:?} not a walkable off-center cell (center {:?})",
                            wp.kind,
                            wp.pos
                        );
                    }
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
            Facing::South,
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
            Facing::South,
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
            Facing::South,
        );
        assert!(m.is_walkable(s.x, s.y));
        assert!(s.y > pos.y, "only south is open, got {s:?}");
    }

    #[test]
    fn walk_target_seat_never_approaches_through_the_back() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 20, 7); // sofa-ish body
                                                          // South-facing sofa: back is north. Even with the origin due north
                                                          // (which would otherwise pull a north approach), the walk-in must NOT
                                                          // be due-north of the seat.
        let south = walk_target(
            WaypointKind::MeetingSofa,
            pos,
            (0, 0),
            &m,
            Point { x: 50, y: 5 },
            Facing::South,
        );
        assert!(m.is_walkable(south.x, south.y) && south != pos);
        assert!(
            !(south.x == pos.x && south.y < pos.y),
            "south-facing sofa back is north — must not approach due-north: {south:?}"
        );
        // North-facing sofa: back is south. Origin due south must not yield a
        // due-south approach.
        let north = walk_target(
            WaypointKind::MeetingSofa,
            pos,
            (0, 0),
            &m,
            Point { x: 50, y: 95 },
            Facing::North,
        );
        assert!(m.is_walkable(north.x, north.y) && north != pos);
        assert!(
            !(north.x == pos.x && north.y > pos.y),
            "north-facing sofa back is south — must not approach due-south: {north:?}"
        );
    }

    #[test]
    fn walk_target_for_obstacle_delegates_to_stand_point() {
        let pos = Point { x: 50, y: 50 };
        let m = mask_with_obstacle(100, 100, pos, 32, 10);
        let origin = Point { x: 50, y: 8 };
        assert_eq!(
            walk_target(
                WaypointKind::Pantry,
                pos,
                (32, 10),
                &m,
                origin,
                Facing::South
            ),
            stand_point(
                WaypointKind::Pantry,
                pos,
                (32, 10),
                &m,
                origin,
                Facing::South
            ),
            "obstacle walk_target must equal stand_point",
        );
    }

    // ── furniture_def single-source-of-truth invariants ────────────────────
    // These iterate `WaypointKind::ALL` so re-introducing a hardcoded shape
    // number or breaking a derivation fails here, not as a silent visual bug.

    #[test]
    fn def_footprint_matches_obstacle_footprint() {
        // furniture_def is the SoT; obstacle_footprint must agree for every
        // non-Pantry kind (Pantry is runtime-sized → special-cased).
        let dummy = (32u16, 10u16);
        for &kind in WaypointKind::ALL {
            if kind == WaypointKind::Pantry {
                continue;
            }
            assert_eq!(
                furniture_def(kind.furniture()).footprint,
                obstacle_footprint(kind, dummy),
                "{kind:?}: furniture_def.footprint must equal obstacle_footprint",
            );
        }
    }

    #[test]
    fn occupies_pos_is_exactly_the_seat_kinds() {
        for &kind in WaypointKind::ALL {
            let expected = matches!(
                kind,
                WaypointKind::Couch | WaypointKind::MeetingSofa | WaypointKind::MeetingStand
            );
            assert_eq!(
                furniture_def(kind.furniture()).occupies_pos,
                expected,
                "{kind:?}: occupies_pos must be true iff the agent occupies pos directly",
            );
        }
    }

    #[test]
    fn approachable_obstacle_resolves_half_extents() {
        let dummy = (32u16, 10u16);
        for &kind in WaypointKind::ALL {
            let def = furniture_def(kind.furniture());
            let has_footprint = def.footprint.is_some() || kind == WaypointKind::Pantry;
            if has_footprint && !def.occupies_pos {
                assert!(
                    half_extents(kind, dummy).is_some(),
                    "{kind:?}: an approachable obstacle must resolve half-extents",
                );
            }
        }
    }

    #[test]
    fn dwell_range_is_nonzero() {
        // pose::dwell_ms does `% range.max(1)`; a zero range would silently
        // collapse the jitter. Keep every row's range positive.
        for &kind in WaypointKind::ALL {
            assert!(
                furniture_def(kind.furniture()).dwell.1 > 0,
                "{kind:?}: dwell range must be > 0",
            );
        }
    }

    #[test]
    fn pod_and_waypoint_twins_resolve_to_one_furniture_row() {
        // PhoneBooth/StandingDesk exist as BOTH a PodDecor and a WaypointKind;
        // after the fold they must map to the SAME Furniture row, so geometry
        // cannot drift between the two roles.
        use crate::layout::PodDecor;
        for (pod, wp) in [
            (PodDecor::PhoneBooth, WaypointKind::PhoneBooth),
            (PodDecor::StandingDesk, WaypointKind::StandingDesk),
        ] {
            assert_eq!(
                pod.furniture(),
                wp.furniture(),
                "{pod:?}/{wp:?}: pod + waypoint twins must share one Furniture row",
            );
        }
    }

    #[test]
    fn waypoint_kind_all_is_unique_and_complete() {
        use std::collections::HashSet;
        let set: HashSet<_> = WaypointKind::ALL.iter().copied().collect();
        assert_eq!(
            set.len(),
            WaypointKind::ALL.len(),
            "ALL contains duplicates"
        );
        // furniture_def's match is compiler-forced exhaustive; this count
        // catches a new variant that was added there but not to ALL.
        assert_eq!(
            WaypointKind::ALL.len(),
            8,
            "a WaypointKind variant was added/removed — update ALL",
        );
    }
}
