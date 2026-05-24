//! Snapshot regression tests for `draw_scene`.
//!
//! Hashes the pixel buffer of a deterministic scene at a fixed `now` and
//! asserts the hash matches when the same input is rendered twice. Also
//! checks that the time-of-day system actually affects the render (so a
//! refactor that accidentally bypasses the daylight cycle is caught).
//!
//! What this DOESN'T cover: full visual regressions against a golden
//! pixel buffer. The daylight code reads chrono::Local, which makes a
//! golden hash machine-dependent. The determinism check + time-sensitivity
//! check together catch the most likely regressions (nondeterminism,
//! broken time wiring) without needing per-machine goldens.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ascii_agents::tui::embedded_pack::load_sprite_pack;
use ascii_agents::tui::frame_cache::FrameCache;
use ascii_agents::tui::renderer::{draw_scene, TickerQueue};
use ascii_agents_core::source::Activity;
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentId, AgentSlot, SceneState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn fixture_scene(now: SystemTime) -> SceneState {
    let mut s = SceneState::new(12);
    let age_offset = Duration::from_secs(60);
    let cases = [
        (
            "agent-a",
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some("tu_a".into()),
                detail: Some("Write".into()),
            },
        ),
        ("agent-b", ActivityState::Idle),
        (
            "agent-c",
            ActivityState::Waiting {
                reason: "perm?".into(),
            },
        ),
        ("agent-d", ActivityState::Idle),
    ];
    for (i, (key, state)) in cases.iter().enumerate() {
        let id = AgentId::from_transcript_path(&format!("/demo/{key}.jsonl"));
        let created_at = now - age_offset;
        s.agents.insert(
            id,
            AgentSlot {
                agent_id: id,
                source: std::sync::Arc::from("claude-code"),
                session_id: std::sync::Arc::from(format!("session-{i}").as_str()),
                cwd: std::sync::Arc::from(PathBuf::from("/demo").as_path()),
                label: std::sync::Arc::from(*key),
                state: state.clone(),
                state_started_at: now,
                last_event_at: now,
                created_at,
                exiting_at: None,
                pending_idle_at: None,
                desk_index: i,
                tool_call_count: 0,
                active_ms: 0,
            },
        );
    }
    s
}

fn render_pixel_hash(now: SystemTime) -> u64 {
    let scene = fixture_scene(now);
    let backend = TestBackend::new(96, 36);
    let mut term = Terminal::new(backend).expect("terminal");
    let mut buf = RgbBuffer::filled(0, 0, Rgb(0, 0, 0));
    let pack = load_sprite_pack(None).expect("pack");
    let mut cache = FrameCache::new();
    let mut router = ascii_agents::tui::pathfind::AStarRouter::new();
    let mut overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
    draw_scene(
        &mut term,
        &scene,
        &pack,
        now,
        &mut buf,
        &mut cache,
        &mut router,
        &mut overlay,
        &mut ascii_agents::tui::pose::PoseHistory::new(),
        None,
        None,
        &TickerQueue::new(),
        &ascii_agents::tui::theme::NORMAL,
        None,
    )
    .expect("render");

    let mut hasher = DefaultHasher::new();
    for px in &buf.pixels {
        px.0.hash(&mut hasher);
        px.1.hash(&mut hasher);
        px.2.hash(&mut hasher);
    }
    hasher.finish()
}

#[test]
fn render_is_deterministic_for_same_now() {
    // Anchor at a deterministic timestamp. Both runs use the same `now`,
    // so any randomness, hashmap iteration leak, or unstable Vec ordering
    // would show up as a hash mismatch here.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let hash_a = render_pixel_hash(now);
    let hash_b = render_pixel_hash(now);
    assert_eq!(
        hash_a, hash_b,
        "render is non-deterministic — repeat calls with identical input produce different output"
    );
}

#[test]
fn render_changes_when_time_advances_by_hours() {
    // Cross-check: the time-of-day system actually drives the output.
    // A refactor that accidentally hardcoded the daylight constants or
    // bypassed `now` in a paint pass would make these hashes identical.
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let later = base + Duration::from_secs(8 * 3600);
    let hash_a = render_pixel_hash(base);
    let hash_b = render_pixel_hash(later);
    assert_ne!(
        hash_a, hash_b,
        "render is identical 8 hours apart — time-of-day system appears bypassed"
    );
}

/// Catches "render bypassed entirely" / "all black" / "stuck on one color"
/// regressions. Hash equality checks compare to themselves; this check
/// asserts the absolute pixel range looks like a real rendered scene.
#[test]
fn render_produces_distinct_wall_band_and_floor_regions() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let scene = fixture_scene(now);
    let backend = TestBackend::new(96, 36);
    let mut term = Terminal::new(backend).expect("terminal");
    let mut buf = RgbBuffer::filled(0, 0, Rgb(0, 0, 0));
    let pack = load_sprite_pack(None).expect("pack");
    let mut cache = FrameCache::new();
    let mut router = ascii_agents::tui::pathfind::AStarRouter::new();
    let mut overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
    draw_scene(
        &mut term,
        &scene,
        &pack,
        now,
        &mut buf,
        &mut cache,
        &mut router,
        &mut overlay,
        &mut ascii_agents::tui::pose::PoseHistory::new(),
        None,
        None,
        &TickerQueue::new(),
        &ascii_agents::tui::theme::NORMAL,
        None,
    )
    .expect("render");

    // Non-trivial color diversity — guards against an "all black" render or
    // a paint pass that collapsed every pixel to one color.
    let mut colors = std::collections::HashSet::new();
    for px in &buf.pixels {
        colors.insert((px.0, px.1, px.2));
    }
    assert!(
        colors.len() > 32,
        "expected non-trivial color diversity, got {} distinct colors",
        colors.len()
    );

    // Region check: the upper quarter (wall band) and lower half (floor) of
    // the buffer should have distinctly different average colors. Avoids
    // a regression where the wall band paint pass is skipped, leaving the
    // wall band painted in floor colors (or vice versa).
    let w = buf.width as usize;
    let h = buf.height as usize;
    let wall_h = h / 4;
    let floor_y0 = h / 2;

    let avg = |y0: usize, y1: usize| -> (f64, f64, f64) {
        let mut r = 0u64;
        let mut g = 0u64;
        let mut b = 0u64;
        let mut n = 0u64;
        for y in y0..y1 {
            for x in 0..w {
                let p = buf.pixels[y * w + x];
                r += p.0 as u64;
                g += p.1 as u64;
                b += p.2 as u64;
                n += 1;
            }
        }
        let n = n.max(1) as f64;
        (r as f64 / n, g as f64 / n, b as f64 / n)
    };

    let wall = avg(0, wall_h);
    let floor = avg(floor_y0, h);
    let dist =
        ((wall.0 - floor.0).powi(2) + (wall.1 - floor.1).powi(2) + (wall.2 - floor.2).powi(2))
            .sqrt();
    assert!(
        dist > 20.0,
        "wall band and floor regions look identical (dist={dist:.1}, wall={wall:?}, floor={floor:?})"
    );
}

#[test]
fn render_changes_when_an_agent_state_changes() {
    // Active vs all-idle should produce different pixels (screen glow,
    // active monitor scanline, skin tint differ).
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let mut scene_idle = fixture_scene(now);
    for slot in scene_idle.agents.values_mut() {
        slot.state = ActivityState::Idle;
    }
    let backend = TestBackend::new(96, 36);
    let mut term = Terminal::new(backend).expect("terminal");
    let mut buf = RgbBuffer::filled(0, 0, Rgb(0, 0, 0));
    let pack = load_sprite_pack(None).expect("pack");
    let mut cache = FrameCache::new();
    let mut router = ascii_agents::tui::pathfind::AStarRouter::new();
    let mut overlay = ascii_agents_core::walkable::OccupancyOverlay::new();
    draw_scene(
        &mut term,
        &scene_idle,
        &pack,
        now,
        &mut buf,
        &mut cache,
        &mut router,
        &mut overlay,
        &mut ascii_agents::tui::pose::PoseHistory::new(),
        None,
        None,
        &TickerQueue::new(),
        &ascii_agents::tui::theme::NORMAL,
        None,
    )
    .expect("render");
    let mut hasher = DefaultHasher::new();
    for px in &buf.pixels {
        px.0.hash(&mut hasher);
        px.1.hash(&mut hasher);
        px.2.hash(&mut hasher);
    }
    let idle_hash = hasher.finish();

    let active_hash = render_pixel_hash(now);
    assert_ne!(
        idle_hash, active_hash,
        "all-idle and mixed-state scenes produced identical pixels"
    );
}
