//! Smoke test that `TuiRenderer` correctly implements the core `Renderer`
//! trait. Closes the v1 gap where the production binary called the free
//! function `draw_scene` directly, leaving the trait unexercised outside of
//! the in-memory `TestRenderer` fixture.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ascii_agents::tui::embedded_pack::load_sprite_pack;
use ascii_agents::tui::tui_renderer::TuiRenderer;
use ascii_agents_core::source::Activity;
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentId, AgentSlot, Renderer, SceneState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn tui_renderer_render_paints_a_full_frame() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800);
    let mut scene = SceneState::new(8);
    let id = AgentId::from_transcript_path("/demo/a.jsonl");
    scene.agents.insert(
        id,
        AgentSlot {
            agent_id: id,
            source: std::sync::Arc::from("claude-code"),
            session_id: std::sync::Arc::from("s-1"),
            cwd: std::sync::Arc::from(PathBuf::from("/demo").as_path()),
            label: std::sync::Arc::from("demo"),
            state: ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some(std::sync::Arc::from("t1")),
                detail: Some(std::sync::Arc::from("Write")),
            },
            state_started_at: now,
            created_at: now - Duration::from_secs(60),
            last_event_at: now - Duration::from_secs(60),
            exiting_at: None,
            pending_idle_at: None,
            desk_index: 0,
            tool_call_count: 0,
            active_ms: 0,
        },
    );

    let backend = TestBackend::new(96, 36);
    let terminal = Terminal::new(backend).expect("terminal");
    let mut renderer = TuiRenderer::new(terminal, &ascii_agents::tui::theme::NORMAL);
    let pack = load_sprite_pack(None).expect("pack");

    renderer
        .render(&scene, &pack, now)
        .expect("render through Renderer trait");

    // The TUI impl owns the pixel buffer — after render, it should be sized
    // for the 96×(36-1) scene area (one row reserved for footer), doubled
    // vertically via half-block: 96 cells wide, 70 pixels tall.
    let buf = renderer.buf();
    assert_eq!(buf.width, 96);
    assert_eq!(buf.height, 70);

    // And it should contain something (non-trivial color diversity), proving
    // the trait method actually triggered the paint pipeline.
    let mut colors = std::collections::HashSet::new();
    for px in &buf.pixels {
        colors.insert((px.0, px.1, px.2));
    }
    assert!(
        colors.len() > 32,
        "TuiRenderer::render produced suspiciously few colors ({})",
        colors.len()
    );
}
