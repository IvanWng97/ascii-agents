//! Pantry chitchat — short speech-bubble conversations between agents
//! who happen to be visiting the same social waypoint (pantry, couch,
//! vending machine, printer).

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::AgentId;

use crate::tui::layout::{Point, WaypointKind};

/// Total duration of a single chitchat exchange (4 turns + silent gap).
pub const CHITCHAT_TOTAL_MS: u64 = 6_000;

/// Each speaker gets 1.5 s per turn.
const TURN_MS: u64 = 1_500;

/// Pool of dev-humor one-liners for speech bubbles.
pub const CHITCHAT_LINES: &[&str] = &[
    "git push -f",
    "// TODO",
    "LGTM!",
    "works on my",
    "ship it!",
    "npm install",
    "sudo !!",
    "404",
    "seg fault",
    "it compiled!",
    "rebase time",
    "merge pls",
    "async await",
    "rm -rf node_",
    "NaN === NaN",
    "overflow",
    "undefined?",
    "coffee++",
    "looks good",
    "trust me",
    "no tests?",
    "WONTFIX",
    "type: any",
    "blame git",
];

/// A live conversation between two agents at a waypoint.
pub struct ActiveChitchat {
    pub wp_key: (usize, usize),
    pub agent_a: AgentId,
    pub agent_b: AgentId,
    pub started_at: SystemTime,
    seed: u64,
}

impl ActiveChitchat {
    pub fn new(
        wp_key: (usize, usize),
        agent_a: AgentId,
        agent_b: AgentId,
        now: SystemTime,
    ) -> Self {
        let ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let seed = agent_a.raw().wrapping_mul(0x9e3779b97f4a7c15) ^ agent_b.raw() ^ ms;
        // Stable speaker assignment: lower raw id = agent_a.
        let (a, b) = if agent_a.raw() <= agent_b.raw() {
            (agent_a, agent_b)
        } else {
            (agent_b, agent_a)
        };
        Self {
            wp_key,
            agent_a: a,
            agent_b: b,
            started_at: now,
            seed,
        }
    }

    pub fn is_expired(&self, now: SystemTime) -> bool {
        self.elapsed_ms(now) >= CHITCHAT_TOTAL_MS
    }

    fn elapsed_ms(&self, now: SystemTime) -> u64 {
        now.duration_since(self.started_at)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(CHITCHAT_TOTAL_MS)
    }

    /// Returns `(speaker_is_a, line_text)` for the current turn, or `None`
    /// in the silent gap after the last turn.
    pub fn current_bubble(&self, now: SystemTime) -> Option<(bool, &'static str)> {
        let elapsed = self.elapsed_ms(now);
        if elapsed >= CHITCHAT_TOTAL_MS {
            return None;
        }
        let turn = elapsed / TURN_MS;
        if turn >= 4 {
            return None;
        }
        let is_speaker_a = turn % 2 == 0;
        let line_idx = (self.seed.wrapping_add(turn) as usize) % CHITCHAT_LINES.len();
        Some((is_speaker_a, CHITCHAT_LINES[line_idx]))
    }
}

/// Whether agents at this waypoint kind can start a chitchat.
pub fn supports_chitchat(kind: WaypointKind) -> bool {
    matches!(
        kind,
        WaypointKind::Pantry
            | WaypointKind::Couch
            | WaypointKind::VendingMachine
            | WaypointKind::Printer
    )
}

/// A single speech bubble ready for the widget layer to render.
pub struct ChitchatBubble {
    pub text: &'static str,
    /// Pixel coords of the speaking agent's anchor.
    pub anchor: Point,
}

/// Expire old conversations, start new ones where two agents share a
/// waypoint, and return the active bubbles for this frame.
pub fn update_and_collect(
    state: &mut HashMap<(usize, usize), ActiveChitchat>,
    floor_idx: usize,
    visitors: &[(usize, AgentId, Point)],
    now: SystemTime,
) -> Vec<ChitchatBubble> {
    // Expire old conversations.
    state.retain(|_, chat| !chat.is_expired(now));

    // Group visitors by waypoint index.
    let mut by_wp: HashMap<usize, Vec<(AgentId, Point)>> = HashMap::new();
    for &(wp_idx, agent_id, anchor) in visitors {
        by_wp.entry(wp_idx).or_default().push((agent_id, anchor));
    }

    let mut bubbles = Vec::new();

    for (wp_idx, agents) in &by_wp {
        if agents.len() < 2 {
            continue;
        }
        let key = (floor_idx, *wp_idx);

        // Create new conversation if none exists for this waypoint.
        state
            .entry(key)
            .or_insert_with(|| ActiveChitchat::new(key, agents[0].0, agents[1].0, now));

        // Generate bubble for active conversation.
        if let Some(chat) = state.get(&key) {
            if let Some((is_a, text)) = chat.current_bubble(now) {
                let speaker_id = if is_a { chat.agent_a } else { chat.agent_b };
                if let Some((_, anchor)) = agents.iter().find(|(id, _)| *id == speaker_id) {
                    bubbles.push(ChitchatBubble {
                        text,
                        anchor: *anchor,
                    });
                }
            }
        }
    }

    bubbles
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn base_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn id_a() -> AgentId {
        AgentId::from_transcript_path("/a.jsonl")
    }

    fn id_b() -> AgentId {
        AgentId::from_transcript_path("/b.jsonl")
    }

    #[test]
    fn test_expires_after_total_ms() {
        let start = base_time();
        let chat = ActiveChitchat::new((0, 0), id_a(), id_b(), start);
        let later = start + Duration::from_millis(7_000);
        assert!(chat.is_expired(later));
    }

    #[test]
    fn test_not_expired_before_total_ms() {
        let start = base_time();
        let chat = ActiveChitchat::new((0, 0), id_a(), id_b(), start);
        let later = start + Duration::from_millis(3_000);
        assert!(!chat.is_expired(later));
    }

    #[test]
    fn test_bubble_turn_a_then_b() {
        let start = base_time();
        let chat = ActiveChitchat::new((0, 0), id_a(), id_b(), start);

        // Turn 0 (0 ms) — speaker A
        let bubble_0 = chat.current_bubble(start);
        assert!(bubble_0.is_some());
        let (is_a_0, _) = bubble_0.unwrap();
        assert!(is_a_0, "turn 0 should be speaker A");

        // Turn 1 (1.5 s) — speaker B
        let bubble_1 = chat.current_bubble(start + Duration::from_millis(1_500));
        assert!(bubble_1.is_some());
        let (is_a_1, _) = bubble_1.unwrap();
        assert!(!is_a_1, "turn 1 should be speaker B");
    }

    #[test]
    fn test_no_bubble_after_4_turns() {
        let start = base_time();
        let chat = ActiveChitchat::new((0, 0), id_a(), id_b(), start);
        // 5.5 s = past 4 turns (4 * 1.5 = 6.0), but CHITCHAT_TOTAL_MS = 6.0
        // so at 5.5s we are in turn 3 (5500/1500 = 3), which is the 4th turn
        // (0-indexed). That's still valid. Let's check exactly at 6.0s.
        let at_6s = start + Duration::from_millis(6_000);
        assert!(chat.current_bubble(at_6s).is_none());
    }

    #[test]
    fn test_snippet_stable_same_seed() {
        let start = base_time();
        let chat1 = ActiveChitchat::new((0, 0), id_a(), id_b(), start);
        let chat2 = ActiveChitchat::new((0, 0), id_a(), id_b(), start);

        let b1 = chat1.current_bubble(start);
        let b2 = chat2.current_bubble(start);
        assert_eq!(b1.map(|b| b.1), b2.map(|b| b.1));
    }

    #[test]
    fn test_update_and_collect_creates_conversation() {
        let now = base_time();
        let mut state = HashMap::new();
        let visitors = vec![
            (0, id_a(), Point { x: 10, y: 20 }),
            (0, id_b(), Point { x: 15, y: 20 }),
        ];
        let bubbles = update_and_collect(&mut state, 0, &visitors, now);
        assert_eq!(state.len(), 1, "one conversation should be created");
        assert_eq!(bubbles.len(), 1, "one bubble should be emitted");
    }

    #[test]
    fn test_update_and_collect_no_conversation_for_single_visitor() {
        let now = base_time();
        let mut state = HashMap::new();
        let visitors = vec![(0, id_a(), Point { x: 10, y: 20 })];
        let bubbles = update_and_collect(&mut state, 0, &visitors, now);
        assert!(state.is_empty());
        assert!(bubbles.is_empty());
    }

    #[test]
    fn test_update_and_collect_expires_old() {
        let start = base_time();
        let mut state = HashMap::new();
        let visitors = vec![
            (0, id_a(), Point { x: 10, y: 20 }),
            (0, id_b(), Point { x: 15, y: 20 }),
        ];
        // Create conversation.
        update_and_collect(&mut state, 0, &visitors, start);
        assert_eq!(state.len(), 1);

        // Advance past expiry — conversation should be reaped and a new
        // one created (since both visitors are still present).
        let later = start + Duration::from_millis(7_000);
        update_and_collect(&mut state, 0, &visitors, later);
        assert_eq!(state.len(), 1, "old expired, new created");
    }

    #[test]
    fn test_supports_chitchat_kinds() {
        assert!(supports_chitchat(WaypointKind::Pantry));
        assert!(supports_chitchat(WaypointKind::Couch));
        assert!(supports_chitchat(WaypointKind::VendingMachine));
        assert!(supports_chitchat(WaypointKind::Printer));
        assert!(!supports_chitchat(WaypointKind::PhoneBooth));
        assert!(!supports_chitchat(WaypointKind::StandingDesk));
    }
}
