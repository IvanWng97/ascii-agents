//! Decor vocabulary used by `SceneLayout` — the enums describing every
//! piece of furniture and waypoint kind in the office. Kept separate from
//! geometry so adding a new sprite kind doesn't churn the layout math.

/// Wander destinations the Idle state machine can pick. Each kind controls
/// the pose + sprite an arriving agent takes. Plants/lamps are decor, not
/// waypoints. Coffee folded into Pantry — the pantry sprite already has
/// a coffee machine on its counter, so visiting the pantry covers both
/// "kitchen" and "coffee break".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaypointKind {
    /// Top-of-cubicle viewing couch facing the city windows.
    Couch,
    /// Pantry counter — kitchen + coffee.
    Pantry,
    /// Aisle phone booth — agent stands at the door (private call).
    PhoneBooth,
    /// Aisle standing desk — agent stands at the desk (alternate
    /// workstation). Random which exact StandingDesk slot is used.
    StandingDesk,
    /// Corridor vending machine — agent stands in front to grab a drink.
    VendingMachine,
    /// Corridor printer — agent stands in front while "printing."
    Printer,
    /// Meeting-room sofa seat — agent sits, facing the table. Multiple
    /// seats per sofa; a group conversation runs when ≥2 share the room.
    MeetingSofa,
    /// Meeting-room standing spot beside the table — agent stands, facing
    /// the table. Part of the same room conversation venue as MeetingSofa.
    MeetingStand,
}

/// Footprints for the two kinds that appear in BOTH `WaypointKind` (wander
/// destination) and `PodDecor` (aisle decor). Declared once so the mask
/// stamp and the wander-approach geometry read the same number and can't
/// drift apart. Referenced by both [`furniture_def`] and [`PodDecor::size`].
pub(crate) const PHONE_BOOTH_FOOTPRINT: (u16, u16) = (6, 12);
pub(crate) const STANDING_DESK_FOOTPRINT: (u16, u16) = (8, 8);

/// Which sides an agent may approach a piece of furniture from, in the
/// CANONICAL frame (furniture facing South, toward the viewer). [`Self::allows`]
/// rotates this to the live `facing`, so one stored set works for
/// variable-facing furniture (a sofa's "front + sides, no back" rotates to the
/// correct absolute sides whether it faces north or south). **To add/remove an
/// entry side, flip one bool** — single place, greppable. `n`/`s`/`e`/`w` are
/// the canonical absolute sides (north = −y).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApproachSides {
    pub n: bool,
    pub s: bool,
    pub e: bool,
    pub w: bool,
}

impl ApproachSides {
    /// 360° — approachable from every open side (pantry counter).
    pub const ALL: Self = Self {
        n: true,
        s: true,
        e: true,
        w: true,
    };

    /// This canonical (facing-South) set rotated to the live `facing`. South is
    /// the canonical front, so e.g. a "no back" set (front+sides) rotates to
    /// exclude whichever absolute side is now the back.
    pub fn rotated(self, facing: Facing) -> Self {
        let s = self;
        match facing {
            Facing::South => s,
            Facing::North => Self {
                n: s.s,
                s: s.n,
                e: s.w,
                w: s.e,
            },
            Facing::East => Self {
                n: s.e,
                s: s.w,
                e: s.s,
                w: s.n,
            },
            Facing::West => Self {
                n: s.w,
                s: s.e,
                e: s.n,
                w: s.s,
            },
        }
    }

    /// Is the absolute unit dir `(dx, dy)` (north = (0,−1), south = (0,1),
    /// east = (1,0), west = (−1,0)) an allowed approach under the live `facing`?
    pub fn allows(self, facing: Facing, dir: (i32, i32)) -> bool {
        let r = self.rotated(facing);
        match dir {
            (0, -1) => r.n,
            (0, 1) => r.s,
            (1, 0) => r.e,
            (-1, 0) => r.w,
            _ => false,
        }
    }
}

/// Approach sides for the home desk (the assigned workstation — NOT a
/// `furniture_def` row). Canonical: exclude the south front (the monitor faces
/// the viewer; the agent sits behind it), so reachable from N/E/W. Editing one
/// bool here changes the home-desk entry sides (e.g. drop east → `e: false`).
pub const DESK_APPROACH: ApproachSides = ApproachSides {
    n: true,
    s: false,
    e: true,
    w: true,
};

/// Definition record for a waypoint-addressable furniture kind — the single
/// source of truth for its ground shape, occupancy semantics, and dwell.
/// Reshaping a piece of furniture is editing ONE row of [`furniture_def`];
/// the walkable mask, stand-point, hit-test hitbox, and the render depth
/// baseline all DERIVE from these fields, so they cannot drift. Render-only
/// choices (sprite name, back-cap policy) deliberately stay in the tui crate
/// — `pixtuoid-core` has no terminal deps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FurnitureDef {
    /// Ground footprint `(w, h)` the walkable mask stamps (top-down z=0
    /// rect), or `None` for slots that add no obstacle of their own
    /// (MeetingSofa/MeetingStand sit on sofa/table furniture stamped
    /// elsewhere). NB: `Pantry` is also `None` here because its footprint is
    /// runtime-sized (`pantry_counter_size`); `obstacle_footprint`
    /// special-cases it — the one kind whose shape isn't a static literal.
    pub footprint: Option<(u16, u16)>,
    /// The agent occupies `pos` DIRECTLY (sprite renders ON the furniture),
    /// so `stand_point` passes `pos` through unchanged instead of resolving a
    /// walkable cell beside the furniture (A* then snaps the walk adjacent).
    /// NOT "a human can sit here": `MeetingStand` is *standing* yet sets this
    /// true (the agent still occupies its `pos`). Opposite case (Pantry/
    /// vending/printer/phone-booth/standing-desk): `pos` = blocked obstacle
    /// CENTER, approached from a side. True set: {Couch, MeetingSofa,
    /// MeetingStand}. (Desks are NOT rows here — home workstation is separate.)
    pub occupies_pos: bool,
    /// Per-spot idle dwell window `(base_ms, range_ms)`. Invariant: range > 0
    /// (a zero range would divide-by-zero in `pose::dwell_ms`).
    pub dwell: (u64, u64),
    /// Canonical (facing-South) sides an agent may approach from. Obstacle
    /// furniture against walls keeps `ALL` (walls already constrain the open
    /// side); seats use "front + sides, no back" so a walker never paths in
    /// through the sofa back. Edit one bool to change an entry side.
    pub approach: ApproachSides,
}

/// Canonical seat approach: front + sides, exclude the back. Rotates with
/// facing so a north- or south-facing sofa each exclude their own back.
const SEAT_APPROACH: ApproachSides = ApproachSides {
    n: false,
    s: true,
    e: true,
    w: true,
};

impl WaypointKind {
    /// Every variant, for exhaustive invariant tests (mirrors
    /// [`PodDecor::ALL`]). Iteration-only — order is not load-bearing.
    pub const ALL: &'static [WaypointKind] = &[
        WaypointKind::Couch,
        WaypointKind::Pantry,
        WaypointKind::PhoneBooth,
        WaypointKind::StandingDesk,
        WaypointKind::VendingMachine,
        WaypointKind::Printer,
        WaypointKind::MeetingSofa,
        WaypointKind::MeetingStand,
    ];
}

/// THE furniture table — one row per kind, the single source of truth for
/// ground shape + occupancy + dwell. Every geometric dependent (mask,
/// stand-point half-extents, hit-test size, render depth baseline) derives
/// from `footprint`; do not re-type these numbers anywhere else.
pub const fn furniture_def(kind: WaypointKind) -> FurnitureDef {
    match kind {
        WaypointKind::Couch => FurnitureDef {
            footprint: Some((8, 7)),
            occupies_pos: true,
            dwell: (20_000, 20_000),
            approach: SEAT_APPROACH,
        },
        WaypointKind::Pantry => FurnitureDef {
            footprint: None, // runtime-sized — see obstacle_footprint
            occupies_pos: false,
            dwell: (10_000, 8_000),
            approach: ApproachSides::ALL,
        },
        WaypointKind::PhoneBooth => FurnitureDef {
            footprint: Some(PHONE_BOOTH_FOOTPRINT),
            occupies_pos: false,
            dwell: (8_000, 22_000),
            approach: ApproachSides::ALL,
        },
        WaypointKind::StandingDesk => FurnitureDef {
            footprint: Some(STANDING_DESK_FOOTPRINT),
            occupies_pos: false,
            dwell: (8_000, 22_000),
            approach: ApproachSides::ALL,
        },
        WaypointKind::VendingMachine => FurnitureDef {
            footprint: Some((4, 6)),
            occupies_pos: false,
            dwell: (4_000, 4_000),
            approach: ApproachSides::ALL,
        },
        WaypointKind::Printer => FurnitureDef {
            footprint: Some((5, 4)),
            occupies_pos: false,
            dwell: (4_000, 4_000),
            approach: ApproachSides::ALL,
        },
        WaypointKind::MeetingSofa => FurnitureDef {
            footprint: None,
            occupies_pos: true,
            dwell: (20_000, 20_000),
            approach: SEAT_APPROACH,
        },
        WaypointKind::MeetingStand => FurnitureDef {
            footprint: None,
            occupies_pos: true,
            dwell: (20_000, 20_000),
            approach: SEAT_APPROACH,
        },
    }
}

/// Which way a waypoint occupant faces. Drives sprite choice (back vs
/// front view) and horizontal mirroring at render time. Most waypoints
/// are `South` (facing the viewer / facing-neutral); meeting-room slots
/// face the table at the room centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facing {
    North,
    South,
    East,
    West,
}

/// Wall-mounted / wall-leaning furniture, painted as decor in the top wall
/// area. Not a wander destination — agents can't walk through their own
/// cubicle row to reach the back wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WallDecor {
    Bookshelf,
    Whiteboard,
    BulletinBoard,
    ExitSign,
    /// Wall-mounted meeting-room display — paints above the meeting
    /// room interior so participants can pretend they're presenting.
    MeetingScreen,
}

impl WallDecor {
    pub fn size(self) -> (u16, u16) {
        match self {
            WallDecor::Whiteboard => (14, 11),
            WallDecor::Bookshelf => (8, 12),
            WallDecor::BulletinBoard => (10, 6),
            WallDecor::ExitSign => (5, 3),
            WallDecor::MeetingScreen => (14, 12),
        }
    }
}

/// Variety of potted plants — each renders a different sprite. Spread
/// these around the lounge so it doesn't feel like one ficus repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlantKind {
    Ficus,
    Tall,
    Flower,
    Succulent,
}

impl PlantKind {
    pub fn size(self) -> (u16, u16) {
        match self {
            PlantKind::Ficus => (6, 7),
            PlantKind::Tall => (6, 10),
            PlantKind::Flower => (6, 6),
            PlantKind::Succulent => (5, 4),
        }
    }
}

/// Decor placed in the aisles BETWEEN 2×2 desk pods. Picked at random
/// (deterministic hash of pod index) so each office layout is varied
/// but stable across renders. Each variant maps to a distinct sprite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PodDecor {
    PlantTall,
    Whiteboard,
    Tv,
    PhoneBooth,
    StandingDesk,
}

impl PodDecor {
    /// The randomly-picked pool. Whiteboard (14 wide) fits in the
    /// 22-px aisle with ~3 px of walking clearance after the 1-px
    /// obstacle pad — same rolling-whiteboard sprite as the wall
    /// mount, just placed in an aisle slot.
    pub const ALL: &'static [PodDecor] = &[
        PodDecor::PlantTall,
        PodDecor::Whiteboard,
        PodDecor::Tv,
        PodDecor::PhoneBooth,
        PodDecor::StandingDesk,
    ];

    /// Width / height in buffer pixels — used for both rendering offset
    /// (centred placement) and walkable-mask obstacle dimensions. Sprite
    /// sizes are fixed: PlantTall=6×10, Whiteboard=14×11, Tv=10×10,
    /// PhoneBooth=6×12, StandingDesk=8×8.
    pub fn size(self) -> (u16, u16) {
        match self {
            PodDecor::PlantTall => (6, 10),
            PodDecor::Whiteboard => (14, 11),
            PodDecor::Tv => (10, 10),
            // Shared with the WaypointKind footprint (these two are ALSO wander
            // destinations) so the mask stamp can't drift between the two enums.
            PodDecor::PhoneBooth => PHONE_BOOTH_FOOTPRINT,
            PodDecor::StandingDesk => STANDING_DESK_FOOTPRINT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: (i32, i32) = (0, -1);
    const S: (i32, i32) = (0, 1);
    const E: (i32, i32) = (1, 0);
    const W: (i32, i32) = (-1, 0);

    fn allowed(sides: ApproachSides, facing: Facing) -> Vec<(i32, i32)> {
        [N, S, E, W]
            .into_iter()
            .filter(|&d| sides.allows(facing, d))
            .collect()
    }

    #[test]
    fn all_allows_every_side_for_any_facing() {
        for facing in [Facing::North, Facing::South, Facing::East, Facing::West] {
            assert_eq!(allowed(ApproachSides::ALL, facing), vec![N, S, E, W]);
        }
    }

    #[test]
    fn seat_facing_south_allows_front_and_sides_not_back() {
        // Sofa facing south (front toward viewer): approach S + E + W, not N.
        assert_eq!(allowed(SEAT_APPROACH, Facing::South), vec![S, E, W]);
    }

    #[test]
    fn seat_facing_north_rotates_to_exclude_the_south_back() {
        // Sofa facing north (back toward viewer/south): approach N + E + W, not S.
        assert_eq!(allowed(SEAT_APPROACH, Facing::North), vec![N, E, W]);
    }

    #[test]
    fn desk_excludes_its_south_front() {
        // Home desk faces south (monitor toward viewer): reachable N/E/W only.
        assert_eq!(allowed(DESK_APPROACH, Facing::South), vec![N, E, W]);
        // And "remove east" would be a one-bool edit:
        let no_east = ApproachSides {
            e: false,
            ..DESK_APPROACH
        };
        assert_eq!(allowed(no_east, Facing::South), vec![N, W]);
    }

    #[test]
    fn rotation_is_a_bijection_on_sides() {
        // A single-side set must map to exactly one side under any facing
        // (no side lost or duplicated by the rotation).
        for facing in [Facing::North, Facing::South, Facing::East, Facing::West] {
            for one in [N, S, E, W] {
                let sides = ApproachSides {
                    n: one == N,
                    s: one == S,
                    e: one == E,
                    w: one == W,
                };
                assert_eq!(
                    allowed(sides, facing).len(),
                    1,
                    "facing {facing:?}, side {one:?} must rotate to exactly one side",
                );
            }
        }
    }
}
