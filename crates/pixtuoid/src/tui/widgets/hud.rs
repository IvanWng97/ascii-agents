use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::SceneState;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::{to_color, TickerQueue};
use crate::tui::renderer::clip_widget_rect;

pub(in crate::tui) fn paint_theme_picker(
    f: &mut ratatui::Frame<'_>,
    selected: usize,
    bounds: Rect,
    theme: &crate::tui::theme::Theme,
) {
    use crate::tui::theme;
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};
    use ratatui::widgets::{Block, Borders, Clear};

    let w = 30u16;
    let h = (theme::ALL_THEMES.len() as u16 + 2).min(bounds.height);
    let x = bounds.width.saturating_sub(w) / 2;
    let y = bounds.height.saturating_sub(h) / 2;
    let area = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, area);
    let items: Vec<Line> = theme::ALL_THEMES
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let prefix = if i == selected { "▸ " } else { "  " };
            let style = if i == selected {
                Style::default()
                    .fg(to_color(theme.ui.neon_brand))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(to_color(theme.ui.label_idle))
            };
            Line::from(TSpan::styled(format!("{prefix}{}", t.name), style))
        })
        .collect();
    let block = Block::default()
        .title(" Theme [↑↓/jk] Enter/Esc ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(to_color(theme.ui.neon_brand)))
        .style(Style::default().bg(to_color(theme.ui.tooltip_bg)));
    f.render_widget(Paragraph::new(items).block(block), area);
}

pub(in crate::tui) fn paint_footer(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    full_rect: Rect,
    theme: &crate::tui::theme::Theme,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
) {
    let summary = build_status_summary(scene, full_rect.width, floor_info);
    let footer = Paragraph::new(Span::raw(summary))
        .style(Style::default().fg(to_color(theme.ui.label_idle)));
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
pub(in crate::tui) fn build_status_summary(
    scene: &SceneState,
    term_width: u16,
    floor_info: Option<crate::tui::renderer::FloorInfo>,
) -> String {
    let n = scene.agents.len();
    // Multi-floor view always shows `n/total` so the total stays visible
    // even when an agent migrates and per-floor matches total transiently.
    let count_str = match floor_info {
        Some(fi) => format!("{n}/{}", fi.total_agents),
        None => format!("{n}"),
    };
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
                    let token = d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
                    if !token.is_empty() {
                        *tool_counts.entry(token).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    let floor_suffix = match floor_info {
        Some(fi) => format!(" F{}/{} [\u{2191}\u{2193}]", fi.current, fi.total_floors),
        None => String::new(),
    };
    let quit_base = " [p]ause [t]heme [q]uit ";
    let quit = format!("{floor_suffix}{quit_base}");
    let tools_str = {
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
        format!(" {count_str} agents ")
    } else {
        let mut s =
            format!(" {count_str} agents · {active} active · {waiting} waiting · {idle} idle");
        if !tools_str.is_empty() {
            s.push_str(" · ");
            s.push_str(&tools_str);
        }
        s.push(' ');
        s
    };
    // Narrow tiers use bare `n` — "5/12a" parses as "5 slash 12a" at a glance.
    let stats_medium = format!(" {n}a · {active}A · {waiting}W · {idle}I ");
    let stats_min = format!(" {n}a ");

    let w = term_width as usize;
    let q = quit.len();
    for stats in [&stats_full, &stats_medium, &stats_min] {
        if stats.len() + q <= w {
            let pad = w.saturating_sub(stats.len() + q);
            let mut out = String::with_capacity(w);
            out.push_str(stats);
            for _ in 0..pad {
                out.push(' ');
            }
            out.push_str(&quit);
            return out;
        }
    }
    quit
}

pub(in crate::tui) fn paint_wall_display(
    f: &mut ratatui::Frame<'_>,
    scene: &SceneState,
    scene_rect: Rect,
    now: SystemTime,
    ticker: &TickerQueue,
    theme: &crate::tui::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let cell_x = scene_rect.x + 2;
    let cell_y = scene_rect.y + 1;

    let live: Vec<&pixtuoid_core::AgentSlot> = scene
        .agents
        .values()
        .filter(|a| a.exiting_at.is_none())
        .collect();
    let active = live
        .iter()
        .filter(|a| matches!(a.state, ActivityState::Active { .. }))
        .count();
    let waiting = live
        .iter()
        .filter(|a| matches!(a.state, ActivityState::Waiting { .. }))
        .count();
    let idle = live.len() - active - waiting;

    let version = env!("CARGO_PKG_VERSION");
    let top_spans = vec![
        Span::styled(
            format!("pixtuoid v{version}"),
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "\u{2605} Star",
            Style::default()
                .fg(to_color(theme.ui.neon_star))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let top_line = Line::from(top_spans);

    let oldest = live
        .iter()
        .filter_map(|a| now.duration_since(a.created_at).ok())
        .max()
        .unwrap_or_default();
    let uptime_secs = oldest.as_secs();
    let uptime_str = if uptime_secs >= 3600 {
        format!(
            "\u{2191}{}h{}m",
            uptime_secs / 3600,
            (uptime_secs % 3600) / 60
        )
    } else if uptime_secs >= 60 {
        format!("\u{2191}{}m", uptime_secs / 60)
    } else {
        "\u{2191}<1m".to_string()
    };

    let bot_line = Line::from(vec![
        Span::styled(
            "\u{25cf}".repeat(active),
            Style::default().fg(to_color(theme.ui.label_active)),
        ),
        Span::styled(
            "\u{25cf}".repeat(waiting),
            Style::default().fg(to_color(theme.ui.label_waiting)),
        ),
        Span::styled(
            "\u{25cf}".repeat(idle),
            Style::default().fg(to_color(theme.ui.label_idle)),
        ),
        Span::raw("  "),
        Span::styled(uptime_str, Style::default().fg(Color::DarkGray)),
    ]);

    let ticker_width = 28usize;
    let visible = ticker.visible(ticker_width, now);
    let ticker_line = Line::from(Span::styled(
        visible,
        Style::default().fg(to_color(theme.ui.neon_ticker)),
    ));

    let w = 30u16;
    if let Some(r) = clip_widget_rect(
        Rect {
            x: cell_x,
            y: cell_y,
            width: w,
            height: 3,
        },
        scene_rect,
    ) {
        f.render_widget(Paragraph::new(vec![top_line, bot_line, ticker_line]), r);
    }
}

/// URL shown on the "More details" line and opened on click.
pub(in crate::tui) const VERSION_POPUP_URL: &str = "https://github.com/IvanWng97/pixtuoid/releases";
/// Prefix rendered before the URL. Its byte-length determines the URL's
/// click-rect x-offset; keep `paint_version_popup` and
/// `version_popup_url_rect` consistent by using this constant.
const URL_PREFIX: &str = "  More details: ";

pub(in crate::tui) fn paint_version_popup(
    f: &mut ratatui::Frame<'_>,
    version: &str,
    notes: &[&str],
    bounds: Rect,
    theme: &crate::tui::theme::Theme,
    scale: f32,
) {
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span as TSpan};
    use ratatui::widgets::{Block, Borders, Clear};

    let needed_w = 2 + URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2;
    let scale = scale.clamp(0.0, 1.0);
    if scale <= 0.01 {
        return; // fully dismissed, skip render
    }
    let w_full = needed_w.min(bounds.width);
    let h_full = (notes.len() as u16 + 6).min(bounds.height);
    let w = ((w_full as f32 * scale).round() as u16).max(2);
    let h = ((h_full as f32 * scale).round() as u16).max(2);
    let x = bounds.x + bounds.width.saturating_sub(w) / 2;
    let y = bounds.y + bounds.height.saturating_sub(h) / 2;
    let area = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, area);

    let mut items: Vec<Line> = Vec::with_capacity(notes.len() + 3);
    items.push(Line::from(""));
    for note in notes {
        items.push(Line::from(TSpan::styled(
            format!("  \u{00b7} {note}"),
            Style::default().fg(to_color(theme.ui.label_idle)),
        )));
    }
    items.push(Line::from(""));
    items.push(Line::from(vec![
        TSpan::styled(
            URL_PREFIX,
            Style::default().fg(to_color(theme.ui.label_idle)),
        ),
        TSpan::styled(
            VERSION_POPUP_URL,
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::UNDERLINED),
        ),
    ]));

    let title = format!(" What's new in v{version} \u{2014} Enter to close ");
    let block = Block::default()
        .title(TSpan::styled(
            title,
            Style::default()
                .fg(to_color(theme.ui.neon_brand))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(to_color(theme.ui.neon_brand)))
        .style(Style::default().bg(to_color(theme.ui.tooltip_bg)));

    f.render_widget(Paragraph::new(items).block(block), area);
}

/// Computes the screen rect of the clickable URL inside the version popup.
/// Returns None if the popup would be too small to render. Mirrors the
/// geometry inside `paint_version_popup` (kept in sync by sharing the same
/// width calculation).
pub(in crate::tui) fn version_popup_url_rect(
    notes_len: usize,
    bounds: Rect,
    scale: f32,
) -> Option<Rect> {
    let scale = scale.clamp(0.0, 1.0);
    if scale < 0.7 {
        return None; // URL not clickable until popup reaches 70% scale
    }
    let needed_w = 2 + URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2;
    // Mirror paint_version_popup's geometry exactly: clamp to bounds first,
    // then scale, then derive popup_x/popup_y from the SCALED w/h. Centering
    // off the unscaled w/h leaves the click rect offset from the painted
    // popup at any scale < 1.0.
    let w_full = needed_w.min(bounds.width);
    let h_full = (notes_len as u16 + 6).min(bounds.height);
    let w = ((w_full as f32 * scale).round() as u16).max(2);
    let h = ((h_full as f32 * scale).round() as u16).max(2);
    if w < 4 || h < 3 {
        return None;
    }
    let popup_x = bounds.x + bounds.width.saturating_sub(w) / 2;
    let popup_y = bounds.y + bounds.height.saturating_sub(h) / 2;
    // URL line layout inside popup (Block with Borders::ALL has 1-cell border):
    //   y = popup_y + 1 (border) + 1 (blank) + notes_len (notes) + 1 (blank)
    //   x = popup_x + 1 (border) + URL_PREFIX.len()
    let url_y = popup_y + notes_len as u16 + 3;
    let url_x = popup_x + 1 + URL_PREFIX.len() as u16;

    // Clip against the popup's inner content area: when the painter clipped
    // the envelope (narrow / short terminal), the URL rect must shrink too —
    // otherwise clicks past the visible popup register as URL clicks.
    let inner_right = popup_x + w - 1; // bottom-right border column (exclusive)
    let inner_bottom = popup_y + h - 1; // bottom border row (exclusive)
    if url_x >= inner_right || url_y >= inner_bottom {
        return None;
    }
    let width = (VERSION_POPUP_URL.len() as u16).min(inner_right - url_x);
    if width == 0 {
        return None;
    }
    Some(Rect {
        x: url_x,
        y: url_y,
        width,
        height: 1,
    })
}

pub(in crate::tui) fn paint_elevator_indicator(
    f: &mut ratatui::Frame<'_>,
    door: crate::tui::layout::Point,
    current_floor: usize,
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    let label = format!(" \u{25b2} F{current_floor} \u{25bc} ");
    let label_w = label.len() as u16;
    let door_cell_x = door.x + 8u16.saturating_sub(label_w / 2);
    let door_cell_y = door.y / 2;
    let indicator_y = door_cell_y.saturating_sub(1);

    if let Some(r) = crate::tui::renderer::clip_widget_rect(
        Rect {
            x: scene_rect.x + door_cell_x,
            y: scene_rect.y + indicator_y,
            width: label_w,
            height: 1,
        },
        scene_rect,
    ) {
        let style = Style::default()
            .fg(to_color(theme.ui.neon_brand))
            .bg(to_color(theme.ui.tooltip_bg))
            .add_modifier(Modifier::BOLD);
        f.render_widget(Paragraph::new(Line::from(Span::styled(label, style))), r);
    }
}

#[cfg(test)]
mod hud_tests {
    use super::*;

    fn full_bounds(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn url_rect_fits_inside_normal_popup() {
        let rect = version_popup_url_rect(4, full_bounds(200, 60), 1.0).expect("should fit");
        assert_eq!(rect.width, VERSION_POPUP_URL.len() as u16);
        assert_eq!(rect.height, 1);
    }

    // Regression for the phantom-browser-launch bug: on a narrow terminal
    // the painter clips the popup envelope, but the URL click rect used to
    // extend past the visible popup's right edge, registering clicks on the
    // scene behind as URL clicks. The rect must stay inside the envelope.
    #[test]
    fn url_rect_does_not_extend_past_clipped_popup_right_edge() {
        let bounds = full_bounds(50, 30);
        if let Some(rect) = version_popup_url_rect(4, bounds, 1.0) {
            let needed_w = 2 + URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2;
            let w = needed_w.min(bounds.width);
            let popup_x = bounds.width.saturating_sub(w) / 2;
            let popup_inner_right = popup_x + w - 1;
            assert!(
                rect.x + rect.width <= popup_inner_right,
                "url rect cols {}..{} extend past popup inner-right {}",
                rect.x,
                rect.x + rect.width,
                popup_inner_right
            );
        }
    }

    // Regression: at scale < 1.0 the URL click rect must center off the
    // SCALED width, mirroring paint_version_popup. Centering off unscaled
    // w shifts the click area ~((1-scale)*needed_w)/2 columns left of the
    // painted URL.
    #[test]
    fn url_rect_centering_matches_painter_at_partial_scale() {
        let bounds = full_bounds(200, 60);
        let scale = 0.85; // ≥ 0.7 gate and ≥ 0.8 vertical threshold for notes_len=4
        let needed_w = 2 + URL_PREFIX.len() as u16 + VERSION_POPUP_URL.len() as u16 + 2;
        let w_full = needed_w.min(bounds.width);
        let w_scaled = ((w_full as f32 * scale).round() as u16).max(2);
        let expected_popup_x = bounds.width.saturating_sub(w_scaled) / 2;
        let expected_url_x = expected_popup_x + 1 + URL_PREFIX.len() as u16;
        let rect = version_popup_url_rect(4, bounds, scale)
            .expect("url rect should exist at scale=0.85 with notes_len=4");
        assert_eq!(
            rect.x, expected_url_x,
            "url click rect x={} must match painter's scaled-centering popup_x+1+prefix={}",
            rect.x, expected_url_x
        );
    }

    // Regression for the off-screen URL row bug: on a too-short terminal,
    // the painter clips the popup envelope vertically, and the URL row used
    // to land on or below the clipped bottom border (where ratatui never
    // paints it). The rect must return None instead.
    #[test]
    fn url_rect_returns_none_when_url_row_falls_outside_clipped_popup() {
        // notes_len=4 → needed h=10. With bounds.height=8 the popup clips
        // to h=8, leaving room for at most ~3 notes — the URL row at offset
        // (notes_len + 3) = 7 lands on the bottom border.
        let rect = version_popup_url_rect(4, full_bounds(200, 8), 1.0);
        assert!(
            rect.is_none(),
            "expected None when URL row falls on the clipped popup's bottom border: got {rect:?}"
        );
    }
}
