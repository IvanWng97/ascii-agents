//! End-to-end headless floor-switch harness. Drives the real
//! `TuiRenderer` (via ratatui `TestBackend`) through the actual
//! `navigate_floor` → transition → `render` path — the production wiring
//! that the unit-level `advance_wander` tests can't reach — and asserts an
//! off-screen floor freezes while hidden and resyncs (no replay) on return.
use super::*;
use pixtuoid_core::state::{ActivityState, AgentSlot, SceneState};
use pixtuoid_core::AgentId;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn slot(id: AgentId, floor_idx: usize, desk_index: usize, started: SystemTime) -> AgentSlot {
    AgentSlot {
        agent_id: id,
        source: Arc::from("cc"),
        session_id: Arc::from("s"),
        cwd: Arc::from(Path::new("/repo")),
        label: Arc::from("a"),
        state: ActivityState::Idle,
        state_started_at: started,
        created_at: started,
        last_event_at: started,
        exiting_at: None,
        pending_idle_at: None,
        desk_index,
        floor_idx,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
    }
}

fn render_until_settled<B: Backend<Error: Send + Sync + 'static>>(
    r: &mut TuiRenderer<B>,
    scene: &SceneState,
    pack: &Pack,
    now: &mut SystemTime,
    target_floor: usize,
) {
    // Drive frames until the transition completes and we're on the target.
    for _ in 0..60 {
        *now += Duration::from_millis(33);
        r.render(scene, pack, *now).expect("render");
        if r.current_floor() == target_floor && r.transition().is_none() {
            return;
        }
    }
    panic!("floor transition to {target_floor} did not settle");
}

// ---- shared helpers -------------------------------------------------

fn pack() -> Pack {
    crate::tui::embedded_pack::load_sprite_pack(None).expect("embedded pack")
}
fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}
fn normal_theme() -> &'static crate::tui::theme::Theme {
    crate::tui::theme::theme_by_name("normal").expect("normal theme")
}
fn dark_theme() -> &'static crate::tui::theme::Theme {
    crate::tui::theme::theme_by_name("cyberpunk").expect("cyberpunk theme")
}
fn build(cols: u16, rows: u16, pets: Vec<PetKind>) -> TuiRenderer<TestBackend> {
    TuiRenderer::new(
        Terminal::new(TestBackend::new(cols, rows)).expect("test backend"),
        normal_theme(),
        pets,
    )
}
/// Idle agent on floor 0 at desk `desk`.
fn idle(id: &str, desk: usize, started: SystemTime) -> AgentSlot {
    slot(AgentId::from_transcript_path(id), 0, desk, started)
}
/// Active (typing) agent with a tool `detail`.
fn active(id: &str, desk: usize, detail: &str, started: SystemTime) -> AgentSlot {
    let mut s = idle(id, desk, started);
    s.state = ActivityState::Active {
        activity: pixtuoid_core::source::Activity::Typing,
        tool_use_id: Some(Arc::from("t")),
        detail: Some(Arc::from(detail)),
    };
    s.last_event_at = started;
    s
}
fn scene_with(agents: Vec<AgentSlot>, cap: usize) -> SceneState {
    let mut s = SceneState::uniform(cap);
    for a in agents {
        s.agents.insert(a.agent_id, a);
    }
    s
}
/// Flatten the ratatui frame into one newline-joined string for substring
/// assertions on rendered text (footer, tooltips, overlays).
fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}
fn lum(c: pixtuoid_core::sprite::Rgb) -> f32 {
    0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32
}
/// Average luminance over a rectangle of the RGB buffer (clamped to bounds).
fn avg_lum(buf: &RgbBuffer, x0: u16, y0: u16, w: u16, h: u16) -> f32 {
    let mut sum = 0.0;
    let mut n = 0u32;
    for y in y0..(y0 + h).min(buf.height) {
        for x in x0..(x0 + w).min(buf.width) {
            sum += lum(buf.get(x, y));
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}
/// Sum of absolute per-channel differences over a rectangle between two
/// buffers of equal size — a robust "did this region change" metric.
fn region_diff(a: &RgbBuffer, b: &RgbBuffer, x0: u16, y0: u16, w: u16, h: u16) -> u64 {
    let mut d = 0u64;
    for y in y0..(y0 + h).min(a.height).min(b.height) {
        for x in x0..(x0 + w).min(a.width).min(b.width) {
            let (p, q) = (a.get(x, y), b.get(x, y));
            d += (p.0 as i32 - q.0 as i32).unsigned_abs() as u64
                + (p.1 as i32 - q.1 as i32).unsigned_abs() as u64
                + (p.2 as i32 - q.2 as i32).unsigned_abs() as u64;
        }
    }
    d
}

#[test]
fn offscreen_floor_freezes_and_resyncs_on_return() {
    let pack = crate::tui::embedded_pack::load_sprite_pack(None).expect("embedded pack");
    let theme = crate::tui::theme::ALL_THEMES[0];
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Two-floor scene: a long-idle (wandering) agent on floor 0, plus a
    // filler on floor 1 so `num_floors` == 2.
    let cap = 16;
    let mut scene = SceneState::uniform(cap);
    let a = AgentId::from_transcript_path("/h/floor0.jsonl");
    let b = AgentId::from_transcript_path("/h/floor1.jsonl");
    scene
        .agents
        .insert(a, slot(a, 0, 0, t0 - Duration::from_secs(120)));
    scene.agents.insert(b, slot(b, 1, cap, t0));

    let term = Terminal::new(TestBackend::new(100, 40)).expect("test backend");
    let mut r = TuiRenderer::new(term, theme, vec![]);

    // Warm up floor 0 so agent A's MotionState initialises and wanders.
    let mut now = t0;
    for _ in 0..10 {
        r.render(&scene, &pack, now).expect("render");
        now += Duration::from_millis(33);
    }
    assert_eq!(r.current_floor(), 0);
    assert!(
        r.floor_motion(0).and_then(|m| m.get(&a)).is_some(),
        "floor-0 agent should have a MotionState after warm-up"
    );

    // Switch to floor 1 and let the transition settle.
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack, &mut now, 1);

    // Baseline: floor 0 is now off-screen.
    let frozen_at = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .map(|ms| ms.last_advanced_at)
        .expect("floor-0 motion present");

    // ~30 s on floor 1 — floor 0 must NOT be advanced.
    for _ in 0..900 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack, now).expect("render");
    }
    let still_frozen = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .map(|ms| ms.last_advanced_at)
        .expect("floor-0 motion present");
    assert_eq!(
        frozen_at, still_frozen,
        "off-screen floor 0 motion must stay frozen while floor 1 is visible"
    );

    // Switch back to floor 0.
    let back_at = now;
    r.navigate_floor(0, now);
    render_until_settled(&mut r, &scene, &pack, &mut now, 0);

    // RESYNC: the stale-resume must re-anchor the phase clock to ~now
    // (clean Seated start) instead of replaying ~30 s of backlogged cycles
    // one transition per frame. wander_phase_started_at would be far in the
    // past if it replayed.
    let ms = r
        .floor_motion(0)
        .and_then(|m| m.get(&a))
        .expect("floor-0 motion present");
    assert!(
            ms.wander_phase_started_at >= back_at,
            "floor-0 agent must resync its wander clock on return (got an anchor before the switch-back ⇒ replay)"
        );
}

// ===================================================================
// Floor navigation
// ===================================================================

fn two_floor_scene() -> SceneState {
    let cap = 16;
    scene_with(
        vec![
            idle("/n/0.jsonl", 0, t0() - Duration::from_secs(120)),
            slot(AgentId::from_transcript_path("/n/1.jsonl"), 1, cap, t0()),
        ],
        cap,
    )
}

#[test]
fn floor_transition_completes_and_lands() {
    let p = pack();
    let scene = two_floor_scene();
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &p, now).unwrap();
    assert_eq!(r.current_floor(), 0);

    r.navigate_floor(1, now);
    assert!(
        r.transition().is_some(),
        "navigation should begin a transition"
    );

    now += Duration::from_millis(450);
    r.render(&scene, &p, now).unwrap();
    assert!(r.transition().is_some(), "still transitioning mid-slide");
    assert!(
        r.cached_layout().is_none(),
        "layout is cleared during a transition"
    );

    now += Duration::from_millis(600); // total 1050ms > 900ms duration
    r.render(&scene, &p, now).unwrap();
    assert!(r.transition().is_none(), "transition complete");
    assert_eq!(r.current_floor(), 1, "landed on the target floor");
    assert!(
        r.cached_layout().is_some(),
        "layout recomputed after landing"
    );
}

#[test]
fn navigation_blocked_during_active_transition() {
    let cap = 16;
    let scene = scene_with(
        vec![
            idle("/b/0.jsonl", 0, t0()),
            slot(AgentId::from_transcript_path("/b/1.jsonl"), 1, cap, t0()),
            slot(
                AgentId::from_transcript_path("/b/2.jsonl"),
                2,
                2 * cap,
                t0(),
            ),
        ],
        cap,
    );
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.navigate_floor(1, now);
    r.navigate_floor(2, now); // must be ignored — a transition is in flight
    assert_eq!(
        r.transition().map(|t| t.to_floor),
        Some(1),
        "a second navigate during a transition is a no-op"
    );
}

#[test]
fn navigate_floor_clears_pinned_agent() {
    let cap = 16;
    let a = AgentId::from_transcript_path("/pin/0.jsonl");
    let scene = scene_with(
        vec![
            slot(a, 0, 0, t0()),
            slot(AgentId::from_transcript_path("/pin/1.jsonl"), 1, cap, t0()),
        ],
        cap,
    );
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_pinned_agent(Some(a));
    r.navigate_floor(1, now);
    assert!(r.pinned_agent().is_none(), "navigation unpins the agent");
}

#[test]
fn transition_cancelled_when_target_floor_disappears() {
    let cap = 16;
    let f1 = slot(AgentId::from_transcript_path("/c/1.jsonl"), 1, cap, t0());
    let mut scene = scene_with(vec![idle("/c/0.jsonl", 0, t0()), f1.clone()], cap);
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.navigate_floor(1, now);
    assert!(r.transition().is_some());

    // Floor-1 agent leaves ⇒ num_floors drops to 1 ⇒ transition target gone.
    scene.agents.remove(&f1.agent_id);
    now += Duration::from_millis(100);
    r.render(&scene, &pack(), now).unwrap();
    assert!(
        r.transition().is_none(),
        "transition to a vanished floor must cancel (no infinite slide)"
    );
    assert_eq!(r.current_floor(), 0);
}

#[test]
fn floor_buffers_grow_on_overflow() {
    let cap = 16;
    let mut r = build(100, 40, vec![]);
    let now = t0();
    let one = scene_with(vec![idle("/g/0.jsonl", 0, t0())], cap);
    r.render(&one, &pack(), now).unwrap();
    assert!(r.floor_buf(1).is_none(), "only one floor allocated");

    let two = scene_with(
        vec![
            idle("/g/0.jsonl", 0, t0()),
            slot(AgentId::from_transcript_path("/g/1.jsonl"), 1, cap, t0()),
        ],
        cap,
    );
    r.render(&two, &pack(), now).unwrap();
    assert!(
        r.floor_buf(1).is_some(),
        "floor-1 buffer allocated after overflow"
    );
}

#[test]
fn per_floor_layout_seeds_differ() {
    let scene = two_floor_scene();
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let seed0 = r.current_floor_seed();
    r.navigate_floor(1, now);
    render_until_settled(&mut r, &scene, &pack(), &mut now, 1);
    assert_ne!(
        seed0,
        r.current_floor_seed(),
        "each floor must use a distinct layout seed"
    );
}

// ===================================================================
// Theme / palette
// ===================================================================

#[test]
fn theme_switch_recolors_floor() {
    let scene = scene_with(vec![idle("/t/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    let now = t0();
    r.render(&scene, &pack(), now).unwrap();
    let before = r.buf().clone();
    r.set_theme(dark_theme());
    r.render(&scene, &pack(), now).unwrap();
    let d = region_diff(&before, r.buf(), 0, 0, before.width, before.height);
    assert!(
        d > 5_000,
        "switching to a different theme must recolor the floor (diff={d})"
    );
}

// ===================================================================
// Lighting
// ===================================================================

// NOTE: the *visible* empty-floor darkening is gated on `look.darkness`
// (time-of-day via `chrono::Local`), so it only manifests at night and is
// timezone-dependent — not robustly assertable through render headlessly.
// The fade math itself is covered by the `LightingState` unit tests in
// floor.rs. Here we only guard the time-independent invariant: an OCCUPIED
// floor must not fade.
#[test]
fn occupied_floor_stays_lit() {
    // A present agent keeps the floor lit (no fade).
    let scene = scene_with(vec![active("/lit/0.jsonl", 0, "Edit x", t0())], 16);
    let mut r = build(100, 40, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    now += Duration::from_millis(2000);
    r.render(&scene, &pack(), now).unwrap();
    let early = avg_lum(r.buf(), 0, 0, r.buf().width, r.buf().height);
    for _ in 0..700 {
        now += Duration::from_millis(33);
        r.render(&scene, &pack(), now).unwrap();
    }
    let late = avg_lum(r.buf(), 0, 0, r.buf().width, r.buf().height);
    assert!(
        late > early * 0.9,
        "occupied floor must stay lit (early={early:.1}, late={late:.1})"
    );
}

// ===================================================================
// Graceful degradation
// ===================================================================

#[test]
fn too_small_terminal_returns_no_layout_no_panic() {
    let scene = scene_with(vec![idle("/sm/0.jsonl", 0, t0())], 16);
    let mut r = build(15, 8, vec![]); // below the 20×12 scene minimum
    r.render(&scene, &pack(), t0())
        .expect("render must not panic");
    assert!(
        r.cached_layout().is_none(),
        "a too-small terminal yields no layout"
    );
}

// ===================================================================
// Coffee state
// ===================================================================

#[test]
fn coffee_stains_cap_at_four_fifo() {
    let mut r = build(100, 40, vec![]);
    let id = AgentId::from_transcript_path("/cof/0.jsonl");
    for i in 0..6 {
        r.note_coffee_stain(id, t0() + Duration::from_secs(i));
    }
    assert_eq!(
        r.coffee_stains_for(id).len(),
        MAX_STAINS_PER_DESK,
        "stains capped at {MAX_STAINS_PER_DESK} (FIFO)"
    );
}

#[test]
fn coffee_state_evicted_when_agent_leaves_scene() {
    let id = AgentId::from_transcript_path("/cof/leave.jsonl");
    let scene = scene_with(vec![slot(id, 0, 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    r.note_coffee_stain(id, t0());
    r.inject_coffee(id, t0());
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(!r.coffee_stains_for(id).is_empty());
    // Agent gone from the scene ⇒ next render evicts its coffee state.
    let empty = SceneState::uniform(16);
    r.render(&empty, &pack(), t0() + Duration::from_millis(33))
        .unwrap();
    assert!(
        r.coffee_stains_for(id).is_empty(),
        "coffee state must be evicted when the agent leaves (no leak)"
    );
}

#[test]
fn injected_coffee_changes_desk_render() {
    // Compare two renders that differ ONLY by coffee state (same scene,
    // same final timestamp) so the diff is attributable to the coffee cup +
    // steam, not elapsed-time animation.
    let id = AgentId::from_transcript_path("/cof/steam.jsonl");
    let scene = scene_with(
        vec![idle("/cof/steam.jsonl", 0, t0() - Duration::from_secs(30))],
        16,
    );
    let t1 = t0() + Duration::from_millis(33);

    let mut base = build(100, 40, vec![]);
    base.render(&scene, &pack(), t0()).unwrap();
    base.render(&scene, &pack(), t1).unwrap();
    let baseline = base.buf().clone();
    let desk = base.cached_layout().expect("layout").home_desks[0];

    let mut r = build(100, 40, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    r.inject_coffee(id, t0()); // fresh fetch ⇒ within steam window
    r.render(&scene, &pack(), t1).unwrap();

    let d = region_diff(
        &baseline,
        r.buf(),
        desk.x.saturating_sub(2),
        desk.y.saturating_sub(6),
        18,
        14,
    );
    assert!(
        d > 0,
        "coffee state should alter the desk render (cup + steam)"
    );
}

// ===================================================================
// Pets
// ===================================================================

#[test]
fn no_pet_when_pets_disabled() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![]); // empty enabled_pets
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.cached_pet_pos().is_none(), "no pet when none enabled");
}

#[test]
fn pet_present_when_enabled() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.cached_pet_pos().is_some(), "a cat should be placed");
}

#[test]
fn pet_position_varies_over_its_cycle() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    let mut seen = std::collections::HashSet::new();
    for i in 0..5 {
        let now = t0() + Duration::from_secs(i * 10);
        r.render(&scene, &pack(), now).unwrap();
        if let Some((pos, anim, _)) = r.cached_pet_pos() {
            seen.insert((pos.x, pos.y, anim));
        }
    }
    assert!(
        seen.len() >= 2,
        "pet should move/animate across its 40s cycle, saw {} distinct states",
        seen.len()
    );
}

#[test]
fn petting_freezes_pet_position() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let (pos, _, kind) = r.cached_pet_pos().expect("pet placed");
    r.set_active_pet(Some(PetState {
        petted_at: t0(),
        pet_pos: pos,
        kind,
        floor_idx: 0,
    }));
    r.render(&scene, &pack(), t0() + Duration::from_millis(500))
        .unwrap();
    let (pos2, _, _) = r.cached_pet_pos().expect("pet still placed");
    assert_eq!(pos, pos2, "a petted pet holds its position");
}

// ===================================================================
// Version popup
// ===================================================================

#[test]
fn version_popup_entrance_reaches_full_scale() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    let s = r.version_popup_scale(t0() + Duration::from_millis(250));
    assert!(s > 0.99, "entrance eases to ~1.0, got {s}");
}

#[test]
fn version_popup_dismissal_reaches_zero() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    let mid = t0() + Duration::from_millis(250);
    r.set_version_popup(false, mid);
    let s = r.version_popup_scale(mid + Duration::from_millis(200));
    assert!(s < 0.01, "dismissal eases to ~0.0, got {s}");
}

#[test]
fn version_popup_interrupt_continues_from_edge() {
    let mut r = build(100, 40, vec![]);
    r.set_version_popup(true, t0());
    // Interrupt entrance ~halfway.
    let half = t0() + Duration::from_millis(100);
    let scale_at_interrupt = r.version_popup_scale(half);
    r.set_version_popup(false, half);
    let s = r.version_popup_scale(half + Duration::from_millis(1));
    assert!(
            (s - scale_at_interrupt).abs() < 0.2,
            "interrupted animation continues from current scale ({scale_at_interrupt}), not a snap (got {s})"
        );
}

// ===================================================================
// Help overlay
// ===================================================================

#[test]
fn help_overlay_renders_shortcuts() {
    let scene = scene_with(vec![idle("/help/0.jsonl", 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    r.set_help_open(true);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.help_open());
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("theme") || text.contains("Keyboard") || text.contains("help"),
        "help overlay should list shortcuts; frame was:\n{text}"
    );
}

// ===================================================================
// Footer / HUD (rendered text)
// ===================================================================

#[test]
fn footer_shows_floor_indicator_on_multi_floor() {
    let scene = two_floor_scene();
    let mut r = build(120, 40, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("1/2") || text.contains("F1"),
        "multi-floor footer should show a floor indicator; frame:\n{text}"
    );
}

// ===================================================================
// Hit-testing against a real rendered layout
// ===================================================================

#[test]
fn furniture_hit_test_resolves_against_rendered_layout() {
    let scene = scene_with(vec![idle("/hit/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // hit_test_furniture takes (pixel_x, cell_y) and doubles y internally.
    let desk = layout.home_desks[0];
    let hit = crate::tui::hit_test::hit_test_furniture(layout, desk.x + 4, desk.y / 2 + 1);
    assert_eq!(
        hit,
        Some("Desk"),
        "a desk pixel should hit the Desk furniture in the cached layout"
    );
}

#[test]
fn coffee_machine_hit_test_resolves_on_pantry() {
    use crate::tui::layout::WaypointKind;
    let scene = scene_with(vec![idle("/cm/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    let pantry = layout
        .waypoints
        .iter()
        .find(|w| w.kind == WaypointKind::Pantry)
        .expect("a 140×48 office must lay out a pantry"); // no silent skip
                                                          // Scan the counter neighbourhood; the machine occupies part of it.
    let cx = pantry.pos.x;
    let cy = pantry.pos.y / 2;
    let mut found = false;
    for dx in -14i32..=14 {
        for dy in -4i32..=4 {
            let mx = (cx as i32 + dx).max(0) as u16;
            let my = (cy as i32 + dy).max(0) as u16;
            if crate::tui::hit_test::hit_test_coffee_machine(layout, mx, my) {
                found = true;
            }
        }
    }
    assert!(
        found,
        "the coffee machine should be hit-testable somewhere on the pantry counter"
    );
}

#[test]
fn pet_hit_test_resolves_at_pet_position() {
    let scene = scene_with(vec![active("/ph/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(120, 44, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let (pos, anim, kind) = r.cached_pet_pos().expect("pet placed");
    assert!(
        crate::tui::hit_test::hit_test_pet(kind, pos, anim, pos.x, pos.y / 2),
        "clicking the pet's own position should hit it"
    );
}

// ===================================================================
// Rendered text: labels, tooltips, footer (via frame_buffer)
// ===================================================================

#[test]
fn agent_label_painted_above_character() {
    let mut s = idle("/lbl/0.jsonl", 0, t0() - Duration::from_secs(300));
    s.label = Arc::from("ZQXLBL");
    let scene = scene_with(vec![s], 16);
    let mut r = build(120, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("ZQXLBL"),
        "the agent's label should be painted above it"
    );
}

#[test]
fn pinned_agent_renders_stats_tooltip() {
    let a = AgentId::from_transcript_path("/pintip/0.jsonl");
    let scene = scene_with(vec![slot(a, 0, 0, t0() - Duration::from_secs(600))], 16);
    let mut r = build(120, 44, vec![]);
    // Baseline without pin.
    r.render(&scene, &pack(), t0()).unwrap();
    let before = frame_text(r.frame_buffer());
    assert!(!before.contains("calls"));
    // Pin → centered stats tooltip appears.
    r.set_pinned_agent(Some(a));
    r.render(&scene, &pack(), t0()).unwrap();
    let after = frame_text(r.frame_buffer());
    assert!(
        after.contains("calls") && after.contains("active"),
        "pinned tooltip should show the agent stat line"
    );
}

#[test]
fn footer_shows_agent_count() {
    let scene = scene_with(
        vec![
            active("/f/0.jsonl", 0, "Edit", t0()),
            idle("/f/1.jsonl", 1, t0()),
            idle("/f/2.jsonl", 2, t0()),
        ],
        16,
    );
    let mut r = build(140, 44, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("agents") && text.contains('3'),
        "full-width footer shows the agent count; frame footer area:\n{}",
        text.lines().last().unwrap_or("")
    );
}

// ===================================================================
// Overlays during a floor transition (transition render path)
// ===================================================================

#[test]
fn version_popup_active_during_floor_transition() {
    let scene = two_floor_scene();
    let mut r = build(120, 44, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_version_popup(true, now);
    r.navigate_floor(1, now);
    now += Duration::from_millis(200); // mid-transition
    r.render(&scene, &pack(), now).unwrap();
    assert!(r.transition().is_some(), "still mid-transition");
    assert!(
        r.last_popup_scale() > 0.0,
        "version popup must keep animating through a floor transition"
    );
}

#[test]
fn help_overlay_renders_during_floor_transition() {
    let scene = two_floor_scene();
    let mut r = build(120, 44, vec![]);
    let mut now = t0();
    r.render(&scene, &pack(), now).unwrap();
    r.set_help_open(true);
    r.navigate_floor(1, now);
    now += Duration::from_millis(200);
    r.render(&scene, &pack(), now).unwrap();
    assert!(r.transition().is_some());
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("theme") || text.contains("Keyboard") || text.contains("help"),
        "help overlay must paint over a floor transition"
    );
}

// ===================================================================
// Per-tool monitor glow (pixel-level)
// ===================================================================

#[test]
fn tool_glow_tint_differs_by_tool() {
    let render_tool = |detail: &str| -> (RgbBuffer, Point) {
        // Long-seated (entry walk done) so it's SeatedTyping at the desk and
        // the monitor screen-glow paints.
        let scene = scene_with(
            vec![active(
                "/tg/0.jsonl",
                0,
                detail,
                t0() - Duration::from_secs(300),
            )],
            16,
        );
        let mut r = build(120, 44, vec![]);
        r.render(&scene, &pack(), t0()).unwrap();
        let desk = r.cached_layout().expect("layout").home_desks[0];
        (r.buf().clone(), desk)
    };
    let (edit, desk) = render_tool("Edit src/main.rs");
    let (bash, _) = render_tool("Bash npm test");
    // Tool tint colours the monitor glow AND the seated worker's skin, both
    // within the cubicle box around the desk.
    let d = region_diff(
        &edit,
        &bash,
        desk.x.saturating_sub(2),
        desk.y.saturating_sub(6),
        20,
        16,
    );
    assert!(
        d > 200,
        "Edit vs Bash should tint the cubicle measurably differently (diff={d})"
    );
}

// ===================================================================
// Tooltip variants on hover (exercise widgets/tooltip.rs branches)
// ===================================================================

#[test]
fn coffee_machine_tooltip_on_hover() {
    let scene = scene_with(vec![idle("/tt/c.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // Find a cell that hits the coffee machine.
    let mut hover = None;
    'scan: for my in 0..48u16 {
        for mx in 0..140u16 {
            if crate::tui::hit_test::hit_test_coffee_machine(layout, mx, my) {
                hover = Some((mx, my));
                break 'scan;
            }
        }
    }
    let hover = hover.expect("coffee machine should be hit-testable");
    r.set_mouse_pos(Some(hover));
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("Ivan"),
        "hovering the coffee machine shows the Buy-Ivan-a-coffee tooltip"
    );
}

#[test]
fn furniture_tooltip_on_hover_over_empty_desk() {
    // Agent on desk 0; hover an EMPTY desk so furniture (not agent) tooltip wins.
    let scene = scene_with(vec![idle("/tt/f.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    if layout.home_desks.len() < 2 {
        return;
    }
    let d1 = layout.home_desks[1];
    r.set_mouse_pos(Some((d1.x + 4, d1.y / 2 + 1)));
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        frame_text(r.frame_buffer()).contains("Desk"),
        "hovering an empty desk shows the Desk furniture tooltip"
    );
}

#[test]
fn pet_tooltip_on_hover() {
    let scene = scene_with(vec![active("/tt/p.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(140, 48, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let (pos, _, _) = r.cached_pet_pos().expect("cat placed");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("Cat") || text.contains("purr"),
        "hovering the cat shows its tooltip"
    );
}

// ===================================================================
// Theme picker + version-popup PAINT (renderer.rs / hud.rs branches)
// ===================================================================

#[test]
fn theme_picker_renders_theme_names() {
    let scene = scene_with(vec![idle("/tp/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.set_theme_picker(Some(0));
    r.render(&scene, &pack(), t0()).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("cyberpunk") || text.contains("normal"),
        "the theme picker lists theme names"
    );
}

#[test]
fn version_popup_paints_when_open() {
    let scene = scene_with(vec![idle("/vp/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    // Baseline (no popup).
    r.render(&scene, &pack(), t0()).unwrap();
    let baseline = r.buf().clone();
    // Open popup; render past the 200ms entrance so it's at full scale.
    r.set_version_popup(true, t0());
    let t1 = t0() + Duration::from_millis(250);
    r.render(&scene, &pack(), t1).unwrap();
    assert!(
        r.last_popup_scale() > 0.9,
        "popup should be near full scale"
    );
    let d = region_diff(&baseline, r.buf(), 0, 0, baseline.width, baseline.height);
    assert!(
        d > 1000,
        "an open version popup must paint over the scene (diff={d})"
    );
}

// ===================================================================
// Desk personalization by session age (drawable.rs)
// ===================================================================

#[test]
fn aged_agent_personalizes_desk() {
    // A long-lived agent accrues desk items (plant ≥30min, photo ≥1hr);
    // its desk region should differ from a brand-new agent's.
    let render_age = |age_secs: u64| -> (RgbBuffer, Point) {
        let scene = scene_with(
            vec![idle(
                "/age/0.jsonl",
                0,
                t0() - Duration::from_secs(age_secs),
            )],
            16,
        );
        let mut r = build(120, 44, vec![]);
        r.render(&scene, &pack(), t0()).unwrap();
        let desk = r.cached_layout().expect("layout").home_desks[0];
        (r.buf().clone(), desk)
    };
    let (fresh, desk) = render_age(5);
    let (aged, _) = render_age(7200); // 2h ⇒ plant + photo frame
    let d = region_diff(
        &fresh,
        &aged,
        desk.x.saturating_sub(2),
        desk.y.saturating_sub(4),
        20,
        12,
    );
    assert!(
        d > 0,
        "an aged agent's desk should show personalization items"
    );
}

// ===================================================================
// Weather smoke-render (background/* + ambient.rs paint paths)
// ===================================================================

#[test]
fn weather_variants_render_without_panic_and_vary() {
    // Weather is a deterministic hash of wall-clock (changes every ~10min).
    // Render across a week of 10-min steps: every variant's paint path runs
    // (no panic), and the window strip takes several distinct appearances.
    let scene = scene_with(vec![idle("/w/0.jsonl", 0, t0())], 16);
    let mut r = build(120, 44, vec![]);
    let mut sigs = std::collections::HashSet::new();
    for step in 0..120u64 {
        // 10-min steps so each sample can land on a different weather window.
        let now = t0() + Duration::from_secs(step * 600 + 12 * 3600);
        r.render(&scene, &pack(), now).unwrap();
        // Signature the top window strip (where weather effects paint).
        let buf = r.buf();
        let mut s: u64 = 0;
        for y in 0..(buf.height / 4).max(1) {
            for x in (0..buf.width).step_by(7) {
                let c = buf.get(x, y);
                s = s
                    .wrapping_mul(1099511628211)
                    .wrapping_add((c.0 as u64) << 16 | (c.1 as u64) << 8 | c.2 as u64);
            }
        }
        sigs.insert(s);
    }
    assert!(
        sigs.len() >= 4,
        "weather/time variation should produce several distinct window renders, saw {}",
        sigs.len()
    );
}
