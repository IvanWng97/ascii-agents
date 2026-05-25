//! Zone-based scene layout for the top-down office — primitive geometry
//! only, no terminal deps. Computed once per (buf_w, buf_h, num_agents)
//! triple; serializable / wire-shippable for the future v2 daemon split.
//!
//! Splits a buf-pixel rectangle into quadrants (meeting / pantry /
//! cubicles / lounge), then computes per-agent home desks, named lounge
//! waypoints, decor positions, and a per-pixel walkability mask.
//!
//! Submodules:
//!   * `decor` — the four furniture/decor enums (vocabulary).
//!   * `mask`  — `build_walkable_mask`: stamps obstacles for routing.

mod decor;
mod mask;

pub use decor::{PlantKind, PodDecor, WallDecor, WaypointKind};

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
    /// wanderer destination.
    pub walkway: Bounds,
    pub home_desks: Vec<Point>,
    pub waypoints: Vec<Waypoint>,
    pub plants: Vec<(PlantKind, Point)>,
    pub wall_decor: Vec<(WallDecor, Point)>,
    /// Decor items placed in the aisles between 2×2 desk pods. Each
    /// (kind, centre-position) tuple paints its sprite centred on the
    /// point and marks it as an obstacle in the walkable mask.
    pub pod_decor: Vec<(PodDecor, Point)>,
    pub floor_lamp: Option<Point>,
    /// Lounge side table (5×3 wood + magazine) placed next to the
    /// viewing couch on the side opposite the floor lamp.
    pub lounge_side_table: Option<Point>,
    pub door: Option<Point>,
    pub door_threshold: Option<Point>,
    pub meeting_room: Option<Bounds>,
    pub pantry_room: Option<Bounds>,
    pub meeting_sofas: Vec<Point>,
    pub meeting_tables: Vec<Point>,
    pub room_walls: Vec<(Point, Point)>,
    pub top_margin: u16,
    pub pantry_table: Option<Point>,
    pub pantry_chairs: Vec<Point>,
    /// Footprint (width, height) of the pantry counter sprite. (32, 10)
    /// when the pantry is large enough for the detailed kitchen run;
    /// (20, 8) fallback for narrow terminals where the wide sprite
    /// wouldn't fit. The renderer reads this to pick which sprite to
    /// paint (`pantry` vs `pantry_small`).
    pub pantry_counter_size: (u16, u16),
    pub corridor: Option<Bounds>,
    pub walkable: WalkableMask,
}

/// Padding (in pixels) added around every obstacle when building the
/// walkable mask. Reserves a buffer zone so characters route AROUND
/// furniture rather than scraping along its edge.
pub const OBSTACLE_PAD_PX: u16 = 2;

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
const MIN_DUAL_MEETING_H: u16 = 80;

/// Number of desks per side in a pod (`POD_SIDE * POD_SIDE` total).
pub const POD_SIDE: u16 = 2;
/// Gap between two desks inside the same pod — big enough that each
/// desk reads as its own workstation (chair + monitor + space), not
/// a merged blob. 12 px ≈ a full desk width of empty floor between
/// pod-mates.
pub const INTRA_POD_GAP_X: u16 = 12;
pub const INTRA_POD_GAP_Y: u16 = 12;
/// Gap between adjacent pods — comfortably wider than the intra-pod
/// gap so the pod boundary is visually obvious. 28 px also fits the
/// rolling whiteboard (14 wide) with ~7 px of walking clearance on
/// each side after the 1-px obstacle pad.
pub const INTER_POD_AISLE_X: u16 = 28;
pub const INTER_POD_AISLE_Y: u16 = 28;

impl SceneLayout {
    /// Returns `None` if the buffer is too small for even one cubicle and the
    /// fixed lounge area. Caller should paint a "terminal too small" message.
    pub fn compute(buf_w: u16, buf_h: u16, num_agents: usize) -> Option<Self> {
        Self::compute_with_seed(buf_w, buf_h, num_agents, 0)
    }

    pub fn compute_with_seed(
        buf_w: u16,
        buf_h: u16,
        num_agents: usize,
        floor_seed: u64,
    ) -> Option<Self> {
        const MIN_W: u16 = DESK_W + DESK_GAP_X * 2;
        let min_h: u16 = 40 + MIN_TOP_MARGIN;
        if buf_w < MIN_W || buf_h < min_h {
            return None;
        }

        let top_margin = (buf_h * 30 / 100).max(MIN_TOP_MARGIN);
        let usable_h = buf_h - top_margin;

        // Per-floor layout variant: floor_seed encodes floor_idx via
        // wrapping_mul, so floor_idx = 0 gives seed=0 (F1), etc.
        // We derive a stable floor index from the seed for variant selection.
        let floor_variant = (floor_seed % 5) as u8;

        // F1(0): Standard — meeting + pantry, vertical wall between them
        //        and the cubicle area, horizontal wall between meeting/pantry.
        // F2(1): Open plan — pantry only, no vertical wall (open kitchen
        //        corner, counter acts as divider). No meeting room.
        // F3(2): Dense — two meeting rooms (top + bottom), no pantry.
        //        Horizontal wall separates the two rooms. Each gets a door.
        // F4(3): Senior — larger meeting + pantry (like Standard but wider).
        // F5(4): Lounge — pantry only, no vertical wall (open break area).
        let (mid_x, has_meeting, has_pantry) = match floor_variant {
            0 => (buf_w * 28 / 100, true, true),
            1 => (buf_w * 18 / 100, false, true),
            2 => (buf_w * 22 / 100, true, false),
            3 => (buf_w * 35 / 100, true, true),
            _ => (buf_w * 22 / 100, false, true),
        };
        // Open-plan floors (1, 4) have no vertical wall — the pantry
        // counter/furniture visually defines the zone boundary.
        let has_vertical_wall = has_meeting;
        // Dense floor (variant 2): two meeting rooms stacked vertically.
        // Only when tall enough for two rooms with furniture + door gaps.
        let has_dual_meeting = floor_variant == 2 && usable_h >= MIN_DUAL_MEETING_H;

        let mid_y_split = top_margin + usable_h / 2;

        let meeting_room = if has_meeting {
            Some(Bounds {
                x: 0,
                y: top_margin,
                width: mid_x,
                height: if has_pantry || has_dual_meeting {
                    usable_h / 2
                } else {
                    usable_h
                },
            })
        } else {
            None
        };
        // Second meeting room for dense layout (below the first).
        let meeting_room_2 = if has_dual_meeting {
            Some(Bounds {
                x: 0,
                y: mid_y_split,
                width: mid_x,
                height: usable_h - usable_h / 2,
            })
        } else {
            None
        };
        let pantry_room = if has_pantry {
            Some(Bounds {
                x: 0,
                y: if has_meeting { mid_y_split } else { top_margin },
                width: mid_x,
                height: if has_meeting {
                    usable_h - usable_h / 2
                } else {
                    usable_h
                },
            })
        } else {
            None
        };

        let right_x = mid_x + 1;
        let right_w = buf_w.saturating_sub(right_x);
        let walkway_h = (usable_h / 10).max(8);
        let cubicle_h = usable_h.saturating_sub(walkway_h);
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

        // 2×2 desk pods. Within a pod desks are tight (small intra-gap);
        // between pods we leave a wide aisle for decor + walkers. This
        // breaks the previously-uniform desk grid into team-like
        // clusters and frees up `pod_decor` slots in the aisles.
        let pod_w = POD_SIDE * DESK_W + (POD_SIDE - 1) * INTRA_POD_GAP_X;
        let pod_h = POD_SIDE * DESK_H + (POD_SIDE - 1) * INTRA_POD_GAP_Y;
        let pod_stride_x = pod_w + INTER_POD_AISLE_X;
        let pod_stride_y = pod_h + INTER_POD_AISLE_Y;
        // Extra padding between the viewing couch (top of cubicle area)
        // and the first row of pods. Scales with buf_h so taller
        // terminals get more breathing room.
        let couch_to_desk_extra = buf_h.saturating_sub(60) / 20;
        let pod_cols = ((right_w.saturating_sub(INTER_POD_AISLE_X / 2)) / pod_stride_x).max(1);
        let pod_rows = {
            let raw = ((cubicle_h.saturating_sub(couch_to_desk_extra) + INTER_POD_AISLE_Y)
                / pod_stride_y)
                .max(1);
            let max_pods = MAX_VISIBLE_DESKS as u16 / (POD_SIDE * POD_SIDE);
            let total_pods = (pod_cols * raw).min(max_pods);
            total_pods.div_ceil(pod_cols).min(raw)
        };

        let home_desks = Self::compute_pod_desks(
            num_agents,
            &cubicle_band,
            right_x,
            right_w,
            cubicle_h,
            pod_cols,
            pod_rows,
            pod_stride_x,
            pod_stride_y,
            couch_to_desk_extra,
        );

        let pod_decor = Self::compute_pod_decor(
            &cubicle_band,
            right_x,
            pod_w,
            pod_h,
            pod_cols,
            pod_rows,
            pod_stride_x,
            pod_stride_y,
            couch_to_desk_extra,
            floor_seed,
        );

        const SOFA_H: u16 = 7;
        let mut meeting_sofas = if let Some(mr) = meeting_room {
            let cx = mr.x + mr.width / 2;
            let south_y =
                (mr.y + mr.height * 80 / 100).min(mr.y + mr.height.saturating_sub(SOFA_H));
            vec![
                Point {
                    x: cx,
                    y: mr.y + mr.height * 30 / 100,
                },
                Point { x: cx, y: south_y },
            ]
        } else {
            vec![]
        };
        let mut meeting_table_vec: Vec<Point> = meeting_room
            .map(|mr| Point {
                x: mr.x + mr.width / 2,
                y: mr.y + mr.height / 2,
            })
            .into_iter()
            .collect();
        // Second meeting room furniture (dense layout).
        if let Some(mr2) = meeting_room_2 {
            let cx2 = mr2.x + mr2.width / 2;
            let south2 =
                (mr2.y + mr2.height * 80 / 100).min(mr2.y + mr2.height.saturating_sub(SOFA_H));
            meeting_sofas.push(Point {
                x: cx2,
                y: mr2.y + mr2.height * 30 / 100,
            });
            meeting_sofas.push(Point { x: cx2, y: south2 });
            meeting_table_vec.push(Point {
                x: mr2.x + mr2.width / 2,
                y: mr2.y + mr2.height / 2,
            });
        }
        let meeting_tables = meeting_table_vec;

        let room_walls = Self::compute_room_walls(
            has_vertical_wall,
            has_dual_meeting,
            has_meeting,
            has_pantry,
            mid_x,
            mid_y_split,
            top_margin,
            usable_h,
        );

        // Counter footprint depends on pantry width — 32×10 detailed
        // kitchen on default terminals, 20×8 compact fallback for narrow
        // ones. The threshold (36 = 32 sprite + 4 px margins) keeps the
        // walkable strip around the counter wide enough for routing.
        let pantry_counter_size: (u16, u16) = match pantry_room {
            Some(pr) if pr.width >= 36 => (32, 10),
            _ => (20, 8),
        };

        let couch_y = top_margin + 3;
        let couch_x = cubicle_band.x + cubicle_band.width * 35 / 100;

        let waypoints = Self::compute_waypoints(
            &cubicle_band,
            top_margin,
            pantry_room,
            pantry_counter_size,
            &pod_decor,
            &walkway,
            right_x,
            right_w,
        );

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
        // Two meeting-room corner plants on the west wall, well clear of
        // the door (which is on the east wall) and the central
        // sofa/table column. Only added when the meeting room is large
        // enough (≥ 30 px wide) that the plant + pad doesn't squeeze the
        // walkable strip below routable width.
        .chain(meeting_room.into_iter().flat_map(|mr| {
            if mr.width < 30 || mr.height < 30 {
                Vec::new()
            } else {
                vec![
                    (
                        PlantKind::Tall,
                        Point {
                            x: mr.x + 5,
                            y: mr.y + 6,
                        },
                    ),
                    (
                        PlantKind::Flower,
                        Point {
                            x: mr.x + 5,
                            y: mr.y + mr.height.saturating_sub(7),
                        },
                    ),
                ]
            }
        }))
        .collect();

        // Floor lamp now sits right next to the viewing couch so its halo
        // bathes the seating area at night.
        let floor_lamp = Some(Point {
            x: couch_x + 9,
            y: couch_y + 2,
        });

        // Lounge side table on the OPPOSITE side from the floor lamp
        // (west of the couch). 5×3 small wood block with a magazine
        // on top.
        let lounge_side_table = Some(Point {
            x: couch_x.saturating_sub(10),
            y: couch_y + 2,
        });

        // Elevator door — 16×14 sprite mounted in the back wall, slotted
        // into the rightmost window position and BOTTOM-aligned with the
        // floor-to-ceiling windows so both sit on the same wall plane.
        // Windows span y=1 to y=top_wall_h-3 inside the wall band; the
        // elevator's bottom row lands at that same y. (`top_wall_h =
        // top_margin - 4` per the renderer's pre-pass; replicated here
        // so the layout owns the geometry.) Requires ≥ 20 px of width
        // to even fit the sprite + margin.
        const ELEVATOR_W: u16 = 16;
        const ELEVATOR_H: u16 = 14;
        let top_wall_h = top_margin.saturating_sub(4);
        let window_bottom_y = top_wall_h.saturating_sub(3); // matches paint_floor_and_walls' window_h
        let door = if buf_w >= ELEVATOR_W + 4 && window_bottom_y + 1 >= ELEVATOR_H {
            Some(Point {
                x: buf_w.saturating_sub(ELEVATOR_W + 2),
                // +2 nudge: drops the elevator bottom 2 px below the
                // window line so it visually rests against the floor
                // instead of floating mid-wall.
                y: window_bottom_y + 1 - ELEVATOR_H + 2,
            })
        } else {
            None
        };
        // Spawn point on the floor right outside the elevator's centre:
        // characters walk from here to their desk. Y is 4 px south of
        // the wall edge so the character clears the elevator threshold
        // before pathing.
        let door_threshold = door.map(|d| Point {
            x: d.x + ELEVATOR_W / 2,
            y: top_margin + 4,
        });

        // Wall decor anchored to the BOTTOM of the wall band so the sprites
        // sit "below the windows" no matter how tall the wall band grows.
        // Hardcoded y=6/8 (like the old code) leaves bookshelf + bulletin
        // floating in the sky on tall terminals where the window glass
        // auto-stretches into the wall band.
        //
        // Sprite heights:
        //   bookshelf:      12 px
        //   bulletin_board: 6 px
        //   exit_sign:      ~6 px (already used top_margin - 13 — kept)
        // We position the TOP-LEFT corner of each sprite so its bottom
        // row lands exactly at `top_margin - 1` (last wall band row).
        let mut wall_decor = vec![
            (
                WallDecor::Bookshelf,
                Point {
                    x: buf_w * 18 / 100,
                    y: top_margin.saturating_sub(12),
                },
            ),
            (
                WallDecor::ExitSign,
                Point {
                    x: buf_w.saturating_sub(9),
                    y: top_margin.saturating_sub(13),
                },
            ),
        ];
        if has_meeting || has_pantry {
            wall_decor.push((
                WallDecor::Whiteboard,
                Point {
                    x: mid_x + 3,
                    y: top_margin + usable_h / 3,
                },
            ));
        }
        if let Some(mr) = meeting_room {
            wall_decor.push((
                WallDecor::MeetingScreen,
                Point {
                    x: mr.x + mr.width / 2 - 7,
                    y: top_margin.saturating_sub(12),
                },
            ));
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

        let walkable = mask::build_walkable_mask(
            buf_w,
            buf_h,
            top_margin,
            door,
            &home_desks,
            &meeting_sofas,
            &meeting_tables,
            pantry_table,
            &pantry_chairs,
            &waypoints,
            &plants,
            floor_lamp,
            lounge_side_table,
            &wall_decor,
            &pod_decor,
            &room_walls,
            pantry_counter_size,
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
            pod_decor,
            floor_lamp,
            lounge_side_table,
            door,
            door_threshold,
            meeting_room,
            pantry_room,
            meeting_sofas,
            meeting_tables,
            room_walls,
            top_margin,
            pantry_table,
            pantry_chairs,
            pantry_counter_size,
            corridor,
            walkable,
        })
    }

    /// Pod-grid desk placement: full pods, partial columns at right edge,
    /// partial row at bottom edge.
    #[allow(clippy::too_many_arguments)]
    fn compute_pod_desks(
        num_agents: usize,
        cubicle_band: &Bounds,
        right_x: u16,
        right_w: u16,
        cubicle_h: u16,
        pod_cols: u16,
        pod_rows: u16,
        pod_stride_x: u16,
        pod_stride_y: u16,
        couch_to_desk_extra: u16,
    ) -> Vec<Point> {
        let n = num_agents.min(MAX_VISIBLE_DESKS);
        let mut home_desks = Vec::with_capacity(n);
        // Clamp: a desk must fit entirely inside the cubicle band.
        // Without this, the last intra-pod row of a bottom pod can
        // extend past cubicle_band into the walkway (the pod_rows
        // formula counts strides between origins but not the final
        // pod's tail height).
        let desk_y_max = cubicle_band.y + cubicle_band.height - DESK_H;
        let push_desk = |desks: &mut Vec<Point>, x: u16, y: u16| -> bool {
            if desks.len() >= n || y > desk_y_max {
                return desks.len() >= n;
            }
            desks.push(Point { x, y });
            false
        };

        // Full pods (row-major fill).
        'outer: for pod_r in 0..pod_rows {
            for pod_c in 0..pod_cols {
                let pod_origin_x = right_x + INTER_POD_AISLE_X / 2 + pod_c * pod_stride_x;
                let pod_origin_y = cubicle_band.y
                    + INTER_POD_AISLE_Y / 2
                    + couch_to_desk_extra
                    + pod_r * pod_stride_y;
                for r in 0..POD_SIDE {
                    for c in 0..POD_SIDE {
                        let full = push_desk(
                            &mut home_desks,
                            pod_origin_x + c * (DESK_W + INTRA_POD_GAP_X),
                            pod_origin_y + r * (DESK_H + INTRA_POD_GAP_Y),
                        );
                        if full {
                            break 'outer;
                        }
                    }
                }
            }
        }

        // Partial pod columns at the RIGHT edge — for each leftover
        // strip after `pod_cols` full pods wide enough for a single
        // desk column + half-aisle, append another 1×POD_SIDE partial
        // column. Resolves the "office looks empty on the right" issue
        // at wide buffers where a full 2nd pod doesn't fit but multiple
        // single-desk columns do.
        let main_pod_used_w = INTER_POD_AISLE_X / 2 + pod_cols * pod_stride_x;
        let residual_w = right_w.saturating_sub(main_pod_used_w);
        let partial_col_stride = DESK_W + INTER_POD_AISLE_X / 2;
        let partial_col_count = (residual_w / partial_col_stride).min(4);
        let partial_col_at_right = partial_col_count > 0;
        let partial_col_x = |i: u16| -> u16 {
            right_x + main_pod_used_w + INTER_POD_AISLE_X / 2 + i * partial_col_stride
        };
        if partial_col_at_right {
            'partial_x: for pod_r in 0..pod_rows {
                let pod_origin_y = cubicle_band.y
                    + INTER_POD_AISLE_Y / 2
                    + couch_to_desk_extra
                    + pod_r * pod_stride_y;
                for r in 0..POD_SIDE {
                    for i in 0..partial_col_count {
                        let full = push_desk(
                            &mut home_desks,
                            partial_col_x(i),
                            pod_origin_y + r * (DESK_H + INTRA_POD_GAP_Y),
                        );
                        if full {
                            break 'partial_x;
                        }
                    }
                }
            }
        }

        // Partial pod ROW at the BOTTOM edge — same idea but vertical.
        // Adds POD_SIDE × pod_cols extra desks (+ the partial column's
        // single desk if it also fits).
        let main_pod_used_h = INTER_POD_AISLE_Y / 2 + couch_to_desk_extra + pod_rows * pod_stride_y;
        let residual_h = cubicle_h.saturating_sub(main_pod_used_h);
        let partial_row_at_bottom = residual_h >= DESK_H + INTER_POD_AISLE_Y / 2;
        if partial_row_at_bottom {
            let partial_y = cubicle_band.y + main_pod_used_h + INTER_POD_AISLE_Y / 2;
            'partial_y: for pod_c in 0..pod_cols {
                let pod_origin_x = right_x + INTER_POD_AISLE_X / 2 + pod_c * pod_stride_x;
                for c in 0..POD_SIDE {
                    let full = push_desk(
                        &mut home_desks,
                        pod_origin_x + c * (DESK_W + INTRA_POD_GAP_X),
                        partial_y,
                    );
                    if full {
                        break 'partial_y;
                    }
                }
            }
            for i in 0..partial_col_count {
                let full = push_desk(&mut home_desks, partial_col_x(i), partial_y);
                if full {
                    break;
                }
            }
        }

        home_desks
    }

    /// Decor items placed in aisles between 2x2 desk pods.
    #[allow(clippy::too_many_arguments)]
    fn compute_pod_decor(
        cubicle_band: &Bounds,
        right_x: u16,
        pod_w: u16,
        pod_h: u16,
        pod_cols: u16,
        pod_rows: u16,
        pod_stride_x: u16,
        pod_stride_y: u16,
        couch_to_desk_extra: u16,
        floor_seed: u64,
    ) -> Vec<(PodDecor, Point)> {
        let mut pod_decor: Vec<(PodDecor, Point)> = Vec::new();
        // Cycle through ALL with a per-slot counter so every decor type
        // appears at least once before any repeats. Beats the prior
        // golden-ratio hash which (empirically) never picked Tv or
        // PhoneBooth at common buffer sizes — slots were stuck on
        // PlantTall / Whiteboard / StandingDesk.
        let mut slot_idx: usize = (floor_seed % 7) as usize;
        let mut push_slot = |pod_decor: &mut Vec<(PodDecor, Point)>, x: u16, y: u16| {
            let kind = PodDecor::ALL[slot_idx % PodDecor::ALL.len()];
            slot_idx += 1;
            pod_decor.push((kind, Point { x, y }));
        };
        // Vertical-aisle slots (between column pod_c and pod_c+1, one
        // per pod row).
        for pod_r in 0..pod_rows {
            for pod_c in 0..pod_cols.saturating_sub(1) {
                let pod_origin_x = right_x + INTER_POD_AISLE_X / 2 + pod_c * pod_stride_x;
                let pod_origin_y = cubicle_band.y
                    + INTER_POD_AISLE_Y / 2
                    + couch_to_desk_extra
                    + pod_r * pod_stride_y;
                let aisle_cx = pod_origin_x + pod_w + INTER_POD_AISLE_X / 2;
                let aisle_cy = pod_origin_y + pod_h / 2;
                push_slot(&mut pod_decor, aisle_cx, aisle_cy);
            }
        }
        // Horizontal-aisle slots (between row pod_r and pod_r+1, one
        // per pod column).
        for pod_r in 0..pod_rows.saturating_sub(1) {
            for pod_c in 0..pod_cols {
                let pod_origin_x = right_x + INTER_POD_AISLE_X / 2 + pod_c * pod_stride_x;
                let pod_origin_y = cubicle_band.y
                    + INTER_POD_AISLE_Y / 2
                    + couch_to_desk_extra
                    + pod_r * pod_stride_y;
                let aisle_cx = pod_origin_x + pod_w / 2;
                let aisle_cy = pod_origin_y + pod_h + INTER_POD_AISLE_Y / 2;
                push_slot(&mut pod_decor, aisle_cx, aisle_cy);
            }
        }
        pod_decor
    }

    /// Wall segments with door gaps for meeting/pantry rooms.
    #[allow(clippy::too_many_arguments)]
    fn compute_room_walls(
        has_vertical_wall: bool,
        has_dual_meeting: bool,
        has_meeting: bool,
        has_pantry: bool,
        mid_x: u16,
        mid_y_split: u16,
        top_margin: u16,
        usable_h: u16,
    ) -> Vec<(Point, Point)> {
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
        // Vertical wall: only when we have enclosed rooms (meeting or
        // meeting+pantry). Open-plan/lounge pantry-only floors skip it
        // — the counter is the visual boundary.
        if has_vertical_wall {
            let v_x = mid_x;
            let v_top = top_margin;
            let v_bot = if has_pantry || has_dual_meeting {
                mid_y_split
            } else {
                top_margin + usable_h
            };
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
            // Second meeting room or pantry below: extend wall with
            // its own door gap.
            if has_dual_meeting {
                // Second meeting room: extend wall below horizontal.
                // Start below the horizontal wall (4px thick + pad).
                let v2_top = mid_y_split + 6;
                let v2_bot = top_margin + usable_h;
                let v2_center = v2_top + (v2_bot - v2_top) / 2;
                let v2_door_top = v2_center.saturating_sub(DOOR_GAP_V / 2);
                let v2_door_bot = (v2_center + DOOR_GAP_V / 2).min(v2_bot);
                room_walls.push((
                    Point { x: v_x, y: v2_top },
                    Point {
                        x: v_x,
                        y: v2_door_top,
                    },
                ));
                room_walls.push((
                    Point {
                        x: v_x,
                        y: v2_door_bot,
                    },
                    Point { x: v_x, y: v2_bot },
                ));
            } else if !has_pantry {
                // Single meeting, no pantry, no dual: extend wall to floor
                room_walls.push((
                    Point { x: v_x, y: v_bot },
                    Point {
                        x: v_x,
                        y: top_margin + usable_h,
                    },
                ));
            }
        }
        // Horizontal wall: separates meeting from pantry, or two meetings.
        let h_y = mid_y_split;
        let h_door_center = mid_x * 60 / 100;
        let h_door_left = h_door_center.saturating_sub(DOOR_GAP_H / 2);
        let h_door_right = (h_door_center + DOOR_GAP_H / 2).min(mid_x);
        if (has_meeting && has_pantry) || has_dual_meeting {
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
        }
        room_walls
    }

    /// Waypoints: couch, pantry, pod-decor-promoted (PhoneBooth/StandingDesk),
    /// corridor appliances (VendingMachine/Printer).
    #[allow(clippy::too_many_arguments)]
    fn compute_waypoints(
        cubicle_band: &Bounds,
        top_margin: u16,
        pantry_room: Option<Bounds>,
        pantry_counter_size: (u16, u16),
        pod_decor: &[(PodDecor, Point)],
        walkway: &Bounds,
        right_x: u16,
        right_w: u16,
    ) -> Vec<Waypoint> {
        let couch_y = top_margin + 3;
        let couch_x = cubicle_band.x + cubicle_band.width * 35 / 100;
        let mut waypoints: Vec<Waypoint> = vec![Waypoint {
            pos: Point {
                x: couch_x,
                y: couch_y,
            },
            kind: WaypointKind::Couch,
        }];
        if let Some(pr) = pantry_room {
            // Clamp x so the counter fits within pantry_room. Without this
            // the counter (32px or 20px wide) extends past the east wall
            // into the cubicle band at small buffer widths.
            let half_cw = pantry_counter_size.0 / 2;
            let max_cx = pr.x + pr.width.saturating_sub(half_cw + 1);
            let (wx, wy) = if pantry_counter_size.0 >= 32 {
                (
                    (pr.x + pr.width / 2).min(max_cx),
                    pr.y + pr.height * 65 / 100,
                )
            } else {
                (
                    (pr.x + pr.width * 60 / 100).min(max_cx),
                    pr.y + pr.height * 60 / 100,
                )
            };
            waypoints.push(Waypoint {
                pos: Point { x: wx, y: wy },
                kind: WaypointKind::Pantry,
            });
        }
        // Interactive pod-aisle decor -> also waypoints. PhoneBooth and
        // StandingDesk are workstation-like destinations agents can
        // wander to during Idle cycles. Plant/Whiteboard/TV are pure
        // decor (already obstacles via pod_decor).
        for (kind, pos) in pod_decor {
            let wp_kind = match kind {
                PodDecor::PhoneBooth => Some(WaypointKind::PhoneBooth),
                PodDecor::StandingDesk => Some(WaypointKind::StandingDesk),
                _ => None,
            };
            if let Some(wp_kind) = wp_kind {
                waypoints.push(Waypoint {
                    pos: *pos,
                    kind: wp_kind,
                });
            }
        }

        // Corridor appliances — stored as centre points (same convention
        // as Pantry/Couch). Painter derives top-left via sub(w/2, h/2).
        // Sizes: vending 4×6, printer 5×4.
        if walkway.height >= 10 && walkway.width > 30 {
            waypoints.push(Waypoint {
                pos: Point {
                    x: right_x + 5,
                    y: walkway.y + 3,
                },
                kind: WaypointKind::VendingMachine,
            });
        }
        if walkway.height >= 9 && right_w > 40 {
            waypoints.push(Waypoint {
                pos: Point {
                    x: right_x + right_w.saturating_sub(10),
                    y: walkway.y + 2,
                },
                kind: WaypointKind::Printer,
            });
        }
        waypoints
    }

    pub fn is_walkable(&self, x: u16, y: u16) -> bool {
        self.walkable.is_walkable(x, y)
    }
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
        // Couch + Pantry are unconditional; PhoneBooth / StandingDesk
        // may appear depending on the random pod_decor pick — so just
        // require the unconditional pair and let the rest vary.
        assert!(l.waypoints.len() >= 2);
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
                // PhoneBooth + StandingDesk waypoints come from
                // pod_decor slots in the cubicle band. They're
                // valid anywhere inside the cubicle band — the
                // tighter check just confirms they're south of the
                // top wall.
                WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {
                    assert!(w.pos.y >= l.top_margin);
                }
                WaypointKind::VendingMachine | WaypointKind::Printer => {
                    assert!(w.pos.y >= l.top_margin);
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

    #[test]
    fn walkable_mask_connected_across_floor_seeds() {
        use std::collections::VecDeque;

        let (buf_w, buf_h, num_agents) = (160u16, 100u16, 12usize);
        for seed in 0..5u64 {
            let l = SceneLayout::compute_with_seed(buf_w, buf_h, num_agents, seed)
                .expect("layout fits");
            let w = l.buf_w as usize;
            let h = l.buf_h as usize;
            let start = l.door_threshold.expect("door_threshold");
            assert!(l.is_walkable(start.x, start.y));

            let mut visited = vec![false; w * h];
            visited[(start.y as usize) * w + (start.x as usize)] = true;
            let mut queue = VecDeque::new();
            queue.push_back((start.x, start.y));
            let mut reachable = 1usize;
            while let Some((cx, cy)) = queue.pop_front() {
                for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as u16, ny as u16);
                    let idx = (ny as usize) * w + (nx as usize);
                    if !visited[idx] && l.is_walkable(nx, ny) {
                        visited[idx] = true;
                        reachable += 1;
                        queue.push_back((nx, ny));
                    }
                }
            }
            let walkable_total = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| l.is_walkable(x as u16, y as u16))
                .count();
            assert_eq!(
                reachable,
                walkable_total,
                "seed={seed}: {buf_w}x{buf_h}: {} disconnected pixels",
                walkable_total - reachable
            );
        }
    }
}
