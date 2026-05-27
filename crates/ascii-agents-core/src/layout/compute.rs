//! Layout computation helpers — extracted from mod.rs for file size.
//! All functions here are `pub(super)` so the parent module can call them
//! from `SceneLayout` impl methods.

use super::mask;
use super::*;

pub(super) fn compute_with_seed(
    buf_w: u16,
    buf_h: u16,
    num_agents: usize,
    floor_seed: u64,
) -> Option<SceneLayout> {
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
    let floor_variant = ((floor_seed.wrapping_mul(0x517cc1b727220a95)) % 5) as u8;

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

    let home_desks = compute_pod_desks(
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

    let pod_decor = compute_pod_decor(
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
        let south_y = (mr.y + mr.height * 80 / 100).min(mr.y + mr.height.saturating_sub(SOFA_H));
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
        let south2 = (mr2.y + mr2.height * 80 / 100).min(mr2.y + mr2.height.saturating_sub(SOFA_H));
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

    let room_walls = compute_room_walls(
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

    let waypoints = compute_waypoints(
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
                x: mr.x + (mr.width / 2).saturating_sub(7),
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

    Some(SceneLayout {
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
pub(super) fn compute_pod_desks(
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
            let pod_origin_y =
                cubicle_band.y + INTER_POD_AISLE_Y / 2 + couch_to_desk_extra + pod_r * pod_stride_y;
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
            let pod_origin_y =
                cubicle_band.y + INTER_POD_AISLE_Y / 2 + couch_to_desk_extra + pod_r * pod_stride_y;
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
pub(super) fn compute_pod_decor(
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
            let pod_origin_y =
                cubicle_band.y + INTER_POD_AISLE_Y / 2 + couch_to_desk_extra + pod_r * pod_stride_y;
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
            let pod_origin_y =
                cubicle_band.y + INTER_POD_AISLE_Y / 2 + couch_to_desk_extra + pod_r * pod_stride_y;
            let aisle_cx = pod_origin_x + pod_w / 2;
            let aisle_cy = pod_origin_y + pod_h + INTER_POD_AISLE_Y / 2;
            push_slot(&mut pod_decor, aisle_cx, aisle_cy);
        }
    }
    pod_decor
}

/// Wall segments with door gaps for meeting/pantry rooms.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_room_walls(
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
pub(super) fn compute_waypoints(
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
