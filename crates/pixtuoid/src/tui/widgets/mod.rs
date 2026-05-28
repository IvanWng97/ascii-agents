//! Ratatui widget paint functions: footer, labels, wall display, tooltips,
//! ticker queue, and theme picker overlay.

mod hud;
mod tooltip;

pub(super) use hud::{
    paint_elevator_indicator, paint_footer, paint_theme_picker, paint_version_popup,
    paint_wall_display, version_popup_url_rect, VERSION_POPUP_URL,
};
pub use tooltip::paint_chitchat_bubbles;
pub(super) use tooltip::{paint_coffee_tooltip, paint_furniture_tooltip, paint_pet_tooltip};
pub(crate) use tooltip::{paint_hover_tooltip, paint_label_widgets};

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::SceneState;
use ratatui::style::Color;

fn to_color(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Persistent scrolling ticker queue. Messages append to the end and scroll
/// off the left naturally — like a news crawl. The queue rebuilds only when
/// the set of active tool details changes, preserving scroll continuity.
pub struct TickerQueue {
    buffer: String,
    last_snapshot: String,
}

impl Default for TickerQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl TickerQueue {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            last_snapshot: String::new(),
        }
    }

    pub fn update(&mut self, scene: &SceneState) {
        let mut items: Vec<String> = scene
            .agents
            .values()
            .filter(|a| a.exiting_at.is_none())
            .filter_map(|a| match &a.state {
                ActivityState::Active { detail, .. } => {
                    let tool = detail.as_deref().unwrap_or("working");
                    Some(format!("{}: {}", a.label, tool))
                }
                ActivityState::Waiting { reason } => Some(format!("{}: ?{}", a.label, reason)),
                _ => None,
            })
            .collect();
        items.sort();
        let snapshot = items.join("|");
        if snapshot != self.last_snapshot {
            self.last_snapshot = snapshot;
            for item in &items {
                self.buffer.push_str(item);
                self.buffer.push_str("  |  ");
            }
            const MAX_CHARS: usize = 512;
            let char_count = self.buffer.chars().count();
            if char_count > MAX_CHARS {
                let trim_chars = char_count - MAX_CHARS;
                if let Some((byte_idx, _)) = self.buffer.char_indices().nth(trim_chars) {
                    self.buffer.drain(..byte_idx);
                }
            }
        }
    }

    pub fn visible(&self, width: usize, now: SystemTime) -> String {
        if self.buffer.is_empty() {
            return String::new();
        }
        let elapsed_ms = now
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let chars: Vec<char> = self.buffer.chars().collect();
        let len = chars.len();
        let offset = (elapsed_ms / 150) as usize % len;
        (0..width).map(|i| chars[(offset + i) % len]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hud::build_status_summary;
    use pixtuoid_core::source::Activity;
    use pixtuoid_core::{AgentId, AgentSlot};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tooltip::truncate_label;

    #[test]
    fn truncate_label_passes_short_labels_through() {
        assert_eq!(truncate_label("hello", 16), "hello");
    }

    #[test]
    fn truncate_label_preserves_disambig_suffix() {
        let out = truncate_label("TikTok-Android\u{00b7}a09a", 16);
        assert_eq!(out.chars().count(), 16);
        assert!(out.ends_with("\u{00b7}a09a"), "suffix lost: {out}");
        assert!(out.starts_with("TikTok"), "base over-truncated: {out}");
    }

    #[test]
    fn truncate_label_falls_back_to_plain_truncate_when_no_separator() {
        let out = truncate_label("a-very-long-project-name", 8);
        assert_eq!(out, "a-very-l");
    }

    // --- build_status_summary ---------------------------------------------

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
            last_event_at: SystemTime::UNIX_EPOCH,
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
        let mut s = SceneState::uniform(16);
        for slot in slots {
            s.agents.insert(slot.agent_id, slot);
        }
        s
    }

    const QUIT_SUFFIX: &str = " [p]ause [t]heme [q]uit ";

    #[test]
    fn footer_zero_agents() {
        let s = scene_of(vec![]);
        let line = build_status_summary(&s, 80, None);
        assert_eq!(line.len(), 80, "should pad to full width");
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_single_idle_agent() {
        let s = scene_of(vec![idle("myproject")]);
        let line = build_status_summary(&s, 80, None);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_full_width_mixed_states() {
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
        let line = build_status_summary(&s, 120, None);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_medium_width_compact() {
        let s = scene_of(vec![
            active_with("Edit src/a.rs", "a"),
            waiting("b"),
            idle("c"),
        ]);
        let line = build_status_summary(&s, 60, None);
        assert!(
            !line.contains("3 agents"),
            "full tier should not fit at width 60"
        );
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_minimal_width() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let w = QUIT_SUFFIX.len() + 6;
        let line = build_status_summary(&s, w as u16, None);
        assert_eq!(line.len(), w);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_quit_only_below_threshold() {
        let s = scene_of(vec![idle("a")]);
        let w = QUIT_SUFFIX.len();
        let line = build_status_summary(&s, w as u16, None);
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_caps_tools_at_four() {
        let s = scene_of(vec![
            active_with("Edit x", "a"),
            active_with("Bash x", "b"),
            active_with("Read x", "c"),
            active_with("Write x", "d"),
            active_with("Grep x", "e"),
            active_with("Glob x", "f"),
        ]);
        let line = build_status_summary(&s, 200, None);
        let crosses = line.matches('\u{00d7}').count();
        assert_eq!(crosses, 4, "expected <=4 tools in breakdown");
        insta::assert_snapshot!(line);
    }

    #[test]
    fn footer_with_floor_info() {
        let s = scene_of(vec![idle("a"), idle("b")]);
        let line = build_status_summary(&s, 120, Some((2, 3)));
        insta::assert_snapshot!(line);
    }
}
