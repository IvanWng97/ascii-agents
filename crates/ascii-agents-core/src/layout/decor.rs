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
            PodDecor::PhoneBooth => (6, 12),
            PodDecor::StandingDesk => (8, 8),
        }
    }
}
