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
}

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
        },
        WaypointKind::Pantry => FurnitureDef {
            footprint: None, // runtime-sized — see obstacle_footprint
            occupies_pos: false,
            dwell: (10_000, 8_000),
        },
        WaypointKind::PhoneBooth => FurnitureDef {
            footprint: Some(PHONE_BOOTH_FOOTPRINT),
            occupies_pos: false,
            dwell: (8_000, 22_000),
        },
        WaypointKind::StandingDesk => FurnitureDef {
            footprint: Some(STANDING_DESK_FOOTPRINT),
            occupies_pos: false,
            dwell: (8_000, 22_000),
        },
        WaypointKind::VendingMachine => FurnitureDef {
            footprint: Some((4, 6)),
            occupies_pos: false,
            dwell: (4_000, 4_000),
        },
        WaypointKind::Printer => FurnitureDef {
            footprint: Some((5, 4)),
            occupies_pos: false,
            dwell: (4_000, 4_000),
        },
        WaypointKind::MeetingSofa => FurnitureDef {
            footprint: None,
            occupies_pos: true,
            dwell: (20_000, 20_000),
        },
        WaypointKind::MeetingStand => FurnitureDef {
            footprint: None,
            occupies_pos: true,
            dwell: (20_000, 20_000),
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
