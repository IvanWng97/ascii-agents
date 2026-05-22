//! Zone-based scene layout for the top-down office — primitive geometry
//! only, no terminal deps. Computed once per (buf_w, buf_h, num_agents)
//! triple; serializable / wire-shippable for the future v2 daemon split.
//!
//! Splits a buf-pixel rectangle into quadrants (meeting / pantry /
//! cubicles / lounge), then computes per-agent home desks, named lounge
//! waypoints, decor positions, and a per-pixel walkability mask.

use crate::walkable::WalkableMask;

/// Primitive rectangle. Same shape as `ratatui::layout::Rect` so the
/// binary can convert with a one-line field-by-field copy without paying
/// for the ratatui dep in core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bounds {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waypoint {
    pub pos: Point,
    pub kind: WaypointKind,
}

#[derive(Debug, Clone)]
pub struct SceneLayout {
    pub buf_w: u16,
    pub buf_h: u16,
    pub cubicle_band: Bounds,
    /// Horizontal corridor at the bottom of the cubicle area — the "main
    /// aisle" connecting door / meeting / pantry. Used by the cat
    /// wanderer and as a fallback location for overflow floor seats.
    pub walkway: Bounds,
    pub home_desks: Vec<Point>,
    pub waypoints: Vec<Waypoint>,
    pub plants: Vec<(PlantKind, Point)>,
    pub wall_decor: Vec<(WallDecor, Point)>,
    pub floor_lamp: Option<Point>,
    pub door: Option<Point>,
    pub door_threshold: Option<Point>,
    pub floor_seats: Vec<Point>,
    pub meeting_room: Option<Bounds>,
    pub pantry_room: Option<Bounds>,
    pub meeting_sofas: Vec<Point>,
    pub meeting_table: Option<Point>,
    pub room_walls: Vec<(Point, Point)>,
    pub top_margin: u16,
    pub pantry_table: Option<Point>,
    pub pantry_chairs: Vec<Point>,
    pub corridor: Option<Bounds>,
    pub walkable: WalkableMask,
}

/// Padding (in pixels) added around every obstacle when building the
/// walkable mask. Reserves a buffer zone so characters route AROUND
/// furniture rather than scraping along its edge.
pub const OBSTACLE_PAD_PX: u16 = 2;

pub const WAYPOINT_COUNT: usize = 2;
pub const DESK_W: u16 = 12;
pub const DESK_H: u16 = 6;
/// Hard cap on how many cubicles get painted regardless of how high
/// `max_desks` is set. Bumped from 8 → 16 after the lounge_band quadrant
/// was retired and the cubicle band absorbed its vertical space — more
/// rows fit, so more agents can have their own desk before falling back
/// to overflow seating.
pub const MAX_VISIBLE_DESKS: usize = 16;
pub const DESK_GAP_X: u16 = 11;
pub const DESK_GAP_Y: u16 = 14;
pub const MIN_TOP_MARGIN: u16 = 20;

impl SceneLayout {
    /// Returns `None` if the buffer is too small for even one cubicle and the
    /// fixed lounge area. Caller should paint a "terminal too small" message.
    pub fn compute(buf_w: u16, buf_h: u16, num_agents: usize) -> Option<Self> {
        const MIN_W: u16 = DESK_W + DESK_GAP_X * 2;
        let min_h: u16 = 40 + MIN_TOP_MARGIN;
        if buf_w < MIN_W || buf_h < min_h {
            return None;
        }

        let top_margin = (buf_h / 4).max(MIN_TOP_MARGIN);
        let usable_h = buf_h - top_margin;
        let mid_x = buf_w * 28 / 100;
        let mid_y_split = top_margin + usable_h / 2;

        let meeting_room = Some(Bounds {
            x: 0,
            y: top_margin,
            width: mid_x,
            height: usable_h / 2,
        });
        let pantry_room = Some(Bounds {
            x: 0,
            y: mid_y_split,
            width: mid_x,
            height: usable_h - usable_h / 2,
        });

        let right_x = mid_x + 2;
        let right_w = buf_w.saturating_sub(right_x);
        // Reserve the last 3 px for the baseboard sprite (matches the
        // BASEBOARD_H constant in the renderer's paint_floor_and_walls).
        // Without this reservation, walkway_h overlapped the baseboard and
        // the corridor itself was marked non-walkable — agents couldn't
        // route through it, which is one of the root causes of "闪现".
        const BASEBOARD_RESERVE: u16 = 3;
        // Walkway floors at 8 px so the corridor is wide enough for the
        // coarsened 4×4 path grid to see at least 2 walkable cell rows
        // even after obstacle padding.
        let walkway_h = (usable_h / 10).max(8);
        let cubicle_h = usable_h
            .saturating_sub(walkway_h)
            .saturating_sub(BASEBOARD_RESERVE);
        let cubicle_band = Bounds {
            x: right_x,
            y: top_margin,
            width: right_w,
            height: cubicle_h,
        };
        let walkway = Bounds {
            x: right_x,
            y: top_margin + cubicle_h,
            width: right_w,
            height: walkway_h,
        };

        let col_w = DESK_W + DESK_GAP_X;
        let row_h = DESK_H + DESK_GAP_Y;
        let cols = ((right_w.saturating_sub(DESK_GAP_X)) / col_w).max(1);
        let rows = (cubicle_h / row_h).max(1);
        let max_grid = (cols * rows) as usize;
        let n = num_agents.min(max_grid).min(MAX_VISIBLE_DESKS);
        let mut home_desks = Vec::with_capacity(n);
        for i in 0..n {
            let r = (i as u16) / cols;
            let c = (i as u16) % cols;
            home_desks.push(Point {
                x: right_x + DESK_GAP_X + c * col_w,
                y: cubicle_band.y + DESK_GAP_Y + r * row_h,
            });
        }

        let meeting_sofas = if let Some(mr) = meeting_room {
            let cx = mr.x + mr.width / 2;
            vec![
                Point {
                    x: cx,
                    y: mr.y + mr.height * 30 / 100,
                },
                Point {
                    x: cx,
                    y: mr.y + mr.height * 80 / 100,
                },
            ]
        } else {
            vec![]
        };
        let meeting_table = meeting_room.map(|mr| Point {
            x: mr.x + mr.width / 2,
            y: mr.y + mr.height / 2,
        });

        // Doorway widths are ABSOLUTE pixels — using percentages here makes
        // the gap shrink to zero on smaller terminals, which after the
        // 2-px wall obstacle padding leaves no walkable cell for A* and
        // disconnects the meeting room from the rest of the office.
        //
        // Minimums: 14 px gives ≥10 px effective gap after padding, which
        // is wide enough that the coarsened 4×4 router grid has at least
        // one row of walkable cells through the doorway.
        const DOOR_GAP_V: u16 = 14;
        const DOOR_GAP_H: u16 = 14;
        let mut room_walls = Vec::new();
        let v_x = mid_x;
        let v_top = top_margin;
        let v_bot = mid_y_split;
        let v_door_center = top_margin + (v_bot - v_top) / 2;
        let v_door_top = v_door_center.saturating_sub(DOOR_GAP_V / 2);
        let v_door_bot = (v_door_center + DOOR_GAP_V / 2).min(v_bot);
        room_walls.push((
            Point { x: v_x, y: v_top },
            Point {
                x: v_x,
                y: v_door_top,
            },
        ));
        room_walls.push((
            Point {
                x: v_x,
                y: v_door_bot,
            },
            Point { x: v_x, y: v_bot },
        ));
        let h_y = mid_y_split;
        let h_door_center = mid_x * 60 / 100;
        let h_door_left = h_door_center.saturating_sub(DOOR_GAP_H / 2);
        let h_door_right = (h_door_center + DOOR_GAP_H / 2).min(mid_x);
        room_walls.push((
            Point { x: 0, y: h_y },
            Point {
                x: h_door_left,
                y: h_y,
            },
        ));
        room_walls.push((
            Point {
                x: h_door_right,
                y: h_y,
            },
            Point { x: mid_x, y: h_y },
        ));

        // Two waypoints now: viewing couch (top of cubicle band, against
        // the city windows) and pantry (bottom-left, doubles as coffee).
        let couch_y = top_margin + 7;
        let couch_x = cubicle_band.x + cubicle_band.width * 35 / 100;
        let mut waypoints: Vec<Waypoint> = vec![Waypoint {
            pos: Point {
                x: couch_x,
                y: couch_y,
            },
            kind: WaypointKind::Couch,
        }];
        if let Some(pr) = pantry_room {
            // y at 60% (not 40%) so the counter sits well below the
            // meeting-room/pantry doorway — keeps the walkable strip
            // through the doorway wide enough for the coarsened
            // pathfinding grid to see it as passable.
            waypoints.push(Waypoint {
                pos: Point {
                    x: pr.x + pr.width * 60 / 100,
                    y: pr.y + pr.height * 60 / 100,
                },
                kind: WaypointKind::Pantry,
            });
        }

        // Plants scatter through the cubicle corridor edges + pantry.
        // No plants in the cubicle TOP strip — that area is too narrow
        // (the gap between top wall and the viewing couch is just 7 px,
        // not enough for a padded plant without blocking the room/door
        // walkability paths). No plants in the meeting room interior
        // either: sofas + table already fill most of the room, and any
        // plant inside its walkable strips disconnects the door gap.
        let plants: Vec<(PlantKind, Point)> = vec![
            // Corridor edges — far from any door or room exit.
            (
                PlantKind::Flower,
                Point {
                    x: cubicle_band.x + 4,
                    y: walkway.y.saturating_sub(4),
                },
            ),
            (
                PlantKind::Succulent,
                Point {
                    x: cubicle_band.x + cubicle_band.width.saturating_sub(4),
                    y: walkway.y.saturating_sub(4),
                },
            ),
        ]
        .into_iter()
        // No pantry plants — the room is small (≤ 26 px wide), and the
        // plant + 1-px pad blocks the only horizontal bridge between the
        // pantry interior and the cubicle area's bottom row. Leaving the
        // pantry plant-free keeps the mask fully connected.
        .chain(std::iter::empty::<(PlantKind, Point)>())
        // The meeting room is intentionally plant-free: sofas + table fill
        // most of the interior, leaving narrow walkable strips. Any plant
        // placed inside one of those strips disconnects the room from the
        // door gap.
        .collect();

        // Floor lamp now sits right next to the viewing couch so its halo
        // bathes the seating area at night.
        let floor_lamp = Some(Point {
            x: couch_x + 9,
            y: couch_y + 2,
        });

        let door = if buf_w >= 12 {
            Some(Point {
                x: buf_w.saturating_sub(10),
                y: top_margin.saturating_sub(10),
            })
        } else {
            None
        };
        let door_threshold = door.map(|d| Point {
            x: d.x.saturating_add(2),
            y: top_margin + 6,
        });

        let wall_decor = vec![
            (
                WallDecor::Bookshelf,
                Point {
                    x: buf_w * 18 / 100,
                    y: 6,
                },
            ),
            (
                WallDecor::BulletinBoard,
                Point {
                    x: buf_w * 42 / 100,
                    y: 8,
                },
            ),
            (
                WallDecor::ExitSign,
                Point {
                    x: buf_w.saturating_sub(9),
                    y: top_margin.saturating_sub(13),
                },
            ),
            (
                WallDecor::Whiteboard,
                Point {
                    x: mid_x + 3,
                    y: v_door_bot + 2,
                },
            ),
        ];

        let used_before_floor = home_desks.len() + meeting_sofas.len();
        let overflow_count = num_agents.saturating_sub(used_before_floor).min(8);
        let mut floor_seats: Vec<Point> = Vec::with_capacity(overflow_count);
        if let Some(pr) = pantry_room {
            for slot in 0..overflow_count.min(2) {
                floor_seats.push(Point {
                    x: pr.x + pr.width * (25 + slot as u16 * 40) / 100,
                    y: pr.y + pr.height * 60 / 100,
                });
            }
        }
        // Remaining overflow falls along the walkway/corridor edges —
        // sitters working on the floor in the main aisle. Spread across
        // the corridor width so they don't pile up.
        let remaining = overflow_count.saturating_sub(floor_seats.len());
        for slot in 0..remaining {
            let c = (slot as u16) % 4;
            let along_x = cubicle_band.x + cubicle_band.width * (10 + c * 25) / 100;
            floor_seats.push(Point {
                x: along_x,
                y: walkway.y + walkway.height / 2,
            });
        }

        let (pantry_table, pantry_chairs) = if let Some(pr) = pantry_room {
            let tx = pr.x + pr.width * 25 / 100;
            let ty = pr.y + pr.height * 25 / 100;
            (
                Some(Point { x: tx, y: ty }),
                vec![
                    Point {
                        x: tx.saturating_sub(4),
                        y: ty,
                    },
                    Point { x: tx + 4, y: ty },
                    Point {
                        x: tx,
                        y: ty.saturating_sub(3),
                    },
                    Point { x: tx, y: ty + 3 },
                ],
            )
        } else {
            (None, vec![])
        };

        let corridor = Some(Bounds {
            x: 0,
            y: walkway.y,
            width: buf_w,
            height: walkway.height,
        });

        let walkable = build_walkable_mask(
            buf_w,
            buf_h,
            top_margin,
            door,
            &home_desks,
            &meeting_sofas,
            meeting_table,
            pantry_table,
            &pantry_chairs,
            &waypoints,
            &plants,
            floor_lamp,
            &wall_decor,
            &room_walls,
        );

        Some(Self {
            buf_w,
            buf_h,
            cubicle_band,
            walkway,
            home_desks,
            waypoints,
            plants,
            wall_decor,
            floor_lamp,
            door,
            door_threshold,
            floor_seats,
            meeting_room,
            pantry_room,
            meeting_sofas,
            meeting_table,
            room_walls,
            top_margin,
            pantry_table,
            pantry_chairs,
            corridor,
            walkable,
        })
    }

    pub fn is_walkable(&self, x: u16, y: u16) -> bool {
        self.walkable.is_walkable(x, y)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_walkable_mask(
    buf_w: u16,
    buf_h: u16,
    top_margin: u16,
    door: Option<Point>,
    home_desks: &[Point],
    meeting_sofas: &[Point],
    meeting_table: Option<Point>,
    pantry_table: Option<Point>,
    pantry_chairs: &[Point],
    waypoints: &[Waypoint],
    plants: &[(PlantKind, Point)],
    floor_lamp: Option<Point>,
    wall_decor: &[(WallDecor, Point)],
    room_walls: &[(Point, Point)],
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

    for (start, end) in room_walls {
        if start.x == end.x {
            mask.mark_blocked(
                start.x,
                start.y.min(end.y),
                2,
                start.y.abs_diff(end.y) + 1,
                OBSTACLE_PAD_PX,
            );
        } else {
            mask.mark_blocked(
                start.x.min(end.x),
                start.y,
                start.x.abs_diff(end.x) + 1,
                2,
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
        mask.mark_blocked(
            sofa.x.saturating_sub(7),
            sofa.y.saturating_sub(3),
            14,
            6,
            OBSTACLE_PAD_PX,
        );
    }

    if let Some(t) = meeting_table {
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
            WaypointKind::Couch => (14, 6),
            WaypointKind::Pantry => (14, 7),
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

    for (kind, pos) in wall_decor {
        if matches!(kind, WallDecor::Whiteboard) {
            mask.mark_blocked(pos.x, pos.y, 14, 11, OBSTACLE_PAD_PX);
        }
    }

    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_returns_none_when_buf_too_small() {
        assert!(SceneLayout::compute(20, 20, 4).is_none());
    }

    #[test]
    fn compute_zones_are_ordered_top_to_bottom_and_nonoverlapping() {
        let l = SceneLayout::compute(120, 80, 6).expect("fits");
        assert!(l.cubicle_band.y < l.walkway.y);
        let c_bot = l.cubicle_band.y + l.cubicle_band.height;
        assert!(c_bot <= l.walkway.y, "cubicle overlaps walkway");
        // Walkway runs to the baseboard now that lounge_band is gone.
        let w_bot = l.walkway.y + l.walkway.height;
        assert!(w_bot <= l.buf_h);
    }

    #[test]
    fn compute_places_one_home_desk_per_agent() {
        let l = SceneLayout::compute(160, 80, 5).expect("fits");
        assert!(l.home_desks.len() <= 5 && !l.home_desks.is_empty());
        for d in &l.home_desks {
            assert!(d.y >= l.cubicle_band.y);
            assert!(d.y + DESK_H <= l.cubicle_band.y + l.cubicle_band.height);
            assert!(d.x >= l.cubicle_band.x);
        }
    }

    #[test]
    fn compute_places_all_waypoint_kinds() {
        let l = SceneLayout::compute(120, 96, 1).expect("fits");
        assert_eq!(l.waypoints.len(), WAYPOINT_COUNT);
        let kinds: std::collections::HashSet<_> = l.waypoints.iter().map(|w| w.kind).collect();
        assert!(kinds.contains(&WaypointKind::Couch));
        assert!(kinds.contains(&WaypointKind::Pantry));
        for w in &l.waypoints {
            match w.kind {
                WaypointKind::Pantry => {
                    let pr = l.pantry_room.expect("pantry");
                    assert!(w.pos.y >= pr.y && w.pos.y < pr.y + pr.height);
                    assert!(w.pos.x >= pr.x && w.pos.x < pr.x + pr.width);
                }
                WaypointKind::Couch => {
                    assert!(w.pos.y >= l.top_margin);
                    assert!(w.pos.y < l.cubicle_band.y + DESK_GAP_Y);
                }
            }
        }
    }

    #[test]
    fn compute_places_bookshelf_on_wall_and_whiteboard_in_walkway() {
        let l = SceneLayout::compute(120, 96, 1).expect("fits");
        let bookshelf = l
            .wall_decor
            .iter()
            .find(|(k, _)| *k == WallDecor::Bookshelf);
        let whiteboard = l
            .wall_decor
            .iter()
            .find(|(k, _)| *k == WallDecor::Whiteboard);
        assert!(bookshelf.is_some());
        assert!(whiteboard.is_some());
        assert!(bookshelf.unwrap().1.y < l.cubicle_band.y);
        assert!(whiteboard.unwrap().1.y > l.cubicle_band.y);
    }

    #[test]
    fn compute_places_plants_in_lounge_and_walkway() {
        let l = SceneLayout::compute(120, 96, 1).expect("fits");
        assert!(!l.plants.is_empty());
        for (_, p) in &l.plants {
            assert!(p.x < l.buf_w);
            assert!(p.y < l.buf_h);
        }
    }

    #[test]
    fn compute_truncates_home_desks_when_more_agents_than_fit() {
        let l = SceneLayout::compute(50, 80, 20).expect("fits");
        assert!(l.home_desks.len() < 20);
    }

    /// Pixel-level BFS from `door_threshold` must reach every walkable
    /// pixel across a range of buffer sizes. If this regresses, any
    /// agent stranded in an unreachable pocket will see A* return its
    /// straight-line fallback and visibly teleport across walls ("闪现").
    ///
    /// The probed sizes span small (typical 80-col terminal) through
    /// large (4K-cell terminal). Each pair is also probed with a high
    /// agent count to exercise the overflow seat placement.
    #[test]
    fn walkable_mask_is_fully_connected_across_buffer_sizes() {
        use std::collections::VecDeque;

        // Range covers the realistic terminal sizes — a small 80×35-cell
        // terminal up to a 4K-cell rig. Below 96×70 the meeting room
        // sofas + table + walls degenerate (sofa padding covers the
        // entire interior) which would be a layout-design problem
        // rather than a pathfinding regression.
        let sizes = [
            (96u16, 70u16, 7usize),
            (128, 80, 10),
            (160, 100, 12),
            (240, 130, 16),
            (320, 180, 16),
        ];
        for (buf_w, buf_h, num_agents) in sizes {
            let l = SceneLayout::compute(buf_w, buf_h, num_agents)
                .unwrap_or_else(|| panic!("layout fits at {buf_w}x{buf_h}"));
            let w = l.buf_w as usize;
            let h = l.buf_h as usize;
            let start = l
                .door_threshold
                .unwrap_or_else(|| panic!("door_threshold missing at {buf_w}x{buf_h}"));
            assert!(
                l.is_walkable(start.x, start.y),
                "door_threshold {start:?} not walkable at {buf_w}x{buf_h}"
            );

            // BFS from the threshold.
            let mut visited = vec![false; w * h];
            visited[(start.y as usize) * w + (start.x as usize)] = true;
            let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
            queue.push_back((start.x as usize, start.y as usize));
            let mut reachable = 1usize;
            while let Some((x, y)) = queue.pop_front() {
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if nx >= w || ny >= h || visited[ny * w + nx] {
                        continue;
                    }
                    if !l.is_walkable(nx as u16, ny as u16) {
                        continue;
                    }
                    visited[ny * w + nx] = true;
                    reachable += 1;
                    queue.push_back((nx, ny));
                }
            }

            // Total walkable pixels.
            let mut walkable_total = 0usize;
            for y in 0..h {
                for x in 0..w {
                    if l.is_walkable(x as u16, y as u16) {
                        walkable_total += 1;
                    }
                }
            }
            assert_eq!(
                reachable,
                walkable_total,
                "{buf_w}x{buf_h} ({num_agents} agents): {} disconnected pixels — \
                 some open area is isolated from the door",
                walkable_total - reachable
            );
        }
    }
}
