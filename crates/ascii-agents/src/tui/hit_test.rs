//! Hit-test functions for mouse interaction: agent hover, coffee machine
//! click-to-open, and furniture tooltip detection.

use std::time::SystemTime;

use ascii_agents_core::walkable::OccupancyOverlay;
use ascii_agents_core::{AgentId, SceneState};

use crate::tui::layout::Layout;
use crate::tui::pathfind::Router;
use crate::tui::pet::PetKind;
use crate::tui::pixel_painter::character_anchor;
use crate::tui::pose;

/// Hit-test the mouse cursor against each agent's current sprite footprint.
/// Returns the agent under `(mx, my)` (in terminal cell coordinates), or
/// `None` if no agent occupies that cell.
///
/// The character sprite is 8×12 pixels, which in cell space is 8 cells
/// wide × 6 cells tall (one cell = 2 vertical pixels). We test against
/// that exact bounding box anchored on the agent's `character_anchor`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hit_test_agent(
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut pose::PoseHistory,
    mx: u16,
    my: u16,
) -> Option<AgentId> {
    // Width-in-cells (sprite is 8 px wide; we don't divide x by 2 because
    // each pixel column is one cell column in the half-block grid).
    const SPRITE_W_CELLS: u16 = 8;
    // Height-in-cells: sprite is 12 px tall = 6 cells.
    const SPRITE_H_CELLS: u16 = 6;
    for agent in scene.agents.values() {
        let Some(anchor) = character_anchor(agent, layout, now, router, overlay, history) else {
            continue;
        };
        let cell_x = anchor.x;
        let cell_y = anchor.y / 2;
        if mx >= cell_x
            && mx < cell_x.saturating_add(SPRITE_W_CELLS)
            && my >= cell_y
            && my < cell_y.saturating_add(SPRITE_H_CELLS)
        {
            return Some(agent.agent_id);
        }
    }
    None
}

/// Lightweight hit-test for click-to-pin without needing router/overlay state.
/// Uses home desk positions only (no walking agents).
pub fn hit_test_from_tui(scene: &SceneState, layout: &Layout, mx: u16, my: u16) -> Option<AgentId> {
    const SPRITE_W: u16 = 8;
    const SPRITE_H_CELLS: u16 = 6;
    for agent in scene.agents.values() {
        if agent.desk_index >= layout.home_desks.len() {
            continue;
        }
        let desk = &layout.home_desks[agent.desk_index];
        let ax = desk.x + 1;
        let ay = desk.y.saturating_sub(4);
        let cell_x = ax;
        let cell_y = ay / 2;
        if mx >= cell_x
            && mx < cell_x.saturating_add(SPRITE_W)
            && my >= cell_y
            && my < cell_y.saturating_add(SPRITE_H_CELLS)
        {
            return Some(agent.agent_id);
        }
    }
    None
}

/// Hit-test whether the mouse is over the pantry coffee machine.
/// Returns true if `(mx, my)` (terminal cell coords) falls on the coffee
/// machine section of the pantry counter sprite.
pub fn hit_test_coffee_machine(layout: &Layout, mx: u16, my: u16) -> bool {
    let pantry_wp = layout
        .waypoints
        .iter()
        .find(|w| matches!(w.kind, crate::tui::layout::WaypointKind::Pantry));
    let Some(wp) = pantry_wp else {
        return false;
    };
    let (cw, ch) = layout.pantry_counter_size;
    let sprite_x = wp.pos.x.saturating_sub(cw / 2);
    let sprite_y = wp.pos.y.saturating_sub(ch / 2);
    let (coffee_x0, coffee_x1) = if cw >= 32 {
        (sprite_x + 11, sprite_x + 18)
    } else {
        (sprite_x + 8, sprite_x + 13)
    };
    let coffee_y0 = sprite_y;
    let coffee_y1 = sprite_y + ch;
    let cell_y = my * 2;
    mx >= coffee_x0 && mx < coffee_x1 && cell_y >= coffee_y0 && cell_y < coffee_y1
}

/// Hit-test all furniture items in the office. Returns a short label
/// if `(mx, my)` (terminal cell coords) falls on any known item.
/// The coffee machine is handled separately for its click-to-open
/// behavior — this function covers the remaining decorations.
pub fn hit_test_furniture(layout: &Layout, mx: u16, my: u16) -> Option<&'static str> {
    use crate::tui::layout::{PlantKind, PodDecor, WallDecor, WaypointKind, DESK_H, DESK_W};
    let px = mx;
    let py = my * 2;

    let hit = |x: u16, y: u16, w: u16, h: u16| -> bool {
        px >= x && px < x.saturating_add(w) && py >= y && py < y.saturating_add(h)
    };

    // Home desks
    for desk in &layout.home_desks {
        if hit(desk.x, desk.y, DESK_W + 2, DESK_H) {
            return Some("Desk");
        }
    }

    // Waypoints
    for wp in &layout.waypoints {
        let (w, h) = match wp.kind {
            WaypointKind::Couch => (16, 7),
            WaypointKind::Pantry => layout.pantry_counter_size,
            WaypointKind::PhoneBooth => (6, 12),
            WaypointKind::StandingDesk => (8, 8),
            WaypointKind::VendingMachine => (4, 6),
            WaypointKind::Printer => (5, 4),
        };
        let wx = wp.pos.x.saturating_sub(w / 2);
        let wy = wp.pos.y.saturating_sub(h / 2);
        if hit(wx, wy, w, h) {
            return Some(match wp.kind {
                WaypointKind::Couch => "Lounge Sofa",
                WaypointKind::Pantry => "Pantry Counter",
                WaypointKind::PhoneBooth => "Phone Booth",
                WaypointKind::StandingDesk => "Standing Desk",
                WaypointKind::VendingMachine => "Vending Machine",
                WaypointKind::Printer => "Printer",
            });
        }
    }

    // Meeting sofas
    for sofa in &layout.meeting_sofas {
        if hit(sofa.x.saturating_sub(8), sofa.y.saturating_sub(3), 16, 7) {
            return Some("Meeting Sofa");
        }
    }

    // Meeting tables
    for t in &layout.meeting_tables {
        if hit(t.x.saturating_sub(6), t.y.saturating_sub(3), 12, 6) {
            return Some("Meeting Table");
        }
    }

    // Pantry table
    if let Some(t) = layout.pantry_table {
        if hit(t.x.saturating_sub(4), t.y.saturating_sub(2), 8, 5) {
            return Some("Pantry Table");
        }
    }

    // Pantry chairs
    for chair in &layout.pantry_chairs {
        if hit(chair.x.saturating_sub(2), chair.y.saturating_sub(2), 3, 3) {
            return Some("Chair");
        }
    }

    // Plants
    for (kind, p) in &layout.plants {
        if hit(p.x.saturating_sub(3), p.y.saturating_sub(3), 6, 6) {
            return Some(match kind {
                PlantKind::Ficus => "Ficus",
                PlantKind::Tall => "Tall Plant",
                PlantKind::Flower => "Flower Pot",
                PlantKind::Succulent => "Succulent",
            });
        }
    }

    // Floor lamp
    if let Some(lamp) = layout.floor_lamp {
        if hit(lamp.x.saturating_sub(2), lamp.y.saturating_sub(3), 4, 6) {
            return Some("Floor Lamp");
        }
    }

    // Wall decor
    for (kind, pos) in &layout.wall_decor {
        let (w, h) = match kind {
            WallDecor::Whiteboard => (14, 11),
            WallDecor::Bookshelf => (10, 8),
            WallDecor::BulletinBoard => (8, 6),
            WallDecor::ExitSign => (6, 3),
            WallDecor::MeetingScreen => (14, 12),
        };
        if hit(pos.x, pos.y, w, h) {
            return Some(match kind {
                WallDecor::Whiteboard => "Whiteboard",
                WallDecor::Bookshelf => "Bookshelf",
                WallDecor::BulletinBoard => "Bulletin Board",
                WallDecor::ExitSign => "Exit Sign",
                WallDecor::MeetingScreen => "Meeting Screen",
            });
        }
    }

    // Pod decor (aisle items)
    for (kind, pos) in &layout.pod_decor {
        let (w, h) = kind.size();
        if hit(
            pos.x.saturating_sub(w / 2),
            pos.y.saturating_sub(h / 2),
            w,
            h,
        ) {
            return Some(match kind {
                PodDecor::PlantTall => "Tall Plant",
                PodDecor::Whiteboard => "Whiteboard",
                PodDecor::Tv => "TV Stand",
                PodDecor::PhoneBooth => "Phone Booth",
                PodDecor::StandingDesk => "Standing Desk",
            });
        }
    }

    // Lounge side table
    if let Some(t) = layout.lounge_side_table {
        if hit(t.x.saturating_sub(3), t.y.saturating_sub(2), 7, 4) {
            return Some("Side Table");
        }
    }

    // Meeting room procedural items (coat rack, doormat)
    if let Some(mr) = layout.meeting_room {
        if mr.width > 20 {
            let cx = mr.x + mr.width - 5;
            let cy = mr.y + mr.height / 2 - 4;
            if hit(cx.saturating_sub(2), cy, 5, 8) {
                return Some("Coat Rack");
            }
        }
        if mr.width > 10 {
            let mat_x = mr.x + mr.width + 1;
            let mat_y = mr.y + mr.height / 2 - 2;
            if hit(mat_x, mat_y, 4, 5) {
                return Some("Doormat");
            }
        }
    }

    // Pantry room procedural items (water cooler, trash bin)
    if let Some(pr) = layout.pantry_room {
        if pr.height > 25 && pr.width > 12 {
            let wx = pr.x + pr.width - 6;
            let wy = pr.y + 8;
            if hit(wx, wy, 3, 6) {
                return Some("Water Cooler");
            }
        }
        if pr.height > 20 {
            let tx = pr.x + 3;
            let ty = pr.y + pr.height - 14;
            if hit(tx, ty, 4, 5) {
                return Some("Trash Bin");
            }
        }
    }

    // Door / elevator (16×14 sprite)
    if let Some(d) = layout.door {
        if hit(d.x, d.y, 16, 14) {
            return Some("Elevator");
        }
    }

    None
}

/// Hit-test whether the mouse is over the office pet.
/// `pet_pos` is the pet's center anchor in pixel coordinates.
/// `kind` selects the species; `anim_name` selects the bounding box size
/// via `PetKind::hitbox`.
///
/// Returns true if `(mx, my)` (terminal cell coords) falls inside
/// the sprite's footprint.
pub fn hit_test_pet(
    kind: PetKind,
    pet_pos: crate::tui::layout::Point,
    anim_name: &str,
    mx: u16,
    my: u16,
) -> bool {
    let (w, h) = kind.hitbox(anim_name);
    let tl_x = pet_pos.x.saturating_sub(w / 2);
    let tl_y = pet_pos.y.saturating_sub(h / 2);
    let cell_y = my * 2;
    mx >= tl_x && mx < tl_x.saturating_add(w) && cell_y >= tl_y && cell_y < tl_y.saturating_add(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coffee_machine_hit_test_returns_false_for_origin() {
        let layout = Layout::compute(160, 200, 4).expect("layout");
        assert!(!hit_test_coffee_machine(&layout, 0, 0));
    }

    #[test]
    fn coffee_machine_hit_test_returns_true_for_machine_area() {
        let layout = Layout::compute(160, 200, 4).expect("layout");
        let pantry_wp = layout
            .waypoints
            .iter()
            .find(|w| w.kind == crate::tui::layout::WaypointKind::Pantry)
            .expect("pantry");
        let (cw, ch) = layout.pantry_counter_size;
        let sprite_x = pantry_wp.pos.x.saturating_sub(cw / 2);
        let sprite_y = pantry_wp.pos.y.saturating_sub(ch / 2);
        let mid_x = if cw >= 32 {
            sprite_x + 14
        } else {
            sprite_x + 10
        };
        let mid_cell_y = (sprite_y + ch / 2) / 2;
        assert!(
            hit_test_coffee_machine(&layout, mid_x, mid_cell_y),
            "expected hit at coffee machine area ({mid_x}, {mid_cell_y})"
        );
    }

    #[test]
    fn furniture_hit_test_returns_none_for_empty_space() {
        let layout = Layout::compute(160, 200, 4).expect("layout");
        assert_eq!(hit_test_furniture(&layout, 80, 50), None);
    }

    #[test]
    fn furniture_hit_test_finds_desk() {
        let layout = Layout::compute(160, 200, 4).expect("layout");
        let desk = layout.home_desks.first().expect("desk");
        let cell_y = (desk.y + 2) / 2;
        assert_eq!(
            hit_test_furniture(&layout, desk.x + 2, cell_y),
            Some("Desk")
        );
    }

    #[test]
    fn furniture_hit_test_finds_elevator() {
        let layout = Layout::compute(160, 200, 4).expect("layout");
        let door = layout.door.expect("door");
        let cell_y = (door.y + 7) / 2;
        assert_eq!(
            hit_test_furniture(&layout, door.x + 8, cell_y),
            Some("Elevator")
        );
    }

    #[test]
    fn furniture_hit_test_finds_meeting_table() {
        let layout = Layout::compute(160, 200, 4).expect("layout");
        let table = layout.meeting_tables.first().expect("table");
        let cell_y = table.y / 2;
        assert_eq!(
            hit_test_furniture(&layout, table.x, cell_y),
            Some("Meeting Table")
        );
    }

    #[test]
    fn furniture_hit_test_respects_floor_seed() {
        let layout1 = Layout::compute_with_seed(160, 200, 4, 1).expect("layout");
        assert!(layout1.meeting_tables.is_empty());
        let layout0 = Layout::compute(160, 200, 4).expect("layout");
        if let Some(table) = layout0.meeting_tables.first() {
            let cell_y = table.y / 2;
            assert_ne!(
                hit_test_furniture(&layout1, table.x, cell_y),
                Some("Meeting Table"),
            );
        }
    }

    #[test]
    fn cat_hit_test_inside_sit_sprite() {
        use crate::tui::layout::Point;
        // cat_sit is 6x6. Center at (50, 80).
        // Top-left pixel: (50-3, 80-3) = (47, 77).
        // cell_y for my=39 → 78, which is inside [77..83).
        // mx=50 inside [47..53).
        let pos = Point { x: 50, y: 80 };
        assert!(hit_test_pet(PetKind::Cat, pos, "cat_sit", 50, 39));
    }

    #[test]
    fn cat_hit_test_outside_returns_false() {
        use crate::tui::layout::Point;
        let pos = Point { x: 50, y: 80 };
        // Way outside the 6x6 sprite.
        assert!(!hit_test_pet(PetKind::Cat, pos, "cat_sit", 10, 10));
    }

    #[test]
    fn cat_hit_test_sleep_smaller_box() {
        use crate::tui::layout::Point;
        // cat_sleep is 6x4. Center at (50, 80).
        // Top-left: (47, 78). Bottom-right: (53, 82).
        let pos = Point { x: 50, y: 80 };
        // cell_y for my=41 → 82, which is at the boundary (82 >= 82 is false for < check).
        // Actually wait: tl_y = 80 - 2 = 78, h=4 so range is [78..82). cell_y=82 is OUT.
        assert!(!hit_test_pet(PetKind::Cat, pos, "cat_sleep", 50, 41));
        // cell_y for my=40 → 80, inside [78..82).
        assert!(hit_test_pet(PetKind::Cat, pos, "cat_sleep", 50, 40));
    }
}
