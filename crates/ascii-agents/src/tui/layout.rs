//! Zone-based scene layout for the top-down office.
//!
//! Splits a buf-pixel rectangle into three vertical bands (cubicle, walkway,
//! lounge), then computes one home-desk position per agent inside the cubicle
//! band and a fixed set of named waypoints inside the lounge band. Pure
//! function — no I/O, no time, no buffer.

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

/// Kind of a lounge waypoint — determines what pose an Idle agent strikes
/// when they arrive there. Plants are pure decor, not waypoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaypointKind {
    Couch,
    Coffee,
    WaterCooler,
}

/// Wall-mounted / wall-leaning furniture, painted as decor in the top wall
/// area. Not a wander destination — agents can't walk through their own
/// cubicle row to reach the back wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WallDecor {
    Bookshelf,
    Whiteboard,
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
pub struct Layout {
    pub buf_w: u16,
    pub buf_h: u16,
    pub cubicle_band: Rect,
    pub walkway: Rect,
    pub lounge_band: Rect,
    pub home_desks: Vec<Point>,
    pub waypoints: Vec<Waypoint>,
    /// Fixed plant positions in the lounge band. Pure decor, not wander
    /// destinations. Centers (the renderer offsets by half plant size).
    pub plants: Vec<(PlantKind, Point)>,
    /// Furniture leaning against the back wall — painted into the top
    /// margin under the window band. Decor only.
    pub wall_decor: Vec<(WallDecor, Point)>,
    /// Floor lamp position in the lounge corner. Decor only.
    pub floor_lamp: Option<Point>,
}

pub const WAYPOINT_COUNT: usize = 3;
pub const DESK_W: u16 = 12;
pub const DESK_H: u16 = 6;
/// Horizontal gap between cubicles. Wider than the previous 2 px so neighbor
/// desks read as distinct cubicles rather than a single long brown bar.
pub const DESK_GAP_X: u16 = 6;
/// Vertical gap between cubicle rows. Sized to clear the seated sprite's
/// 8 px head-above-desk so row N+1's desk doesn't paint over row N's character.
pub const DESK_GAP_Y: u16 = 10;
/// Vertical reserve above the cubicle band, in buf pixels. The renderer paints
/// the top wall band (14 px tall, with windows + a clock) into this region.
/// Sized so a standing character (12 px tall) anchored at desk.y - 12 sits
/// comfortably below the wall trim line.
pub const TOP_MARGIN_PX: u16 = 28;

impl Layout {
    /// Returns `None` if the buffer is too small for even one cubicle and the
    /// fixed lounge area. Caller should paint a "terminal too small" message.
    pub fn compute(buf_w: u16, buf_h: u16, num_agents: usize) -> Option<Self> {
        const MIN_W: u16 = DESK_W + DESK_GAP_X * 2;
        const MIN_H: u16 = 40 + TOP_MARGIN_PX;
        if buf_w < MIN_W || buf_h < MIN_H {
            return None;
        }

        // Vertical split: TOP_MARGIN_PX reserved for the wall band, then the
        // remaining height splits 50/15/35 between cubicles / walkway / lounge.
        let usable_h = buf_h - TOP_MARGIN_PX;
        let cubicle_h = usable_h * 50 / 100;
        let walkway_h = usable_h * 15 / 100;
        let lounge_h = usable_h - cubicle_h - walkway_h;
        let cubicle_band = Rect {
            x: 0,
            y: TOP_MARGIN_PX,
            width: buf_w,
            height: cubicle_h,
        };
        let walkway = Rect {
            x: 0,
            y: TOP_MARGIN_PX + cubicle_h,
            width: buf_w,
            height: walkway_h,
        };
        let lounge_band = Rect {
            x: 0,
            y: TOP_MARGIN_PX + cubicle_h + walkway_h,
            width: buf_w,
            height: lounge_h,
        };

        // Home desks: pack into the cubicle band as a grid.
        let col_w = DESK_W + DESK_GAP_X;
        let row_h = DESK_H + DESK_GAP_Y;
        let cols = ((buf_w - DESK_GAP_X) / col_w).max(1);
        let rows = (cubicle_h / row_h).max(1);
        let max_desks = (cols * rows) as usize;
        let n = num_agents.min(max_desks);
        let mut home_desks = Vec::with_capacity(n);
        for i in 0..n {
            let r = (i as u16) / cols;
            let c = (i as u16) % cols;
            home_desks.push(Point {
                x: DESK_GAP_X + c * col_w,
                y: cubicle_band.y + DESK_GAP_Y + r * row_h,
            });
        }

        // Lounge waypoints: places agents actually walk to. Couch / coffee /
        // water cooler are the destinations. Bookshelf + whiteboard moved to
        // wall_decor — agents can't realistically walk through their own
        // cubicle row to reach the back wall.
        let wp_layout: &[(WaypointKind, u16, u16)] = &[
            // (kind, x_frac/100, y_frac/100 inside lounge band)
            (WaypointKind::Couch,       20, 35),  // center-left
            (WaypointKind::WaterCooler, 55, 75),  // center-bottom
            (WaypointKind::Coffee,      85, 30),  // right
        ];
        let waypoints: Vec<Waypoint> = wp_layout
            .iter()
            .map(|(kind, xf, yf)| Waypoint {
                pos: Point {
                    x: buf_w * xf / 100,
                    y: lounge_band.y + lounge_band.height * yf / 100,
                },
                kind: *kind,
            })
            .collect();

        // Plants scattered in the lounge — mix of types so it doesn't read
        // as one ficus copy-pasted.
        let plants: Vec<(PlantKind, Point)> = vec![
            (PlantKind::Ficus,     Point { x: buf_w * 35 / 100, y: lounge_band.y + lounge_band.height * 55 / 100 }),
            (PlantKind::Tall,      Point { x: buf_w * 10 / 100, y: lounge_band.y + lounge_band.height * 35 / 100 }),
            (PlantKind::Flower,    Point { x: buf_w * 70 / 100, y: lounge_band.y + lounge_band.height * 60 / 100 }),
            (PlantKind::Succulent, Point { x: buf_w * 95 / 100, y: lounge_band.y + lounge_band.height * 80 / 100 }),
        ];

        // Floor lamp in the right side of the lounge — adds a warm bulb
        // color that breaks up the floor.
        let floor_lamp = Some(Point {
            x: buf_w * 92 / 100,
            y: lounge_band.y + lounge_band.height * 50 / 100,
        });

        // Wall decor — bookshelf + whiteboard *leaning against* the back
        // wall. Top-down view: the back of the furniture is tucked into
        // the wall sprite, so its top rows overlap the wall band (which is
        // 0..14 px). Painted AFTER the wall so it sits in front of the
        // wall trim.
        // Bookshelf stays leaning against the back wall — wall-mounted by
        // nature. Whiteboard is a portable on-wheels stand: position it in
        // the walkway band so it reads as "rolled out for standup".
        let wall_decor = vec![
            (WallDecor::Bookshelf, Point { x: buf_w * 18 / 100, y: 6 }),
            (WallDecor::Whiteboard, Point {
                x: buf_w * 80 / 100,
                y: walkway.y.saturating_sub(4),
            }),
        ];

        Some(Self {
            buf_w,
            buf_h,
            cubicle_band,
            walkway,
            lounge_band,
            home_desks,
            waypoints,
            plants,
            wall_decor,
            floor_lamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_returns_none_when_buf_too_small() {
        assert!(Layout::compute(20, 20, 4).is_none());
    }

    #[test]
    fn compute_zones_are_ordered_top_to_bottom_and_nonoverlapping() {
        let l = Layout::compute(120, 80, 6).expect("fits");
        assert!(l.cubicle_band.y < l.walkway.y);
        assert!(l.walkway.y < l.lounge_band.y);
        let c_bot = l.cubicle_band.y + l.cubicle_band.height;
        let w_bot = l.walkway.y + l.walkway.height;
        assert!(c_bot <= l.walkway.y, "cubicle overlaps walkway");
        assert!(w_bot <= l.lounge_band.y, "walkway overlaps lounge");
    }

    #[test]
    fn compute_places_one_home_desk_per_agent() {
        let l = Layout::compute(120, 80, 5).expect("fits");
        assert_eq!(l.home_desks.len(), 5);
        for d in &l.home_desks {
            assert!(d.y >= l.cubicle_band.y);
            assert!(d.y + DESK_H <= l.cubicle_band.y + l.cubicle_band.height);
        }
    }

    #[test]
    fn compute_places_all_waypoint_kinds() {
        let l = Layout::compute(120, 96, 1).expect("fits");
        assert_eq!(l.waypoints.len(), WAYPOINT_COUNT);
        let kinds: std::collections::HashSet<_> =
            l.waypoints.iter().map(|w| w.kind).collect();
        assert!(kinds.contains(&WaypointKind::Couch));
        assert!(kinds.contains(&WaypointKind::WaterCooler));
        assert!(kinds.contains(&WaypointKind::Coffee));
        for w in &l.waypoints {
            assert!(w.pos.y >= l.lounge_band.y);
            assert!(w.pos.y < l.lounge_band.y + l.lounge_band.height);
        }
        // Waypoints should be at *different* y positions — not all in one row.
        let ys: std::collections::HashSet<_> =
            l.waypoints.iter().map(|w| w.pos.y).collect();
        assert!(ys.len() >= 2, "waypoints should be at varied y, got {ys:?}");
    }

    #[test]
    fn compute_places_bookshelf_on_wall_and_whiteboard_in_walkway() {
        let l = Layout::compute(120, 96, 1).expect("fits");
        let bookshelf = l.wall_decor.iter().find(|(k, _)| *k == WallDecor::Bookshelf);
        let whiteboard = l.wall_decor.iter().find(|(k, _)| *k == WallDecor::Whiteboard);
        assert!(bookshelf.is_some(), "missing bookshelf");
        assert!(whiteboard.is_some(), "missing whiteboard");
        // Bookshelf leans against the back wall, above the cubicle band.
        assert!(bookshelf.unwrap().1.y < l.cubicle_band.y, "bookshelf below cubicles");
        // Whiteboard is freestanding/portable, lives near the walkway band.
        assert!(
            whiteboard.unwrap().1.y > l.cubicle_band.y,
            "whiteboard should be below cubicle band"
        );
    }

    #[test]
    fn compute_places_plants_in_lounge() {
        let l = Layout::compute(120, 96, 1).expect("fits");
        assert!(!l.plants.is_empty(), "expected at least one plant");
        for (_, p) in &l.plants {
            assert!(p.y >= l.lounge_band.y);
            assert!(p.y < l.lounge_band.y + l.lounge_band.height);
            assert!(p.x < l.buf_w);
        }
        // Plant variety: more than one kind in the lounge.
        let kinds: std::collections::HashSet<_> =
            l.plants.iter().map(|(k, _)| *k).collect();
        assert!(kinds.len() >= 2, "expected plant variety, got {kinds:?}");
    }

    #[test]
    fn compute_truncates_home_desks_when_more_agents_than_fit() {
        // 30 cells wide buffer, DESK_W=12 + GAP=4 = 16 per column → 1 col.
        let l = Layout::compute(30, 80, 20).expect("fits");
        assert!(l.home_desks.len() < 20, "should clamp to what fits");
        assert!(!l.home_desks.is_empty(), "should fit at least 1");
    }
}
