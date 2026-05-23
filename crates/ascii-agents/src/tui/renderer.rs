//! Terminal-coupled rendering: orchestrator (`draw_scene`), half-block
//! flush, label/tooltip/notice widget overlays, and terminal lifecycle.
//!
//! The pure-pixel pass (floor/walls/decor/characters → `RgbBuffer`) lives
//! in `tui::pixel_painter`. This file is the integrator that calls into
//! that pipeline and then hands the buffer to ratatui.

use std::collections::HashMap;
use std::io::{stdout, Stdout};
use std::time::SystemTime;

use anyhow::Result;
use ascii_agents_core::sprite::format::Pack;
use ascii_agents_core::sprite::{Rgb, RgbBuffer};
use ascii_agents_core::state::ActivityState;
use ascii_agents_core::{AgentId, SceneState};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

use ascii_agents_core::walkable::OccupancyOverlay;

use crate::tui::frame_cache::FrameCache;
use crate::tui::layout::{Layout, DESK_W};
use crate::tui::pathfind::Router;
use crate::tui::pixel_painter::{character_anchor, render_to_rgb_buffer};
use crate::tui::pose;

/// Wall band background — used to fall back to a sensible color when the
/// terminal is too small to render the full scene.
const BG: Rgb = Rgb(28, 32, 40);

/// Clip a widget rect to fit inside `bounds`. Returns `None` if the rect
/// falls fully outside or has zero width/height after clipping — callers
/// use that to skip the render entirely. Prevents ratatui's
/// "index outside of buffer" panic when label/notice widgets land near
/// the right or bottom edge.
fn clip_widget_rect(rect: Rect, bounds: Rect) -> Option<Rect> {
    if rect.x >= bounds.x + bounds.width || rect.y >= bounds.y + bounds.height {
        return None;
    }
    if rect.x + rect.width <= bounds.x || rect.y + rect.height <= bounds.y {
        return None;
    }
    let x = rect.x.max(bounds.x);
    let y = rect.y.max(bounds.y);
    let right = (rect.x + rect.width).min(bounds.x + bounds.width);
    let bot = (rect.y + rect.height).min(bounds.y + bounds.height);
    if right <= x || bot <= y {
        return None;
    }
    Some(Rect {
        x,
        y,
        width: right - x,
        height: bot - y,
    })
}

pub type Term = Terminal<CrosstermBackend<Stdout>>;

// --- Terminal lifecycle ---------------------------------------------------
pub fn setup_terminal() -> Result<Term> {
    enable_raw_mode()?;
    let mut out = stdout();
    // EnableMouseCapture turns on the terminal's mouse-event reporting.
    // Modern terminals emit MouseEventKind::Moved on cursor motion (no
    // button required), which is how we drive the hover tooltip.
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

pub fn teardown_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    term.show_cursor()?;
    Ok(())
}

// --- draw_scene ----------------------------------------------------------
//
// `draw_scene` is the orchestrator: get terminal geometry, compute the
// layout, run the pure pixel pass, then flush to the terminal. The two
// helpers below are deliberately split:
//
//   * `render_to_rgb_buffer` — pure RGB output. No ratatui types, no
//     terminal I/O. Can be called by any renderer (web canvas, PNG
//     snapshot, GIF capture).
//   * `flush_to_terminal` — ratatui half-block compression + label overlay
//     + bulletin notice + footer. Terminal-specific, runs inside
//     `term.draw`.
#[allow(clippy::too_many_arguments)]
pub fn draw_scene<B: Backend>(
    term: &mut Terminal<B>,
    scene: &SceneState,
    pack: &Pack,
    now: SystemTime,
    buf: &mut RgbBuffer,
    cache: &mut FrameCache,
    router: &mut dyn Router,
    overlay: &mut OccupancyOverlay,
    history: &mut pose::PoseHistory,
    mouse_pos: Option<(u16, u16)>,
) -> Result<()> {
    let term_size = term.size()?;
    let full_rect = Rect {
        x: 0,
        y: 0,
        width: term_size.width,
        height: term_size.height,
    };
    let scene_rect = Rect {
        x: 0,
        y: 0,
        width: full_rect.width,
        height: full_rect.height.saturating_sub(1),
    };
    if scene_rect.width < 20 || scene_rect.height < 12 {
        term.draw(|f| paint_footer(f, scene, full_rect))?;
        return Ok(());
    }

    let buf_w = scene_rect.width;
    let buf_h = scene_rect.height * 2;
    buf.ensure_size(buf_w, buf_h, BG);
    let Some(layout) = Layout::compute(buf_w, buf_h, scene.max_desks) else {
        term.draw(|f| paint_footer(f, scene, full_rect))?;
        return Ok(());
    };

    // Bias the router toward the corridor (the office "main aisle") so
    // walkers naturally use the hallway instead of cutting diagonally
    // across the cubicle floor. Cheap call — invalidates the cache only
    // when the zone actually changes (layout resize).
    router.set_preferred_zone(layout.corridor);

    // Pure pixel pass — no ratatui types touched. Pixel pass writes
    // into PoseHistory for every walking/waypoint agent so the next
    // frame's snap-back lookup is fresh.
    render_to_rgb_buffer(
        scene, &layout, pack, now, buf, cache, router, overlay, history,
    );

    // Hit-test the cursor against each agent's current sprite footprint
    // so the tooltip + focus ring know who's under the pointer. Cell-
    // accurate (one terminal cell = 2 vertical pixels in the half-block
    // buffer).
    let hovered = mouse_pos
        .and_then(|(mx, my)| hit_test_agent(scene, &layout, now, router, overlay, history, mx, my));

    // Terminal-flush pass — half-block + widgets, inside ratatui's draw.
    term.draw(|f| {
        paint_footer(f, scene, full_rect);
        flush_buffer_to_term(f, buf, scene_rect);
        paint_label_widgets(
            f, scene, &layout, now, router, overlay, history, scene_rect, hovered,
        );
        paint_bulletin_notice(f, scene, &layout, scene_rect);
        if let (Some(agent_id), Some((mx, my))) = (hovered, mouse_pos) {
            paint_hover_tooltip(f, scene, agent_id, mx, my, scene_rect);
        }
    })?;
    Ok(())
}

fn paint_footer(f: &mut ratatui::Frame<'_>, scene: &SceneState, full_rect: Rect) {
    let summary = build_status_summary(scene, full_rect.width);
    let footer = Paragraph::new(Span::raw(summary)).style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        footer,
        Rect {
            x: full_rect.x,
            y: full_rect.y + full_rect.height.saturating_sub(1),
            width: full_rect.width,
            height: 1,
        },
    );
}

/// Compose the footer's single-line summary, picking the widest variant
/// (full / medium / minimal) that fits inside `term_width` alongside the
/// fixed-right `[q] quit` suffix. Pure function — drives `paint_footer`
/// and is unit-tested directly.
///
/// Tier breakdown:
///   * **full** (~50+ cells) — total count, per-state counts, top tool
///     names with usage tallies, e.g. `12 agents · 3 active · 2 waiting
///     · 7 idle · Edit×2 Bash×1`.
///   * **medium** (~30+ cells) — compact letters, e.g. `12a · 3A · 2W · 7I`.
///   * **minimal** — just the total, e.g. `12a`.
///   * **fallback** — only the quit hint (any narrower terminal will
///     truncate this naturally).
pub(super) fn build_status_summary(scene: &SceneState, term_width: u16) -> String {
    let n = scene.agents.len();
    let mut active = 0usize;
    let mut waiting = 0usize;
    let mut idle = 0usize;
    let mut tool_counts: HashMap<&str, usize> = HashMap::new();
    for slot in scene.agents.values() {
        match &slot.state {
            ActivityState::Idle => idle += 1,
            ActivityState::Waiting { .. } => waiting += 1,
            ActivityState::Active { detail, .. } => {
                active += 1;
                if let Some(d) = detail.as_deref() {
                    // Take the leading alphanumeric token as the tool
                    // name — Generic tools format as "Edit src/x.rs",
                    // "Bash: ls"; Task tool shows "Delegating".
                    let token = d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
                    if !token.is_empty() {
                        *tool_counts.entry(token).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    const QUIT: &str = " [q] quit ";
    let tools_str = {
        // Sort by count desc, then name asc for stable output. Top 4
        // keeps the line bounded — beyond that the listing crowds out
        // the state counts on medium-width terminals.
        let mut tools: Vec<(&&str, &usize)> = tool_counts.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        tools
            .iter()
            .take(4)
            .map(|(name, count)| format!("{name}×{count}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let stats_full = if n == 0 {
        " 0 agents ".to_string()
    } else {
        let mut s = format!(" {n} agents · {active} active · {waiting} waiting · {idle} idle");
        if !tools_str.is_empty() {
            s.push_str(" · ");
            s.push_str(&tools_str);
        }
        s.push(' ');
        s
    };
    let stats_medium = format!(" {n}a · {active}A · {waiting}W · {idle}I ");
    let stats_min = format!(" {n}a ");

    let w = term_width as usize;
    let q = QUIT.len();
    for stats in [&stats_full, &stats_medium, &stats_min] {
        if stats.len() + q <= w {
            let pad = w.saturating_sub(stats.len() + q);
            let mut out = String::with_capacity(w);
            out.push_str(stats);
            for _ in 0..pad {
                out.push(' ');
            }
            out.push_str(QUIT);
            return out;
        }
    }
    QUIT.to_string()
}

fn flush_buffer_to_term(f: &mut ratatui::Frame<'_>, buf: &RgbBuffer, scene_rect: Rect) {
    let term_buf = f.buffer_mut();
    let w = buf.width as usize;
    let cell_rows = (buf.height / 2) as usize;
    for cy in 0..cell_rows {
        for cx in 0..(buf.width as usize) {
            let x = scene_rect.x + cx as u16;
            let y = scene_rect.y + cy as u16;
            if x >= scene_rect.x + scene_rect.width || y >= scene_rect.y + scene_rect.height {
                continue;
            }
            let py_top = cy * 2;
            let py_bot = cy * 2 + 1;
            let fg = buf.pixels[py_top * w + cx];
            let bg = buf.pixels[py_bot * w + cx];
            let cell = &mut term_buf[(x, y)];
            cell.set_symbol("▀");
            cell.fg = Color::Rgb(fg.0, fg.1, fg.2);
            cell.bg = Color::Rgb(bg.0, bg.1, bg.2);
        }
    }
}

/// Labels above each character — uses `character_anchor` to follow the
/// agent along its current path, color-codes by activity, falls back to
/// disambiguating session-id suffix only when multiple agents share a label.
///
/// `hovered` highlights one agent's label: bright white + bold + leading
/// ▸ marker so the focused character is easy to pick out of a crowd.
#[allow(clippy::too_many_arguments)]
fn paint_label_widgets(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    router: &mut dyn Router,
    overlay: &OccupancyOverlay,
    history: &mut pose::PoseHistory,
    scene_rect: Rect,
    hovered: Option<AgentId>,
) {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    let mut label_counts: HashMap<&str, usize> = HashMap::new();
    for agent in &agents {
        *label_counts.entry(&*agent.label).or_insert(0) += 1;
    }
    for agent in &agents {
        let Some(anchor) = character_anchor(agent, layout, now, router, overlay, history) else {
            continue;
        };
        let lx = scene_rect.x + anchor.x.saturating_sub(2);
        let ly = scene_rect.y + (anchor.y / 2).saturating_sub(1);
        let needs_disambig = label_counts.get(&*agent.label).copied().unwrap_or(0) > 1
            && agent.session_id.len() >= 4;
        let raw: std::borrow::Cow<'_, str> = if needs_disambig {
            std::borrow::Cow::Owned(format!("{}·{}", agent.label, &agent.session_id[..4]))
        } else {
            std::borrow::Cow::Borrowed(&*agent.label)
        };
        let display = truncate_label(&raw, (DESK_W + 4) as usize);
        let is_hovered = hovered == Some(agent.agent_id);
        let label_color = if is_hovered {
            Color::Rgb(255, 255, 255)
        } else if agent.exiting_at.is_some() {
            Color::Rgb(100, 110, 130)
        } else {
            match &agent.state {
                ActivityState::Active { .. } => Color::Rgb(140, 240, 170),
                ActivityState::Waiting { .. } => Color::Rgb(240, 200, 80),
                ActivityState::Idle => Color::Rgb(160, 160, 160),
            }
        };
        let text = if is_hovered {
            format!("▸{}", display)
        } else {
            display.into_owned()
        };
        let mut style = Style::default().fg(label_color);
        if is_hovered {
            style = style.add_modifier(ratatui::style::Modifier::BOLD);
        }
        let para = Paragraph::new(Span::styled(text, style));
        if let Some(r) = clip_widget_rect(
            Rect {
                x: lx,
                y: ly,
                width: DESK_W + 4,
                height: 1,
            },
            scene_rect,
        ) {
            f.render_widget(para, r);
        }
    }
}

/// Hit-test the mouse cursor against each agent's current sprite footprint.
/// Returns the agent under `(mx, my)` (in terminal cell coordinates), or
/// `None` if no agent occupies that cell.
///
/// The character sprite is 8×12 pixels, which in cell space is 8 cells
/// wide × 6 cells tall (one cell = 2 vertical pixels). We test against
/// that exact bounding box anchored on the agent's `character_anchor`.
#[allow(clippy::too_many_arguments)]
fn hit_test_agent(
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

/// Floating detail panel painted near the cursor when an agent is hovered.
/// Shows the label, source, state, current tool detail, cwd, and session
/// id. Positioned to avoid the cursor itself and the screen edges.
fn paint_hover_tooltip(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    agent_id: AgentId,
    mx: u16,
    my: u16,
    scene_rect: Rect,
) {
    let Some(agent) = scene.agents.get(&agent_id) else {
        return;
    };

    // Build the tooltip lines.
    let (state_label, state_detail, state_color) = match &agent.state {
        ActivityState::Idle => ("Idle", String::new(), Color::Rgb(160, 160, 160)),
        ActivityState::Active { detail, .. } => (
            "Active",
            detail.as_deref().unwrap_or("").to_string(),
            Color::Rgb(140, 240, 170),
        ),
        ActivityState::Waiting { reason } => {
            ("Waiting", reason.to_string(), Color::Rgb(240, 200, 80))
        }
    };
    let cwd_short = agent
        .cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)");
    let session_short = if agent.session_id.len() >= 8 {
        &agent.session_id[..8]
    } else {
        &agent.session_id
    };

    let mut lines: Vec<ratatui::text::Line> = Vec::new();
    lines.push(ratatui::text::Line::from(Span::styled(
        format!(" {} ", agent.label),
        Style::default()
            .fg(Color::White)
            .add_modifier(ratatui::style::Modifier::BOLD),
    )));
    lines.push(ratatui::text::Line::from(vec![
        Span::raw(" ●  "),
        Span::styled(state_label, Style::default().fg(state_color)),
    ]));
    if !state_detail.is_empty() {
        // Truncate long tool detail (e.g. full file paths) to keep tooltip narrow.
        let trimmed: String = state_detail.chars().take(34).collect();
        lines.push(ratatui::text::Line::from(Span::styled(
            format!("    {}", trimmed),
            Style::default().fg(Color::Rgb(200, 200, 210)),
        )));
    }
    lines.push(ratatui::text::Line::from(Span::styled(
        format!(" 📁 {}", cwd_short),
        Style::default().fg(Color::Rgb(180, 180, 180)),
    )));
    lines.push(ratatui::text::Line::from(Span::styled(
        format!(" ⌗ {} · {}", session_short, agent.source),
        Style::default().fg(Color::Rgb(140, 140, 150)),
    )));

    let lines_h = lines.len() as u16;
    let max_w = lines.iter().map(|l| l.width() as u16).max().unwrap_or(20) + 2;
    let tip_w = max_w.min(scene_rect.width).max(18);
    let tip_h = lines_h;

    // Place the tooltip to the RIGHT and BELOW the cursor when there's
    // room; otherwise flip to the other side so it stays on-screen.
    let mut tx = mx.saturating_add(2);
    if tx.saturating_add(tip_w) > scene_rect.x + scene_rect.width {
        tx = mx.saturating_sub(tip_w + 1);
    }
    let mut ty = my.saturating_add(1);
    if ty.saturating_add(tip_h) > scene_rect.y + scene_rect.height {
        ty = my.saturating_sub(tip_h).max(scene_rect.y);
    }
    let rect = Rect {
        x: tx,
        y: ty,
        width: tip_w,
        height: tip_h,
    };
    let Some(clipped) = clip_widget_rect(rect, scene_rect) else {
        return;
    };

    let para =
        Paragraph::new(lines).style(Style::default().bg(Color::Rgb(20, 22, 30)).fg(Color::White));
    f.render_widget(ratatui::widgets::Clear, clipped);
    f.render_widget(para, clipped);
}

/// Live agent count painted as a sticky on the bulletin board sprite.
fn paint_bulletin_notice(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    layout: &Layout,
    scene_rect: Rect,
) {
    use crate::tui::layout::WallDecor;
    let Some((_, bb_pos)) = layout
        .wall_decor
        .iter()
        .find(|(k, _)| *k == WallDecor::BulletinBoard)
    else {
        return;
    };
    let cell_x = scene_rect.x + bb_pos.x;
    let cell_y = scene_rect.y + (bb_pos.y / 2).saturating_sub(1);
    let n = scene
        .agents
        .values()
        .filter(|a| a.exiting_at.is_none())
        .count();
    let label = format!("{} live", n);
    let notice = Paragraph::new(Span::styled(label, Style::default().fg(Color::Yellow)));
    if let Some(r) = clip_widget_rect(
        Rect {
            x: cell_x,
            y: cell_y,
            width: 8,
            height: 1,
        },
        scene_rect,
    ) {
        f.render_widget(notice, r);
    }
}

/// Fit a label into `budget` chars without losing the `·xxxx` session-id
/// disambiguation suffix that the reducer appends to colliding cwds.
/// Truncates from the base (left side of the `·`), not from the suffix —
/// otherwise the disambig becomes useless ("TikTok-Android·a" tells us
/// nothing the base alone wouldn't).
fn truncate_label(label: &str, budget: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if label.chars().count() <= budget {
        return Cow::Borrowed(label);
    }
    if let Some(sep_byte) = label.rfind('·') {
        let suffix = &label[sep_byte..];
        let suffix_len = suffix.chars().count();
        if suffix_len < budget {
            let base = &label[..sep_byte];
            let base_take = budget - suffix_len;
            let truncated: String = base.chars().take(base_take).collect();
            return Cow::Owned(format!("{truncated}{suffix}"));
        }
    }
    Cow::Owned(label.chars().take(budget).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_label_passes_short_labels_through() {
        assert_eq!(truncate_label("hello", 16), "hello");
    }

    #[test]
    fn truncate_label_preserves_disambig_suffix() {
        // 19 chars > 16 budget → must drop chars from the base, NOT the suffix.
        let out = truncate_label("TikTok-Android·a09a", 16);
        assert_eq!(out.chars().count(), 16);
        assert!(out.ends_with("·a09a"), "suffix lost: {out}");
        assert!(out.starts_with("TikTok"), "base over-truncated: {out}");
    }

    #[test]
    fn truncate_label_falls_back_to_plain_truncate_when_no_separator() {
        let out = truncate_label("a-very-long-project-name", 8);
        assert_eq!(out, "a-very-l");
    }

    // --- build_status_summary ---------------------------------------------

    use ascii_agents_core::source::Activity;
    use ascii_agents_core::{AgentId, AgentSlot};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn slot_with(state: ActivityState, label: &str) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path(&format!("/p/{label}.jsonl")),
            source: Arc::from("claude-code"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: Arc::from(label),
            state,
            state_started_at: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: 0,
        }
    }
    fn active_with(detail: &str, label: &str) -> AgentSlot {
        slot_with(
            ActivityState::Active {
                activity: Activity::Typing,
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from(detail)),
            },
            label,
        )
    }
    fn waiting(label: &str) -> AgentSlot {
        slot_with(
            ActivityState::Waiting {
                reason: Arc::from("perm"),
            },
            label,
        )
    }
    fn idle(label: &str) -> AgentSlot {
        slot_with(ActivityState::Idle, label)
    }
    fn scene_of(slots: Vec<AgentSlot>) -> SceneState {
        let mut s = SceneState::new(16);
        for slot in slots {
            s.agents.insert(slot.agent_id, slot);
        }
        s
    }

    #[test]
    fn footer_zero_agents_shows_zero_count_and_quit() {
        let s = scene_of(vec![]);
        let line = build_status_summary(&s, 80);
        assert!(line.contains("0 agents"), "missing zero count: {line:?}");
        assert!(
            line.ends_with(" [q] quit "),
            "missing quit suffix: {line:?}"
        );
        assert_eq!(line.len(), 80, "should pad to full width: {line:?}");
    }

    #[test]
    fn footer_full_width_shows_state_breakdown_and_tools() {
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            active_with("Edit src/b.rs", "b"),
            active_with("Bash: ls", "c"),
            waiting("d"),
            waiting("e"),
            idle("f"),
            idle("g"),
            idle("h"),
        ]);
        let line = build_status_summary(&s, 120);
        // Per-state counts present.
        assert!(line.contains("8 agents"), "{line:?}");
        assert!(line.contains("3 active"), "{line:?}");
        assert!(line.contains("2 waiting"), "{line:?}");
        assert!(line.contains("3 idle"), "{line:?}");
        // Top tools by count, ordered desc: Edit×2 should come before Bash×1.
        let edit_pos = line.find("Edit×2").expect("Edit×2 present");
        let bash_pos = line.find("Bash×1").expect("Bash×1 present");
        assert!(
            edit_pos < bash_pos,
            "tools should sort by count desc: {line:?}"
        );
    }

    #[test]
    fn footer_medium_width_drops_to_compact_letters() {
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            waiting("b"),
            idle("c"),
        ]);
        // 36 cells is too narrow for the full version but fits medium.
        let line = build_status_summary(&s, 36);
        assert!(
            line.contains("3a") && line.contains("1A"),
            "expected medium tier letters: {line:?}"
        );
        assert!(
            !line.contains("3 agents · "),
            "full tier should not fit at width 36: {line:?}"
        );
        assert!(line.ends_with(" [q] quit "), "{line:?}");
    }

    #[test]
    fn footer_minimal_width_keeps_total_and_quit_only() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        // Just wide enough for " 2a " (4) + " [q] quit " (10) = 14.
        let line = build_status_summary(&s, 16);
        assert!(line.contains("2a"), "expected minimal tier: {line:?}");
        assert!(line.ends_with(" [q] quit "), "{line:?}");
        assert_eq!(line.len(), 16);
    }

    #[test]
    fn footer_collapses_to_quit_only_below_minimal_threshold() {
        let s = scene_of(vec![idle("a")]);
        // Width 10 fits exactly " [q] quit " — no stats at all.
        let line = build_status_summary(&s, 10);
        assert_eq!(line, " [q] quit ");
    }

    #[test]
    fn footer_caps_tool_breakdown_at_four_entries() {
        let s = scene_of(vec![
            active_with("Edit x", "a"),
            active_with("Bash x", "b"),
            active_with("Read x", "c"),
            active_with("Write x", "d"),
            active_with("Grep x", "e"),
            active_with("Glob x", "f"),
        ]);
        let line = build_status_summary(&s, 200);
        // Six distinct tools, but only 4 should appear. Count `×` markers.
        let crosses = line.matches('×').count();
        assert_eq!(crosses, 4, "expected ≤4 tools in breakdown: {line:?}");
    }
}
