//! Pure-pixel paint pass — no ratatui types, no terminal I/O.
//!
//! Split from `tui/renderer.rs` to separate the pixel-painting pipeline
//! (called by any renderer impl — `TuiRenderer`, a future web canvas, PNG
//! export, GIF capture) from the ratatui-coupled half-block flush + widget
//! overlay + terminal lifecycle.
//!
//! `render_to_rgb_buffer` is the public entry point. Everything else is
//! private to this module except `character_anchor`, which `widgets.rs`
//! uses for label placement and `hit_test.rs` for mouse hit-testing.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::blit::blit_frame;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Rgb, RgbBuffer};
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::walkable::OccupancyOverlay;
use pixtuoid_core::{AgentSlot, SceneState};

use crate::tui::chitchat::{self, ActiveChitchat, ChitchatBubble};
use crate::tui::floor::LightingState;
use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{Layout, Point, DESK_W};
use crate::tui::motion::MotionState;
use crate::tui::pathfind::Router;
use crate::tui::pet::PetKind;
use crate::tui::pose::{self, Pose};

/// Result of the pure-pixel pass — carries the resolved cat position
/// (for hit-testing), active chitchat bubbles (for widget rendering),
/// and agent ids that were seen carrying coffee this frame (so the
/// caller can persist them into `coffee_holders`).
pub struct PixelPassResult {
    pub pet_pos: Option<(Point, &'static str, PetKind)>,
    pub chitchat_bubbles: Vec<ChitchatBubble>,
    /// Agent ids observed in `Walking { carrying_coffee: true }` this
    /// frame. The caller inserts them into the persistent
    /// `coffee_holders` set and records `coffee_fetched_at`.
    pub new_coffee_carriers: Vec<pixtuoid_core::AgentId>,
}

mod ambient;
mod anchors;
mod background;
mod drawable;
mod effects;
mod furniture;
mod palette;

pub(in crate::tui) use anchors::character_anchor;
pub(in crate::tui) use anchors::walking_position;
use anchors::{
    back_couch_anchor, compute_door_frame_idx, seated_anchor, standing_at_desk_anchor,
    walking_anchor, waypoint_anchor, waypoint_rank_offset_x, with_breath,
};
use background::{
    dim_floor_overlay, paint_ceiling_pool, paint_clock, paint_corridor_runner,
    paint_floor_and_walls, paint_floor_lamp_halo, paint_neon_panel, paint_shadow, time_of_day_look,
};
use drawable::{paint_drawable, pet_position, Drawable, DrawableKind};
use palette::{agent_palette, blend, recolor_frame};

const COFFEE_STEAM_WINDOW_SECS: u64 = 120;
const DOOR_SPRITE_WIDTH: u16 = 16;

// Room-divider frosted-glass partitions. The E-W (horizontal) wall shows its
// face — 6 px tall, kept in sync with `mask.rs` WALL_THICK_H — while the N-S
// (vertical) wall is seen edge-on at 3 px (wider than its 1 px footprint). The
// 2:1 ratio sells the top-down fake-3D. Each strip is a cool gradient (bright
// specular edge → tinted body → soft slate edge, all alpha-composited over
// what's behind so the room glows through) with a brighter seam every
// `GLASS_SEAM_STRIDE` px. The horizontal wall paints in the y-sorted drawable
// pass (so it composites over — frostily occluding — a walker standing behind
// it); the vertical paints in the background.
pub(super) const WALL_THICK_V_PX: u16 = 3; // visual; footprint is 1 px (mask.rs)
                                           // Derived from the core mask const so the visible glass face and the blocked
                                           // ground footprint share a single source of truth (can't drift apart).
pub(super) const WALL_THICK_H_PX: u16 = pixtuoid_core::layout::WALL_THICK_H;
const GLASS_SEAM_STRIDE: u16 = 16;
// The horizontal wall's frosted glass rises this many px NORTH of its walkable
// footprint — a "back cap" giving the wall height. Because the strip is
// y-sorted at its south (front) base, a character standing just north of the
// wall has their feet/legs composited behind this translucent cap (occluded
// behind the glass). The cap is over floor (visual only), not the mask.
//
// Sized to WALL_THICK_H so the cap reaches into the legs of a walker at the
// northmost walkable row (footprint top `W` minus OBSTACLE_PAD+1 = `W-3`): the
// 12px sprite spans `W-15..W-3`, the cap now covers `W-6..W-1`, so the bottom
// ~4px (feet + lower legs) read behind the pane. At the old value of 3 only the
// single feet row was grazed.
const GLASS_CAP_PX: u16 = 6;

fn glass_tones(theme: &crate::tui::theme::Theme) -> (Rgb, Rgb, Rgb) {
    let tl = theme.office.room_wall_trim_light;
    (
        Rgb(
            tl.0.saturating_add(125),
            tl.1.saturating_add(135),
            tl.2.saturating_add(124),
        ),
        Rgb(
            tl.0.saturating_add(70),
            tl.1.saturating_add(100),
            tl.2.saturating_add(116),
        ),
        Rgb(
            tl.0.saturating_add(18),
            tl.1.saturating_add(52),
            tl.2.saturating_add(86),
        ),
    )
}

/// Stitch a vertical (N-S) wall segment's `[y_top, y_bot]` to its joints — the
/// terminal-agnostic layout emits raw geometry; the render thicknesses/offsets
/// that open the gaps live here:
///   • Top: a segment starting at `top_margin` abuts the north wall band, which
///     ends 4 px higher at `top_wall_h` — raise it so no floor shows between
///     window and wall. A segment sitting just below a horizontal wall (the
///     dual-meeting layout offsets its lower segment ~6 px to clear the cross
///     wall — see `compute_room_walls`) is bridged up to meet it.
///   • Bottom: where the vertical meets a horizontal wall, extend it down by
///     the horizontal's thickness to fill the inside corner (else its right
///     columns leave an L-notch beside the horizontal run).
fn stitch_vertical_wall(
    start_y: u16,
    end_y: u16,
    top_margin: u16,
    top_wall_h: u16,
    h_rows: &[u16],
) -> (u16, u16) {
    let y_top = if start_y == top_margin {
        top_wall_h
    } else if let Some(&hr) = h_rows
        .iter()
        .find(|&&hr| hr < start_y && start_y - hr <= WALL_THICK_H_PX + 2)
    {
        hr
    } else {
        start_y
    };
    let y_bot = if h_rows.contains(&end_y) {
        end_y + (WALL_THICK_H_PX - 1)
    } else {
        end_y
    };
    (y_top, y_bot)
}

fn glass_over(buf: &RgbBuffer, x: u16, y: u16, g: Rgb, a: f32) -> Rgb {
    let b = buf.get(x, y);
    Rgb(blend(b.0, g.0, a), blend(b.1, g.1, a), blend(b.2, g.2, a))
}

/// Paint a horizontal (E-W) frosted-glass wall strip: lit top edge → body →
/// soft bottom edge, seam glints every `GLASS_SEAM_STRIDE` px.
pub(super) fn paint_glass_wall_h(
    buf: &mut RgbBuffer,
    theme: &crate::tui::theme::Theme,
    x0: u16,
    x1: u16,
    y_top: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width, buf.height);
    // The strip spans the back cap (rising north of the footprint) + the
    // 6 px face. Row 0 = lit far/top edge (north), last row = soft front base.
    let cap_top = y_top.saturating_sub(GLASS_CAP_PX);
    let rows = GLASS_CAP_PX + WALL_THICK_H_PX;
    for x in x0..=x1.min(bw.saturating_sub(1)) {
        let seam = (x - x0) % GLASS_SEAM_STRIDE == 0;
        for i in 0..rows {
            let y = cap_top + i;
            if y >= bh {
                continue;
            }
            let (g, a) = if seam {
                (hi, 0.55)
            } else if i == 0 {
                (hi, 0.82)
            } else if i == rows - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.58)
            };
            let color = glass_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}

/// Paint a vertical (N-S) frosted-glass wall strip: lit left edge → body →
/// soft right edge, seam glints every `GLASS_SEAM_STRIDE` px.
fn paint_glass_wall_v(
    buf: &mut RgbBuffer,
    theme: &crate::tui::theme::Theme,
    x_left: u16,
    y_top: u16,
    y_bot: u16,
) {
    let (hi, mid, lo) = glass_tones(theme);
    let (bw, bh) = (buf.width, buf.height);
    for y in y_top..=y_bot.min(bh.saturating_sub(1)) {
        let seam = (y - y_top) % GLASS_SEAM_STRIDE == 0;
        for dx in 0..WALL_THICK_V_PX {
            let x = x_left + dx;
            if x >= bw {
                continue;
            }
            let (g, a) = if seam {
                (hi, 0.6)
            } else if dx == 0 {
                (hi, 0.85)
            } else if dx == WALL_THICK_V_PX - 1 {
                (lo, 0.72)
            } else {
                (mid, 0.6)
            };
            let color = glass_over(buf, x, y, g, a);
            buf.put(x, y, color);
        }
    }
}

/// Bundled input for the pixel-painting pass. Constructed from `DrawCtx`
/// fields + per-frame inputs at the `draw_scene` call site.
pub struct PixelCtx<'a> {
    pub scene: &'a SceneState,
    pub layout: &'a Layout,
    pub pack: &'a Pack,
    pub now: SystemTime,
    pub buf: &'a mut RgbBuffer,
    pub cache: &'a mut FrameCache,
    pub router: &'a mut dyn Router,
    pub overlay: &'a mut OccupancyOverlay,
    pub history: &'a mut pose::PoseHistory,
    /// Forwarded from `DrawCtx.motion` — identical lifetime, identical
    /// borrow rules. `derive_with_routing` reads/writes per-agent entries.
    pub motion: &'a mut std::collections::HashMap<pixtuoid_core::AgentId, MotionState>,
    /// Per-floor max in-flight entry/exit physics duration (ms), forwarded
    /// from `DrawCtx.door_anim_max_ms`. Used by `compute_door_frame_idx`
    /// instead of the old hardcoded `ENTRY_ANIMATION_MS`.
    pub door_anim_max_ms: u64,
    pub theme: &'a crate::tui::theme::Theme,
    pub floor: crate::tui::floor::FloorMeta,
    pub active_pet: Option<&'a crate::tui::renderer::PetState>,
    pub floor_pet_kind: Option<PetKind>,
    pub chitchat_state: &'a mut HashMap<crate::tui::chitchat::VenueKey, ActiveChitchat>,
    pub coffee_holders: &'a std::collections::HashSet<pixtuoid_core::AgentId>,
    pub coffee_fetched_at: &'a HashMap<pixtuoid_core::AgentId, SystemTime>,
    pub coffee_stains: &'a HashMap<pixtuoid_core::AgentId, Vec<crate::tui::tui_renderer::StainPos>>,
    pub light: &'a mut crate::tui::floor::LightingState,
}

/// Paint a character at an arbitrary anchor with per-agent recolor. `flip_x`
/// mirrors the sprite horizontally — used to make walkers face the direction
/// they're moving. `glow_tint` should carry the tool-derived monitor color
/// when the character is at a lit screen (SeatedTyping); tints the skin
/// toward that color so the eye reads "the monitor is lighting their face."
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_character_at(
    buf: &mut RgbBuffer,
    anim_name: &'static str,
    frame_idx: usize,
    anchor: Point,
    agent: &AgentSlot,
    pack: &Pack,
    flip_x: bool,
    glow_tint: Option<Rgb>,
    cache: &mut FrameCache,
) {
    let Some(anim) = pack.animation(anim_name) else {
        return;
    };
    let Some(frame) = anim.frames.get(frame_idx).or_else(|| anim.frames.first()) else {
        return;
    };
    let cached = cache.get_or_make(
        agent.agent_id,
        anim_name,
        frame_idx,
        flip_x,
        glow_tint,
        || {
            let pal = agent_palette(&pack.palette, agent, glow_tint);
            let recolored = recolor_frame(frame, &pal, &pack.palette);
            if flip_x {
                recolored.mirror_horizontal()
            } else {
                recolored
            }
        },
    );
    blit_frame(cached, anchor.x, anchor.y, buf);
}

/// Sprite name + horizontal flip for an agent at a meeting slot, by facing.
/// A north-side sofa seat faces the viewer across the table (front `seated`);
/// a south-side seat faces away (back view). A meeting stand faces inward, so
/// the west-side stander (which the layout marks `Facing::East`) is mirrored.
/// Extracted so the facing→sprite mapping is unit-testable without a render.
pub(super) fn meeting_sprite(
    kind: crate::tui::layout::WaypointKind,
    facing: crate::tui::layout::Facing,
) -> (&'static str, bool) {
    use crate::tui::layout::{Facing, WaypointKind};
    match kind {
        WaypointKind::MeetingSofa => match facing {
            Facing::North => ("back_couch", false),
            _ => ("seated", false),
        },
        // Stand mirrors toward the table centre; west stand is `Facing::East`.
        WaypointKind::MeetingStand => ("standing", matches!(facing, Facing::East)),
        // Not a meeting slot — caller dispatches these directly.
        _ => ("standing", false),
    }
}

pub fn render_to_rgb_buffer(ctx: &mut PixelCtx<'_>) -> PixelPassResult {
    let agents: Vec<_> = ctx.scene.agents.values().cloned().collect();
    let buf_w = ctx.layout.buf_w;
    let buf_h = ctx.layout.buf_h;
    let mut resolved_pet_pos: Option<(Point, &'static str, PetKind)> = None;
    let mut new_coffee_carriers: Vec<pixtuoid_core::AgentId> = Vec::new();

    // Compute time-of-day once per frame and pass to every paint
    // helper that depends on it. Avoids recomputing the chrono local
    // hour for each window + ceiling pool + lamp halo.
    let look = time_of_day_look(ctx.now, ctx.theme);
    // Wall band height tracks layout.top_margin (which is buf_h/4 with
    // a floor) — leaves a 4-px buffer between wall trim and cubicles.
    let top_wall_h = ctx.layout.top_margin.saturating_sub(4);
    // The elevator door replaces the rightmost window — pass its x-range
    // so `paint_floor_and_walls` skips drawing a window that would
    // otherwise bleed through behind the elevator frame.
    let door_x_range = ctx.layout.door.map(|d| (d.x, d.x + DOOR_SPRITE_WIDTH));
    paint_floor_and_walls(
        ctx.buf,
        buf_w,
        buf_h,
        ctx.now,
        &look,
        top_wall_h,
        door_x_range,
        ctx.theme,
        ctx.floor.altitude,
    );

    // Per-floor lighting: tick the fade state with the current occupancy.
    // `indoor_scale` smoothly travels from MIN_LEVEL (empty + past
    // debounce) to 1.0 (populated). Windows/skyline are unaffected.
    let indoor_scale = ctx.light.tick(ctx.scene.agents.is_empty(), ctx.now);
    // Empty floors get an extra floor-darken boost on top of the time-of-
    // day dim — there are no monitor/lamp light sources to balance against
    // the overhead darkness, so without the boost they read as "lights
    // off but room weirdly bright."
    let min_level = LightingState::MIN_LEVEL;
    let boost_ceiling = LightingState::EMPTY_FLOOR_DIM_BOOST;
    let empty_floor_boost = 1.0 + (1.0 - indoor_scale) * (boost_ceiling - 1.0) / (1.0 - min_level);

    let dim_strength = (0.45 - ctx.floor.sunlight_boost).max(0.1);
    dim_floor_overlay(
        ctx.buf,
        top_wall_h,
        buf_h,
        look.darkness * dim_strength * empty_floor_boost,
        ctx.theme,
    );
    let pool_strength = (0.15 + 0.30 * look.darkness) * indoor_scale;
    for desk in &ctx.layout.home_desks {
        paint_ceiling_pool(
            ctx.buf,
            desk.x + DESK_W / 2,
            desk.y.saturating_sub(2),
            10,
            5,
            pool_strength,
            ctx.theme,
        );
    }
    // Two ceiling fluorescents over the pantry and a third over the
    // corridor so the floor is lit consistently with the lounge_band gone.
    if let Some(pr) = ctx.layout.pantry_room {
        paint_ceiling_pool(
            ctx.buf,
            pr.x + pr.width / 2,
            pr.y + pr.height / 2,
            12,
            6,
            pool_strength,
            ctx.theme,
        );
    }
    if let Some(corridor) = ctx.layout.corridor {
        paint_ceiling_pool(
            ctx.buf,
            corridor.x + corridor.width / 2,
            corridor.y + corridor.height / 2,
            14,
            5,
            pool_strength,
            ctx.theme,
        );
    }
    if let Some(lamp) = ctx.layout.floor_lamp {
        paint_floor_lamp_halo(
            ctx.buf,
            lamp.x,
            lamp.y,
            look.darkness * 0.55 * indoor_scale,
            ctx.theme,
        );
    }

    // Neon sign panel in the wall band — dark bg with glow border.
    // Text overlay (branding, dots, star link) is rendered by the ratatui
    // widget pass in renderer.rs::paint_wall_display.
    let neon_w = 30u16;
    let neon_h = 8u16;
    paint_neon_panel(ctx.buf, 1, 1, neon_w, neon_h, ctx.now, ctx.theme);

    // Live wall clock painted after the wall (so hands sit on top of it)
    // but before wall decor — the bookshelf etc. shouldn't cover it.
    // 7x7 sprite, center at clock_x+3; clamp so it never collides with
    // the 30-wide neon panel on the left.
    let clock_x = (buf_w / 2).saturating_sub(3).max(neon_w + 2);
    paint_clock(ctx.buf, clock_x, 1, ctx.now, ctx.theme);
    // Corridor runner — painted over the floor but BEFORE walls/decor
    // so walls cleanly overlap it where they cross.
    if let Some(corridor) = ctx.layout.corridor {
        paint_corridor_runner(ctx.buf, corridor, ctx.theme);
    }
    // Room dividers — frosted-glass partitions (see the module-level glass
    // helpers + WALL_THICK_*_PX). The VERTICAL (N-S, edge-on) wall paints here
    // in the background; the HORIZONTAL (E-W, face-on) wall is emitted into the
    // y-sorted drawable pass below so it composites over a walker standing
    // behind it. Stitch the vertical's joints (the layout emits geometry only;
    // the render thicknesses/offsets that open the gaps live here):
    //   • Top: a segment starting at top_margin abuts the north wall band,
    //     which ends 4 px higher at top_wall_h — raise it so no floor shows
    //     between window and wall. A segment just below a horizontal wall (the
    //     dual-meeting layout offsets it ~6 px to clear the cross wall) is
    //     bridged up to meet it.
    //   • Bottom: where the vertical meets a horizontal wall, extend it down by
    //     the horizontal's thickness to fill the inside corner (else its right
    //     columns leave an L-notch beside the horizontal run).
    let h_rows: Vec<u16> = ctx
        .layout
        .room_walls
        .iter()
        .filter(|(s, e)| s.y == e.y)
        .map(|(s, _)| s.y)
        .collect();
    for (start, end) in &ctx.layout.room_walls {
        if start.x != end.x {
            continue; // horizontal walls paint in the drawable pass
        }
        let (y_top, y_bot) =
            stitch_vertical_wall(start.y, end.y, ctx.layout.top_margin, top_wall_h, &h_rows);
        paint_glass_wall_v(ctx.buf, ctx.theme, start.x, y_top, y_bot.min(buf_h - 1));
    }

    // Meeting sofas + table, pantry table + chairs are all painted by
    // the y-sorted Drawable pass below (MeetingSofa / MeetingTable /
    // PantryTable / PantryChair variants). They used to be painted
    // here in the background pass too — leftover from before the
    // y-sort refactor; the duplicate paints were dead pixels
    // overwritten 50 lines later. Removed.
    //
    // Entry mat was also painted here (a small blue rug just south of
    // the door). The old wooden-door era used it to define the arrival
    // zone, but the elevator already defines that visually + the blue
    // rectangle looked out of place under the elevator.

    // Procedural room fill — small pixel items that make rooms feel lived-in.
    // Ground footprint rule: walkable mask is NOT affected by these (they're
    // small items characters can walk around or over).
    if let Some(mr) = ctx.layout.meeting_room {
        let wall_color = ctx.theme.office.room_wall_trim_dark;
        let accent = ctx.theme.furniture.rug_accent;
        let wood = ctx.theme.furniture.wood_top;
        let wood_dark = ctx.theme.furniture.wood_trim;

        // Notice board on the south wall (8×5)
        if mr.height > 20 && mr.width > 15 {
            let bx = mr.x + 4;
            let by = mr.y + mr.height - 8;
            for dy in 0..5u16 {
                for dx in 0..8u16 {
                    let px = bx + dx;
                    let py = by + dy;
                    if px < buf_w && py < buf_h {
                        let on_edge = dx == 0 || dx == 7 || dy == 0 || dy == 4;
                        ctx.buf
                            .put(px, py, if on_edge { wall_color } else { accent });
                    }
                }
            }
        }

        // Coat rack: 1px pole, 3-wide base, bright coat blobs on hooks
        if mr.width > 20 {
            let cx = mr.x + mr.width - 5;
            let cy = mr.y + mr.height / 2 - 4;
            let pole = wood_dark;
            let base = wood;
            let coats = [Rgb(200, 60, 60), Rgb(80, 120, 200), Rgb(240, 240, 240)];
            // Pole (1px wide, 8 tall)
            for dy in 0..8u16 {
                let py = cy + dy;
                if py < buf_h && cx < buf_w {
                    ctx.buf.put(cx, py, pole);
                }
            }
            // Base (3px wide)
            let by = cy + 7;
            for dx in 0..3u16 {
                let px = cx.saturating_sub(1) + dx;
                if px < buf_w && by < buf_h {
                    ctx.buf.put(px, by, base);
                }
            }
            // Coat blobs (2×2 colored blocks hanging from hooks)
            for (i, &coat_color) in coats.iter().enumerate() {
                let hook_y = cy + 1 + (i as u16) * 2;
                let side: i16 = if i % 2 == 0 { -1 } else { 1 };
                let hx = (cx as i16 + side) as u16;
                for dy in 0..2u16 {
                    for dx in 0..2u16 {
                        let px = hx.wrapping_add(if side < 0 { dx.wrapping_sub(1) } else { dx });
                        let py = hook_y + dy;
                        if px < buf_w && py < buf_h {
                            ctx.buf.put(px, py, coat_color);
                        }
                    }
                }
            }
        }

        // Small doormat at the meeting room entrance (on the cubicle side)
        if mr.width > 10 {
            let mat_x = mr.x + mr.width;
            let mat_y = mr.y + mr.height / 2 - 2;
            let mat_color = ctx.theme.furniture.rug_trim;
            let mat_accent = ctx.theme.furniture.rug_field;
            for dy in 0..5u16 {
                for dx in 0..4u16 {
                    let px = mat_x + dx + 1;
                    let py = mat_y + dy;
                    if px < buf_w && py < buf_h {
                        let on_border = dx == 0 || dx == 3 || dy == 0 || dy == 4;
                        ctx.buf
                            .put(px, py, if on_border { mat_color } else { mat_accent });
                    }
                }
            }
        }
    }
    if let Some(pr) = ctx.layout.pantry_room {
        let cooler_body = ctx.theme.office.building_light;
        let cooler_water = Rgb(100, 180, 230);

        // Water cooler near the pantry wall (3×6)
        if pr.height > 25 && pr.width > 12 {
            let wx = pr.x + pr.width - 6;
            let wy = pr.y + 8;
            for dy in 0..6u16 {
                for dx in 0..3u16 {
                    let px = wx + dx;
                    let py = wy + dy;
                    if px < buf_w && py < buf_h {
                        let color = if dy < 2 { cooler_water } else { cooler_body };
                        ctx.buf.put(px, py, color);
                    }
                }
            }
        }

        // Trash bin near the pantry counter (4×5 with visible bag liner)
        if pr.height > 20 {
            let tx = pr.x + 3;
            let ty = pr.y + pr.height - 14;
            let bin_outer = Rgb(70, 70, 78);
            let bin_rim = Rgb(100, 100, 108);
            let bag_liner = Rgb(200, 200, 210);
            let bag_fill = Rgb(160, 160, 170);
            for dy in 0..5u16 {
                for dx in 0..4u16 {
                    let px = tx + dx;
                    let py = ty + dy;
                    if px < buf_w && py < buf_h {
                        let color = if dy == 0 {
                            // Rim row — lighter metal rim with bag liner peek
                            if dx == 0 || dx == 3 {
                                bin_rim
                            } else {
                                bag_liner
                            }
                        } else if dy == 1 {
                            // Bag liner visible
                            if dx == 0 || dx == 3 {
                                bin_outer
                            } else {
                                bag_fill
                            }
                        } else {
                            // Bin body
                            bin_outer
                        };
                        ctx.buf.put(px, py, color);
                    }
                }
            }
        }
    }

    // Shadow pass — soft floor shadows under desks + lounge furniture
    // so nothing floats. Painted BEFORE the y-sorted entity pass so
    // every entity sits on top of its own shadow. Strength is a
    // function of daylight so noon shadows are crisp and night shadows
    // are subtle.
    let shadow_strength = 0.5 - 0.3 * look.darkness;
    for desk in &ctx.layout.home_desks {
        paint_shadow(
            ctx.buf,
            desk.x + DESK_W / 2,
            desk.y + 7,
            DESK_W / 2 + 1,
            3,
            shadow_strength,
            ctx.theme,
        );
    }
    for wp in &ctx.layout.waypoints {
        // Couch shadow is emitted once below (it's 3 seat waypoints; per-seat
        // shadows would overlap and darken the cushion seams).
        if wp.kind == crate::tui::layout::WaypointKind::Couch {
            continue;
        }
        paint_shadow(
            ctx.buf,
            wp.pos.x,
            wp.pos.y + 2,
            7,
            2,
            shadow_strength,
            ctx.theme,
        );
    }
    if let Some(center) = ctx.layout.couch_sprite_center {
        paint_shadow(
            ctx.buf,
            center.x,
            center.y + 2,
            7,
            2,
            shadow_strength,
            ctx.theme,
        );
    }
    for (_, p) in &ctx.layout.plants {
        paint_shadow(ctx.buf, p.x, p.y + 3, 3, 1, shadow_strength, ctx.theme);
    }
    if let Some(lamp) = ctx.layout.floor_lamp {
        paint_shadow(
            ctx.buf,
            lamp.x,
            lamp.y + 5,
            2,
            1,
            shadow_strength,
            ctx.theme,
        );
    }

    ambient::paint_ambient(ctx);

    // Build per-frame occupancy from STATIONARY agent positions only.
    // Walkers are deliberately excluded — their position interpolates
    // every frame, which would change the overlay signature every frame,
    // wipe the path cache, recompute A*, and snap walkers to new path
    // segments (the visible "flash"). Sitters at desks are already
    // covered by the static desk mask. Only waypoint visitors
    // contribute here — they have stable positions across frames,
    // so the signature is stable and the cache hits.
    ctx.overlay.clear();
    for agent in &agents {
        let Some(pose) = pose::derive(agent, ctx.now, ctx.layout) else {
            continue;
        };
        if let Pose::AtWaypoint { wp, .. } = pose {
            if let Some(w) = ctx.layout.waypoints.get(wp) {
                // Reserve the cell the agent actually stands on (the stand cell,
                // off the furniture), NOT the blocked furniture center — else
                // another agent's A* routes straight through the stander. Same
                // `desk` origin as every other stand_point caller.
                let origin = ctx
                    .layout
                    .home_desks
                    .get(agent.desk_index)
                    .copied()
                    .unwrap_or(w.pos);
                let stand = pixtuoid_core::layout::stand_point(
                    w.kind,
                    w.pos,
                    ctx.layout.pantry_counter_size,
                    &ctx.layout.walkable,
                    origin,
                );
                ctx.overlay
                    .add(stand.x.saturating_sub(4), stand.y.saturating_sub(6), 8, 12);
            }
        }
    }

    // --- Build the y-sortable middle pass -------------------------------
    //
    // Every entity gets an `anchor_y` representing its front-facing /
    // floor-touching row. Sort ascending and paint in order so things
    // closer to the camera (larger anchor_y) appear in front. This is
    // the painter's algorithm applied to a top-down 2D scene.
    let mut drawables: Vec<Drawable<'_>> = Vec::new();

    // Desk cubicles (each carries its divider + cabinet + bin + screen
    // glow). Sprite is 16×8, so the actual bottom edge is desk.y + 8 —
    // just past the seated character's feet (desk.y + 4), which keeps
    // the seated worker visually behind the desk like it always was.
    let seated_agents: HashMap<usize, bool> = agents
        .iter()
        .filter(|a| a.desk_index < ctx.layout.home_desks.len() && a.exiting_at.is_none())
        .map(|a| {
            let p = pose::derive_with_routing(
                a,
                ctx.now,
                ctx.layout,
                ctx.router,
                ctx.overlay,
                ctx.history,
                ctx.motion,
            );
            let seated = matches!(p, Some(Pose::SeatedTyping { .. } | Pose::SeatedThinking));
            (a.desk_index, seated)
        })
        .collect();
    for (i, &desk) in ctx.layout.home_desks.iter().enumerate() {
        let is_last_col = desk.x + DESK_W + 2 + DESK_W
            >= ctx.layout.cubicle_band.x + ctx.layout.cubicle_band.width;
        let occupant = agents
            .iter()
            .find(|a| a.desk_index == i && a.exiting_at.is_none());
        let screen_glow = occupant
            .filter(|_| seated_agents.get(&i).copied().unwrap_or(false))
            .and_then(|a| palette::tool_glow_tint(a, &ctx.theme.tool_glow));
        let session_age_secs = occupant
            .and_then(|a| ctx.now.duration_since(a.created_at).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let has_coffee = occupant.is_some_and(|a| ctx.coffee_holders.contains(&a.agent_id));
        let coffee_steam = has_coffee
            && occupant.is_some_and(|a| {
                ctx.coffee_fetched_at
                    .get(&a.agent_id)
                    .and_then(|t| ctx.now.duration_since(*t).ok())
                    .is_some_and(|d| d.as_secs() < COFFEE_STEAM_WINDOW_SECS)
            });
        let stains: &[crate::tui::tui_renderer::StainPos] = occupant
            .and_then(|a| ctx.coffee_stains.get(&a.agent_id))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        drawables.push(Drawable {
            anchor_y: desk.y + 8,
            kind: DrawableKind::DeskCubicle {
                desk,
                is_last_col,
                has_cabinet: i % 2 == 0,
                screen_glow,
                session_age_secs,
                has_coffee,
                coffee_steam,
                stains,
            },
        });
    }

    // Meeting-room area rug — sized to span both sofas + the coffee
    // table with a small margin. Anchored at the TOP so y-sort paints
    // it before the furniture sitting on top of it.
    // Meeting-room area rugs + sofas + tables. For dual-meeting layouts,
    // sofas come in pairs (2 per room), tables 1 per room.
    let sofas_per_room = if ctx.layout.meeting_tables.len() > 1 {
        2
    } else {
        ctx.layout.meeting_sofas.len()
    };
    for (room_idx, &table) in ctx.layout.meeting_tables.iter().enumerate() {
        let sofa_start = room_idx * sofas_per_room;
        let top_sofa = ctx.layout.meeting_sofas.get(sofa_start);
        let bot_sofa = ctx.layout.meeting_sofas.get(sofa_start + 1);
        if let (Some(&ts), Some(&bs)) = (top_sofa, bot_sofa) {
            let rug_w = 18u16;
            let rug_h =
                bs.y.saturating_sub(ts.y)
                    .saturating_add(8)
                    .min(ctx.layout.buf_h.saturating_sub(table.y).saturating_add(8));
            drawables.push(Drawable {
                anchor_y: table.y.saturating_sub(rug_h / 2),
                kind: DrawableKind::AreaRug {
                    pos: table,
                    width: rug_w,
                    height: rug_h,
                },
            });
        }
    }
    for (i, &sofa) in ctx.layout.meeting_sofas.iter().enumerate() {
        let mirrored = i % 2 != 0;
        // A south-of-table sofa faces away (Facing::North → `back_couch`
        // sprite), so the sitter sits BEHIND the sofa back and must be
        // occluded by it — same as the lounge couch. The sitter's y-sort
        // key is `sofa.y + 2`, so the back sofa needs +3 to win that tie;
        // the front (north) sofa stays +2 so its sitter paints on top
        // (insertion order breaks the tie in the sitter's favor). Mirrors
        // core's facing rule (`compute.rs`): back iff sofa.y >= table.y.
        let room_id = i / 2;
        let table_y = ctx
            .layout
            .meeting_tables
            .get(room_id)
            .map_or(sofa.y, |t| t.y);
        let faces_away = sofa.y >= table_y;
        drawables.push(Drawable {
            anchor_y: sofa.y + if faces_away { 3 } else { 2 },
            kind: DrawableKind::MeetingSofa {
                pos: sofa,
                mirrored,
            },
        });
    }
    for &table in &ctx.layout.meeting_tables {
        drawables.push(Drawable {
            anchor_y: table.y + 2,
            kind: DrawableKind::MeetingTable { pos: table },
        });
    }

    // Pantry bistro table (7×4 centered).
    if let Some(table) = ctx.layout.pantry_table {
        drawables.push(Drawable {
            anchor_y: table.y + 2,
            kind: DrawableKind::PantryTable { pos: table },
        });
    }
    // Pantry stools (2×2 anchored at center → bottom = pos.y).
    for chair in &ctx.layout.pantry_chairs {
        drawables.push(Drawable {
            anchor_y: chair.y,
            kind: DrawableKind::PantryChair { pos: *chair },
        });
    }

    // Lounge couch furniture — emitted ONCE, centred on the sofa via
    // `couch_sprite_center`. The couch is now 3 separate seat waypoints, so
    // per-seat emission would triple-paint the sofa/rug/table. Decor → pushed
    // before the character loop so the y-sort tie-break keeps the couch behind
    // its sitters. The rug anchors BEHIND (north of) the couch so it spans the
    // floor on the south side; y-sort anchor at the top so the couch sits on it.
    if let Some(center) = ctx.layout.couch_sprite_center {
        drawables.push(Drawable {
            anchor_y: center.y.saturating_sub(2),
            kind: DrawableKind::AreaRug {
                pos: Point {
                    x: center.x,
                    y: center.y + 3,
                },
                width: 22,
                height: 7,
            },
        });
        drawables.push(Drawable {
            anchor_y: center.y + 3,
            kind: DrawableKind::WaypointCouch { pos: center },
        });
        if let Some(table) = ctx.layout.lounge_side_table {
            drawables.push(Drawable {
                anchor_y: table.y + 1,
                kind: DrawableKind::LoungeSideTable { pos: table },
            });
        }
    }

    // Waypoint furniture — pantry counter, vending, printer — centered on the
    // waypoint position. PhoneBooth/StandingDesk render via the `pod_decor`
    // drawables below (they ARE the decor). The lounge couch is emitted once
    // above (it spans 3 seat waypoints).
    for wp in &ctx.layout.waypoints {
        use crate::tui::layout::WaypointKind;
        match wp.kind {
            WaypointKind::Couch => {}
            WaypointKind::Pantry => {
                let (cw, ch) = ctx.layout.pantry_counter_size;
                drawables.push(Drawable {
                    anchor_y: wp.pos.y + ch / 2,
                    kind: DrawableKind::WaypointPantry {
                        pos: wp.pos,
                        use_large: cw >= 32,
                    },
                });
            }
            WaypointKind::PhoneBooth | WaypointKind::StandingDesk => {}
            WaypointKind::VendingMachine => {
                drawables.push(Drawable {
                    anchor_y: wp.pos.y + 3,
                    kind: DrawableKind::VendingMachine { pos: wp.pos },
                });
            }
            WaypointKind::Printer => {
                drawables.push(Drawable {
                    anchor_y: wp.pos.y + 2,
                    kind: DrawableKind::Printer { pos: wp.pos },
                });
            }
            // Meeting slots ride on the sofa/table furniture, which is
            // painted via the `meeting_sofas` / `meeting_tables` drawables
            // elsewhere — no per-slot furniture here.
            WaypointKind::MeetingSofa | WaypointKind::MeetingStand => {}
        }
    }

    // Pod-aisle decor (plant / whiteboard / TV / phone booth /
    // standing desk). All centered at `pos`; anchor at the bottom of
    // the sprite footprint so y-sort places them correctly against
    // walkers and characters in the aisles.
    for (kind, pos) in &ctx.layout.pod_decor {
        let (_, h) = kind.size();
        drawables.push(Drawable {
            anchor_y: pos.y + h / 2,
            kind: DrawableKind::PodDecorItem {
                kind: *kind,
                pos: *pos,
            },
        });
    }

    // Plants — height varies by sprite, anchor = pos.y + h/2 (center
    // pos convention).
    for (kind, p) in &ctx.layout.plants {
        use crate::tui::layout::PlantKind;
        let h: u16 = match kind {
            PlantKind::Ficus => 7,
            PlantKind::Tall => 9,
            PlantKind::Flower => 6,
            PlantKind::Succulent => 4,
        };
        drawables.push(Drawable {
            anchor_y: p.y + h / 2,
            kind: DrawableKind::Plant {
                kind: *kind,
                pos: *p,
            },
        });
    }

    // Floor lamp (4×10 centered).
    if let Some(lamp) = ctx.layout.floor_lamp {
        drawables.push(Drawable {
            anchor_y: lamp.y + 5,
            kind: DrawableKind::FloorLamp { pos: lamp },
        });
    }

    // Elevator door (16×14, top-left anchored). Frame is computed
    // stateless from agents in their entry/exit window: door opens
    // (0→1→2) over the first DOOR_TRANSITION_MS of the agent's
    // transit, holds open (2) in the middle, then closes (2→1→0)
    // over the final DOOR_TRANSITION_MS. With multiple agents in
    // flight we take the MAX frame so the door is at least as open
    // as the most-in-progress agent needs.
    if let Some(door_pos) = ctx.layout.door {
        let frame_idx = compute_door_frame_idx(&agents, ctx.now, ctx.door_anim_max_ms);
        drawables.push(Drawable {
            anchor_y: door_pos.y + 14,
            kind: DrawableKind::Door {
                pos: door_pos,
                frame_idx,
            },
        });
    }

    // Wall decor — hung on walls (top-left anchored), bottom = pos.y + h.
    for (kind, pos) in &ctx.layout.wall_decor {
        let (_, h) = kind.size();
        drawables.push(Drawable {
            anchor_y: pos.y + h,
            kind: DrawableKind::WallDecor {
                kind: *kind,
                pos: *pos,
            },
        });
    }

    let idle_desk_indices: Vec<usize> = agents
        .iter()
        .filter(|a| {
            matches!(a.state, ActivityState::Idle)
                && a.desk_index < ctx.layout.home_desks.len()
                && a.exiting_at.is_none()
        })
        .map(|a| a.desk_index)
        .collect();
    let all_idle = agents
        .iter()
        .all(|a| matches!(a.state, ActivityState::Idle));

    if let Some(kind) = ctx.floor_pet_kind {
        let active_pet = ctx.active_pet.filter(|p| {
            p.is_active(ctx.now) && p.kind == kind && p.floor_idx == ctx.floor.floor_idx
        });
        let pet_data = if let Some(pet) = active_pet {
            Some((
                pet.pet_pos,
                false,
                kind.sit_anim(),
                0usize,
                Some(pet.elapsed_ms(ctx.now)),
            ))
        } else {
            pet_position(
                kind,
                ctx.layout,
                ctx.pack,
                ctx.now,
                &idle_desk_indices,
                all_idle,
                ctx.floor.floor_seed,
            )
            .map(|(pos, flip, anim, frame)| (pos, flip, anim, frame, None))
        };
        if let Some((pos, flip, anim_name, frame_idx, pet_elapsed)) = pet_data {
            resolved_pet_pos = Some((pos, anim_name, kind));
            drawables.push(Drawable {
                anchor_y: pos.y + 3,
                kind: DrawableKind::Pet {
                    kind,
                    pos,
                    flip,
                    anim_name,
                    frame_idx,
                    pet_elapsed_ms: pet_elapsed,
                },
            });
        }
    }

    // Characters. Anchor = feet (anchor.y + sprite_height). Decollision
    // rank for crowded waypoints — stable across frames thanks to
    // BTreeMap iteration order.
    let mut wp_rank: HashMap<usize, usize> = HashMap::new();
    let mut waypoint_visitors: Vec<chitchat::Visitor> = Vec::new();
    // All 3 lounge-couch seat waypoints collapse to ONE chitchat venue (keyed
    // on the first couch's index) so the couch hosts a single group
    // conversation like the meeting room — without overloading the
    // meeting-only `room_id` field (which indexes `meeting_tables`).
    let couch_group_idx = ctx
        .layout
        .waypoints
        .iter()
        .position(|w| w.kind == crate::tui::layout::WaypointKind::Couch);
    for agent in &agents {
        let Some(desk) = ctx.layout.home_desks.get(agent.desk_index).copied() else {
            continue;
        };
        let Some(p) = pose::derive_with_routing(
            agent,
            ctx.now,
            ctx.layout,
            ctx.router,
            ctx.overlay,
            ctx.history,
            ctx.motion,
        ) else {
            continue;
        };
        match p {
            Pose::SeatedIdle => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, ctx.now);
                let sleep_variant = if agent.agent_id.raw() % 2 == 0 {
                    "seated_sleeping"
                } else {
                    "seated_sleeping_alt"
                };
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: sleep_variant,
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: Some(agent.agent_id.raw()),
                        waiting_bubble: false,
                        thinking_dots: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::SeatedThinking => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, ctx.now);
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "seated",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: Some(ctx.theme.tool_glow.default),
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: true,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::SeatedTyping { frame } => {
                let anchor = with_breath(seated_anchor(desk), agent.agent_id, ctx.now);
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "typing",
                        frame_idx: frame,
                        anchor,
                        flip_x: false,
                        glow_tint: palette::tool_glow_tint(agent, &ctx.theme.tool_glow),
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::StandingAtDesk => {
                let anchor = with_breath(standing_at_desk_anchor(desk), agent.agent_id, ctx.now);
                let is_waiting = matches!(agent.state, ActivityState::Waiting { .. });
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "standing",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: is_waiting,
                        thinking_dots: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::AtWaypoint { wp, kind } => {
                if let Some(wp_obj) = ctx.layout.waypoints.get(wp) {
                    let rank = *wp_rank.entry(wp).or_insert(0);
                    wp_rank.insert(wp, rank + 1);
                    let dx = waypoint_rank_offset_x(kind, rank);
                    use crate::tui::layout::WaypointKind;
                    // Stand cell off the furniture, on the side nearest this
                    // agent's desk — identical resolution to character_anchor /
                    // the walk destination, so the sprite lands where it walked
                    // (no arrival pop) instead of the blocked furniture center.
                    let stand = pixtuoid_core::layout::stand_point(
                        wp_obj.kind,
                        wp_obj.pos,
                        ctx.layout.pantry_counter_size,
                        &ctx.layout.walkable,
                        desk,
                    );
                    let (anim_name, anchor_base, sprite_h, flip_x) = match kind {
                        WaypointKind::Couch => {
                            ("back_couch", back_couch_anchor(stand), 9u16, false)
                        }
                        WaypointKind::Pantry => {
                            ("holding_coffee", waypoint_anchor(stand), 12u16, false)
                        }
                        // Meeting sofa: the north-side seat faces the viewer
                        // across the table (front "seated"); the south-side seat
                        // faces away (back view) — the pair reads as two people
                        // facing each other. Both reuse the 16×7-sofa anchor.
                        WaypointKind::MeetingSofa => {
                            let (anim, flip) = meeting_sprite(kind, wp_obj.facing);
                            (anim, back_couch_anchor(stand), 9u16, flip)
                        }
                        // Meeting stand: beside the table, facing inward.
                        WaypointKind::MeetingStand => {
                            let (anim, flip) = meeting_sprite(kind, wp_obj.facing);
                            (anim, waypoint_anchor(stand), 12u16, flip)
                        }
                        // PhoneBooth + StandingDesk → agent just stands at the
                        // decor. waypoint_anchor positions them directly above
                        // the decor centre (sprite footprint sits just north
                        // of the decor's centre, head visible above).
                        WaypointKind::PhoneBooth
                        | WaypointKind::StandingDesk
                        | WaypointKind::VendingMachine
                        | WaypointKind::Printer => {
                            ("standing", waypoint_anchor(stand), 12u16, false)
                        }
                    };
                    let anchor_no_breath = Point {
                        x: anchor_base.x.saturating_add_signed(dx),
                        y: anchor_base.y,
                    };
                    if chitchat::supports_chitchat(kind) {
                        waypoint_visitors.push(chitchat::Visitor {
                            // Couch seats share one venue (group chat); other
                            // waypoints key on their own index.
                            wp_idx: chitchat::venue_wp_idx(kind, wp, couch_group_idx),
                            agent_id: agent.agent_id,
                            anchor: anchor_no_breath,
                            room_id: wp_obj.room_id,
                        });
                    }
                    let anchor = with_breath(anchor_no_breath, agent.agent_id, ctx.now);
                    drawables.push(Drawable {
                        // Breath-independent sort key: a seated occupant must
                        // y-sort identically every frame so the breath ±1px
                        // never flips it under its sofa (the overlap bug). The
                        // visual `anchor` below still breathes; only the z-order
                        // is pinned. Insertion order (decor before characters)
                        // then keeps the sitter on top of the couch.
                        anchor_y: anchor_no_breath.y + sprite_h,
                        kind: DrawableKind::Character {
                            agent,
                            anim_name,
                            frame_idx: 0,
                            anchor,
                            flip_x,
                            glow_tint: None,
                            sleep_z_seed: None,
                            waiting_bubble: false,
                            thinking_dots: false,
                            walking_dust_frame: None,
                        },
                    });
                }
            }
            Pose::AimlessAt { dest } => {
                let anchor = with_breath(waypoint_anchor(dest), agent.agent_id, ctx.now);
                drawables.push(Drawable {
                    anchor_y: anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name: "standing",
                        frame_idx: 0,
                        anchor,
                        flip_x: false,
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: false,
                        walking_dust_frame: None,
                    },
                });
            }
            Pose::Walking {
                from,
                to,
                t_x1000,
                frame,
                mut carrying_coffee,
            } => {
                // Exit walks: core sets carrying_coffee=false (no
                // render-side state), but we know from coffee_holders.
                if agent.exiting_at.is_some() && ctx.coffee_holders.contains(&agent.agent_id) {
                    carrying_coffee = true;
                }
                if carrying_coffee {
                    new_coffee_carriers.push(agent.agent_id);
                }
                let pos = walking_position(from, to, t_x1000);
                let walker_anchor = walking_anchor(pos);
                let dx = to.x as i32 - from.x as i32;
                let dy = to.y as i32 - from.y as i32;
                let going_back = dy.unsigned_abs() > dx.unsigned_abs() && dy < 0;
                let flip = to.x < from.x;
                // walking_back always wins (no back-facing coffee sprite).
                let anim_name: &'static str = if going_back {
                    "walking_back"
                } else if carrying_coffee && ctx.pack.animation("walking_coffee").is_some() {
                    "walking_coffee"
                } else {
                    "walking"
                };
                drawables.push(Drawable {
                    anchor_y: walker_anchor.y + 12,
                    kind: DrawableKind::Character {
                        agent,
                        anim_name,
                        frame_idx: frame,
                        anchor: walker_anchor,
                        flip_x: flip,
                        glow_tint: None,
                        sleep_z_seed: None,
                        waiting_bubble: false,
                        thinking_dots: false,
                        walking_dust_frame: Some(frame),
                    },
                });
            }
        }
    }

    // Horizontal (E-W) room dividers join the y-sort, anchored at their south
    // (front) edge, so a character standing behind (north of) the wall is
    // composited over by the frosted glass instead of painting on top of it.
    // The vertical (edge-on) dividers already painted in the background pass.
    for (start, end) in &ctx.layout.room_walls {
        if start.y == end.y {
            drawables.push(Drawable {
                anchor_y: start.y + (WALL_THICK_H_PX - 1),
                kind: DrawableKind::RoomWallH {
                    x0: start.x.min(end.x),
                    x1: start.x.max(end.x),
                    y_top: start.y,
                },
            });
        }
    }

    // Stable sort (Rust's `sort_by_key` is stable) — ties preserve
    // insertion order. Insertion order above: decor first, characters
    // last, so a character tied with a piece of furniture paints
    // BEFORE the furniture (matches the prior pass-1 → pass-1.5
    // → pass-2 layering for waypoint couch / pantry counter).
    drawables.sort_by_key(|d| d.anchor_y);
    for d in &drawables {
        paint_drawable(d, ctx.buf, ctx.pack, ctx.cache, ctx.now, ctx.theme);
    }

    // Room-wide lightning bounce — LAST, so a Storm strike briefly flares the
    // whole interior (floor, walls, furniture, characters), not just the window
    // strip. No-op outside a strike / non-storm weather.
    background::paint_lightning_flash(ctx.buf, ctx.now, background::weather_state(ctx.now));

    let chitchat_bubbles = chitchat::update_and_collect(
        ctx.chitchat_state,
        ctx.floor.floor_idx,
        &waypoint_visitors,
        ctx.now,
    );

    PixelPassResult {
        pet_pos: resolved_pet_pos,
        chitchat_bubbles,
        new_coffee_carriers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::sprite::{Frame, Palette};
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn stitch_vertical_wall_connects_each_joint() {
        let top_margin = 48u16;
        let top_wall_h = top_margin - 4; // 44
        let h_y = 90u16; // a horizontal divider row
        let h_rows = [h_y];

        // Top joint: a segment starting at top_margin rises to the window band.
        let (yt, _) = stitch_vertical_wall(top_margin, 70, top_margin, top_wall_h, &h_rows);
        assert_eq!(
            yt, top_wall_h,
            "top segment should connect up to the window band"
        );

        // Corner joint: a segment ending on the horizontal row extends down by
        // the horizontal's thickness to fill the inside corner.
        let (_, yb) = stitch_vertical_wall(60, h_y, top_margin, top_wall_h, &h_rows);
        assert_eq!(
            yb,
            h_y + (WALL_THICK_H_PX - 1),
            "bottom should fill the corner"
        );

        // Bridge-up joint (the dual-meeting case): a segment starting ~6 px
        // below the cross wall is bridged up to meet it. This branch only fires
        // on variant-2 floors, so it has no end-to-end render guard.
        let (yt2, _) = stitch_vertical_wall(h_y + 6, 120, top_margin, top_wall_h, &h_rows);
        assert_eq!(yt2, h_y, "lower segment should bridge up to the cross wall");

        // No false bridge: a segment well below the tolerance stays put, and a
        // segment with no joints is returned unchanged.
        let (yt3, yb3) = stitch_vertical_wall(h_y + 20, 130, top_margin, top_wall_h, &h_rows);
        assert_eq!(
            (yt3, yb3),
            (h_y + 20, 130),
            "distant segment must not bridge"
        );
        let (yt4, yb4) = stitch_vertical_wall(60, 80, top_margin, top_wall_h, &[]);
        assert_eq!((yt4, yb4), (60, 80), "no joints → unchanged");
    }

    #[test]
    fn glass_wall_h_back_cap_composites_over_a_character_behind_it() {
        // Occlusion: the horizontal wall's frosted glass rises GLASS_CAP_PX
        // north of its footprint, y-sorted at the south base — so a character
        // standing just NORTH of the wall (drawn earlier) is composited over
        // by the translucent glass. Stand in for that character with a vivid
        // warm pixel inside the cap band; the glass must shift it toward the
        // cool tone (red drops, blue rises) rather than leave it untouched.
        let theme = crate::tui::theme::theme_by_name("normal").expect("theme");
        let y_top = 20u16;
        // Place the stand-in at the REAL northmost row a routed walker's feet
        // can reach: footprint top `y_top` minus (OBSTACLE_PAD_PX=2 + 1) = the
        // first walkable row north of the wall. With GLASS_CAP_PX=6 the cap
        // (rows y_top-6..y_top-1) covers this row, so a walker's feet/lower legs
        // composite behind the glass. (The old test used y_top-2, a row inside
        // the blocked footprint+pad band that no walker ever occupies.)
        let cap_row = y_top - 3;
        let character = Rgb(220, 40, 40);
        let mut buf = RgbBuffer::filled(48, 48, Rgb(150, 110, 72)); // carpet
        for x in 4..20 {
            buf.put(x, cap_row, character);
        }
        paint_glass_wall_h(&mut buf, theme, 0, 47, y_top);
        let after = buf.get(8, cap_row);
        assert_ne!(after, character, "glass must composite over the character");
        assert!(
            after.0 < character.0 && after.2 > character.2,
            "frosted glass should cool the occluded pixel (red↓ blue↑): {after:?}"
        );
    }

    #[test]
    fn meeting_sprite_maps_facing_to_sprite_and_flip() {
        use crate::tui::layout::{Facing, WaypointKind};
        // North-side sofa seat faces away → back view, no flip.
        assert_eq!(
            meeting_sprite(WaypointKind::MeetingSofa, Facing::North),
            ("back_couch", false)
        );
        // South-side sofa seat faces the viewer → front seated, no flip.
        assert_eq!(
            meeting_sprite(WaypointKind::MeetingSofa, Facing::South),
            ("seated", false)
        );
        // West stand (layout marks it Facing::East) mirrors toward the table.
        assert_eq!(
            meeting_sprite(WaypointKind::MeetingStand, Facing::East),
            ("standing", true)
        );
        // East stand (Facing::West) is unmirrored.
        assert_eq!(
            meeting_sprite(WaypointKind::MeetingStand, Facing::West),
            ("standing", false)
        );
    }

    fn make_slot(id: pixtuoid_core::AgentId, state: ActivityState) -> AgentSlot {
        let now = SystemTime::UNIX_EPOCH;
        AgentSlot {
            agent_id: id,
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/x").as_path()),
            label: Arc::from("x"),
            state,
            state_started_at: now,
            created_at: now,
            last_event_at: now,
            exiting_at: None,
            pending_idle_at: None,

            desk_index: 0,
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        }
    }

    fn base_palette() -> Palette {
        let mut p = Palette::new();
        p.insert('B', Some(Rgb(10, 20, 30))); // shirt
        p.insert('H', Some(Rgb(40, 50, 60))); // hair
        p.insert('S', Some(Rgb(70, 80, 90))); // skin
        p.insert('X', Some(Rgb(99, 99, 99))); // unrelated key
        p
    }

    #[test]
    fn agent_palette_is_deterministic_per_id() {
        let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let a = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        let b = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        assert_eq!(a.get('B'), b.get('B'));
        assert_eq!(a.get('H'), b.get('H'));
        assert_eq!(a.get('S'), b.get('S'));
    }

    #[test]
    fn agent_palette_overrides_only_bhs_keys() {
        let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let p = agent_palette(&base, &make_slot(id, ActivityState::Idle), None);
        // X is not a recolor target — must pass through unchanged.
        assert_eq!(p.get('X'), Some(Some(Rgb(99, 99, 99))));
        // B/H/S must be replaced — the base RGBs (10/20/30 etc.) are
        // unlikely to be in any preset, so they should differ.
        assert_ne!(p.get('B'), Some(Some(Rgb(10, 20, 30))));
        assert_ne!(p.get('H'), Some(Some(Rgb(40, 50, 60))));
        assert_ne!(p.get('S'), Some(Some(Rgb(70, 80, 90))));
    }

    #[test]
    fn agent_palette_glow_tint_shifts_skin_toward_given_color() {
        let id = pixtuoid_core::AgentId::from_transcript_path("/a.jsonl");
        let base = base_palette();
        let slot = make_slot(id, ActivityState::Idle);
        let unlit = agent_palette(&base, &slot, None);
        let green_glow = agent_palette(&base, &slot, Some(Rgb(140, 240, 170)));
        let blue_glow = agent_palette(&base, &slot, Some(Rgb(100, 160, 255)));
        // Shirt / hair / pants are unaffected by glow.
        assert_eq!(unlit.get('B'), green_glow.get('B'));
        assert_eq!(unlit.get('H'), green_glow.get('H'));
        assert_eq!(unlit.get('P'), green_glow.get('P'));
        // Green glow pushes skin's green channel up.
        let (Some(Some(Rgb(_, ug, _))), Some(Some(Rgb(_, gg, _)))) =
            (unlit.get('S'), green_glow.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            gg > ug,
            "green glow should push skin green (lit={gg}, unlit={ug})"
        );
        // Blue glow pushes skin's blue channel up.
        let (Some(Some(Rgb(_, _, ub))), Some(Some(Rgb(_, _, bb)))) =
            (unlit.get('S'), blue_glow.get('S'))
        else {
            panic!("S key missing")
        };
        assert!(
            bb > ub,
            "blue glow should push skin blue (lit={bb}, unlit={ub})"
        );
    }

    #[test]
    fn tool_glow_tint_maps_known_tools() {
        use pixtuoid_core::source::Activity;
        let id = pixtuoid_core::AgentId::from_transcript_path("/t.jsonl");
        let edit_slot = make_slot(
            id,
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: None,
                detail: Some(Arc::from("Edit src/main.rs")),
            },
        );
        let bash_slot = make_slot(
            id,
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: None,
                detail: Some(Arc::from("Bash: ls")),
            },
        );
        let idle_slot = make_slot(id, ActivityState::Idle);
        let glow = &crate::tui::theme::NORMAL.tool_glow;
        let edit_tint = palette::tool_glow_tint(&edit_slot, glow);
        let bash_tint = palette::tool_glow_tint(&bash_slot, glow);
        let idle_tint = palette::tool_glow_tint(&idle_slot, glow);
        assert!(edit_tint.is_some(), "Edit should produce glow");
        assert!(bash_tint.is_some(), "Bash should produce glow");
        assert_eq!(idle_tint, None, "Idle should produce no glow");
        // Edit and Bash should be different colors.
        assert_ne!(edit_tint, bash_tint, "Edit and Bash should differ");
    }

    #[test]
    fn recolor_frame_substitutes_bhs_pixels() {
        let base = base_palette();
        // Build an agent palette where B/H/S are clearly distinguishable.
        let mut agent_pal = base.clone();
        agent_pal.insert('B', Some(Rgb(200, 0, 0))); // red shirt
        agent_pal.insert('H', Some(Rgb(0, 200, 0))); // green hair
        agent_pal.insert('S', Some(Rgb(0, 0, 200))); // blue skin

        // Frame: 1 pixel per palette key + 1 unrelated pixel + 1 transparent.
        let frame = Frame {
            width: 5,
            height: 1,
            pixels: vec![
                Some(Rgb(10, 20, 30)),  // matches base B → should become red
                Some(Rgb(40, 50, 60)),  // matches base H → should become green
                Some(Rgb(70, 80, 90)),  // matches base S → should become blue
                Some(Rgb(123, 45, 67)), // unrelated     → unchanged
                None,                   // transparent   → unchanged
            ],
        };

        let out = recolor_frame(&frame, &agent_pal, &base);
        assert_eq!(out.width, 5);
        assert_eq!(out.height, 1);
        assert_eq!(out.pixels[0], Some(Rgb(200, 0, 0)));
        assert_eq!(out.pixels[1], Some(Rgb(0, 200, 0)));
        assert_eq!(out.pixels[2], Some(Rgb(0, 0, 200)));
        assert_eq!(out.pixels[3], Some(Rgb(123, 45, 67)));
        assert_eq!(out.pixels[4], None);
    }

    #[test]
    fn recolor_frame_handles_palette_with_no_overrides() {
        // If agent palette equals base, frame must come back identical.
        let base = base_palette();
        let frame = Frame {
            width: 3,
            height: 1,
            pixels: vec![
                Some(Rgb(10, 20, 30)),
                Some(Rgb(40, 50, 60)),
                Some(Rgb(70, 80, 90)),
            ],
        };
        let out = recolor_frame(&frame, &base, &base);
        assert_eq!(out.pixels, frame.pixels);
    }

    /// Helper — build a minimal Drawable for sort-order tests. Uses the
    /// MeetingTable variant since it carries no borrowed data.
    fn drawable(anchor_y: u16) -> Drawable<'static> {
        Drawable {
            anchor_y,
            kind: DrawableKind::MeetingTable {
                pos: Point { x: 0, y: 0 },
            },
        }
    }

    #[test]
    fn drawables_sort_ascending_by_anchor_y() {
        let mut v = [drawable(30), drawable(10), drawable(20)];
        v.sort_by_key(|d| d.anchor_y);
        let ys: Vec<u16> = v.iter().map(|d| d.anchor_y).collect();
        assert_eq!(ys, [10, 20, 30]);
    }

    #[test]
    fn drawables_sort_is_stable_on_ties() {
        // Same anchor_y values — TimSort (Rust's stable sort) must
        // preserve insertion order. The y-sort relies on this so that
        // a character at the same anchor_y as the couch behind them
        // still paints first (matches the prior Pass 1 → Pass 1.5
        // layering).
        let mut v = [
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 1, y: 0 },
                },
            },
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 2, y: 0 },
                },
            },
            Drawable {
                anchor_y: 10,
                kind: DrawableKind::MeetingTable {
                    pos: Point { x: 3, y: 0 },
                },
            },
        ];
        v.sort_by_key(|d| d.anchor_y);
        let xs: Vec<u16> = v
            .iter()
            .map(|d| match &d.kind {
                DrawableKind::MeetingTable { pos } => pos.x,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(xs, [1, 2, 3]);
    }

    #[test]
    fn back_view_meeting_sofa_sorts_over_its_sitter() {
        // A south-of-table meeting sofa renders the `back_couch` sprite
        // (Facing::North) — the sitter's body must be occluded BEHIND the
        // sofa back, same as the lounge couch. The back-view sitter's
        // y-sort key is `sofa.y + 2` (back_couch_anchor = stand.y - 7,
        // sprite_h = 9, stand.y = sofa.y); the back sofa must beat that.
        let sofa_y: u16 = 40;
        let sitter_anchor_y = (sofa_y - 7) + 9; // back_couch_anchor + sprite_h
        let back_sofa_anchor_y = sofa_y + 3; // faces_away bump
        let front_sofa_anchor_y = sofa_y + 2; // sitter-on-top default
        assert!(
            back_sofa_anchor_y > sitter_anchor_y,
            "back-view sofa must sort AFTER its sitter (paint on top): \
             sofa={back_sofa_anchor_y}, sitter={sitter_anchor_y}"
        );
        // Front sofa ties the sitter; insertion order (decor first) then
        // keeps the sitter on top — so it must NOT exceed the sitter.
        assert!(
            front_sofa_anchor_y <= sitter_anchor_y,
            "front-view sofa must not sort after its sitter: \
             sofa={front_sofa_anchor_y}, sitter={sitter_anchor_y}"
        );
    }

    #[test]
    fn character_anchor_y_exceeds_desk_when_south_of_it() {
        // The bug-fix invariant: a character whose feet (anchor.y + 12)
        // land BELOW the desk's bottom row (desk.y + 8) must sort AFTER
        // the desk and therefore paint on top.
        let desk_y: u16 = 20;
        let desk_anchor_y = desk_y + 8;
        let char_feet_anchor = (desk_y + 10) + 12; // walker south of desk
        assert!(
            char_feet_anchor > desk_anchor_y,
            "walker south of desk must sort after it: char={char_feet_anchor}, desk={desk_anchor_y}"
        );
    }

    #[test]
    fn character_anchor_y_below_desk_when_seated_at_it() {
        // Inverse invariant — a SEATED character at this desk has feet
        // that land ABOVE the desk's bottom (because they're tucked
        // under the desktop). They must sort BEFORE the desk so the
        // desk occludes their lower body in top-down view.
        let desk_y: u16 = 20;
        let seated_anchor = seated_anchor(Point { x: 0, y: desk_y });
        let char_feet_anchor = seated_anchor.y + 12;
        let desk_anchor_y = desk_y + 8;
        assert!(
            char_feet_anchor < desk_anchor_y,
            "seated char must sort before desk: char={char_feet_anchor}, desk={desk_anchor_y}"
        );
    }

    // --- compute_door_frame_idx -------------------------------------------

    fn entry_slot(created_at_ms_ago: u64, now: SystemTime) -> AgentSlot {
        let id = pixtuoid_core::AgentId::from_transcript_path("/door.jsonl");
        let mut s = make_slot(id, ActivityState::Idle);
        s.created_at = now - std::time::Duration::from_millis(created_at_ms_ago);
        s
    }

    fn exit_slot(exit_ms_ago: u64, now: SystemTime) -> AgentSlot {
        let id = pixtuoid_core::AgentId::from_transcript_path("/exit.jsonl");
        let mut s = make_slot(id, ActivityState::Idle);
        s.created_at = now - std::time::Duration::from_secs(300);
        s.exiting_at = Some(now - std::time::Duration::from_millis(exit_ms_ago));
        s
    }

    #[test]
    fn door_frame_closed_when_no_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(compute_door_frame_idx(&[], now, 0), 0);
    }

    #[test]
    fn door_frame_just_spawned_is_half_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 50 ms into the 200 ms opening ramp — first half = frame 1.
        let slot = entry_slot(50, now);
        assert_eq!(compute_door_frame_idx(&[slot], now, 0), 1);
    }

    #[test]
    fn door_frame_after_opening_ramp_is_fully_open() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 150 ms (still inside opening ramp but past midpoint) → frame 2.
        let s1 = entry_slot(150, now);
        assert_eq!(compute_door_frame_idx(&[s1], now, 0), 2);
        // 2 s into the 4 s window → fully open.
        let s2 = entry_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[s2], now, 0), 2);
    }

    #[test]
    fn door_frame_closing_then_closed_at_end_of_entry() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 150 ms left in the entry window → closing ramp first half → frame 1.
        let mid_close = entry_slot(pose::ENTRY_ANIMATION_MS - 150, now);
        assert_eq!(compute_door_frame_idx(&[mid_close], now, 0), 1);
        // 50 ms left → closing ramp final half → frame 0 (closed).
        let near_end = entry_slot(pose::ENTRY_ANIMATION_MS - 50, now);
        assert_eq!(compute_door_frame_idx(&[near_end], now, 0), 0);
    }

    #[test]
    fn door_frame_expired_entry_contributes_nothing() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Older than the 4 s entry window → no contribution.
        let old = entry_slot(pose::ENTRY_ANIMATION_MS + 1, now);
        assert_eq!(compute_door_frame_idx(&[old], now, 0), 0);
    }

    #[test]
    fn door_frame_exit_window_uses_4500ms_total() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // 2 s into a 4.5 s exit window → mid-flight → fully open.
        let exiting = exit_slot(2_000, now);
        assert_eq!(compute_door_frame_idx(&[exiting], now, 0), 2);
    }

    #[test]
    fn door_frame_takes_max_across_agents() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let opening = entry_slot(50, now); // frame 1
        let open = entry_slot(2_000, now); // frame 2
        assert_eq!(compute_door_frame_idx(&[opening, open], now, 0), 2);
    }

    #[test]
    fn door_frame_uses_physics_window_when_nonzero() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        // Slot spawned 3 s ago; with old ENTRY_ANIMATION_MS=4000 it would still
        // be mid-flight. Supply a short physics window (2500 ms) so it reads as
        // near the closing ramp instead.
        let short_window_ms: u64 = 2_500;
        // elapsed=3000, total=2500 → elapsed > total → door should be in closing
        // ramp or closed (remaining = 0 → frame 0).
        let slot = entry_slot(3_000, now);
        let frame = compute_door_frame_idx(&[slot], now, short_window_ms);
        assert_eq!(
            frame, 0,
            "with short physics window elapsed>total should yield closed door, got frame {frame}"
        );

        // Slot spawned 500 ms ago; physics window = 2500 ms → still well in the
        // middle (fully open frame = 2).
        let slot_mid = entry_slot(500, now);
        let frame_mid = compute_door_frame_idx(&[slot_mid], now, short_window_ms);
        assert_eq!(
            frame_mid, 2,
            "500ms into 2500ms window should be fully open, got frame {frame_mid}"
        );
    }

    #[test]
    fn weather_state_covers_all_variants() {
        let mut seen = std::collections::HashSet::new();
        let base = SystemTime::UNIX_EPOCH;
        for cycle in 0..200u64 {
            let now = base + std::time::Duration::from_secs(cycle * 600);
            seen.insert(std::mem::discriminant(&background::weather_state(now)));
        }
        assert!(
            seen.len() >= 8,
            "expected all 8 weather variants in 200 cycles, got {}",
            seen.len()
        );
    }

    #[test]
    fn weather_state_deterministic() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
        let a = background::weather_state(now);
        let b = background::weather_state(now);
        assert_eq!(a, b);
    }

    #[test]
    fn weather_state_changes_across_cycles() {
        let mut states = Vec::new();
        let base = SystemTime::UNIX_EPOCH;
        for cycle in 0..20u64 {
            states.push(background::weather_state(
                base + std::time::Duration::from_secs(cycle * 600),
            ));
        }
        let unique: std::collections::HashSet<_> =
            states.iter().map(std::mem::discriminant).collect();
        assert!(unique.len() >= 2, "weather should vary across cycles");
    }

    #[test]
    fn sunset_strength_varies_across_day() {
        let mut strengths = Vec::new();
        let base = SystemTime::UNIX_EPOCH;
        for hour in 0..24u64 {
            strengths.push(background::sunset_strength(
                base + std::time::Duration::from_secs(hour * 3600),
            ));
        }
        let has_zero = strengths.iter().any(|s| *s < 0.05);
        let has_nonzero = strengths.iter().any(|s| *s > 0.1);
        assert!(has_zero, "sunset should be ~0 at some hours");
        assert!(has_nonzero, "sunset should be >0 at dawn/dusk hours");
    }
}
