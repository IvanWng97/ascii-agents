use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AgentId(u64);

impl AgentId {
    /// CC-specific shortcut — same as `from_parts("claude-code", path)`. Kept
    /// for backwards compatibility with the existing CC decoder.
    pub fn from_transcript_path(path: &str) -> Self {
        Self::from_parts("claude-code", path)
    }

    /// Source-agnostic factory. `source` is the source's name (matches the
    /// `Source::name()` return value, e.g. `"claude-code"`, `"codex"`,
    /// `"cursor"`); `opaque_id` is whatever the source uses to uniquely
    /// identify a session — a JSONL path for CC, a session UUID for an
    /// SDK source, a socket path for a hook-based source. The pair is
    /// hashed so two sources with the same `opaque_id` produce distinct
    /// `AgentId`s (no cross-source collisions).
    pub fn from_parts(source: &str, opaque_id: &str) -> Self {
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in source.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        // Domain separator between source and opaque id so e.g. source="a",
        // opaque="bc" doesn't collide with source="ab", opaque="c".
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
        for b in opaque_id.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        AgentId(hash)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_is_deterministic_per_path() {
        let a = AgentId::from_transcript_path("/Users/me/.claude/projects/x/abc.jsonl");
        let b = AgentId::from_transcript_path("/Users/me/.claude/projects/x/abc.jsonl");
        assert_eq!(a, b);
    }

    #[test]
    fn agent_id_differs_per_path() {
        let a = AgentId::from_transcript_path("/Users/me/.claude/projects/x/abc.jsonl");
        let b = AgentId::from_transcript_path("/Users/me/.claude/projects/x/def.jsonl");
        assert_ne!(a, b);
    }

    #[test]
    fn agent_id_displays_as_hex() {
        let id = AgentId::from_transcript_path("x");
        assert_eq!(format!("{id}").len(), 16);
    }

    #[test]
    fn from_parts_distinguishes_source_and_opaque() {
        // Two sources with the same opaque_id must NOT collide.
        let cc = AgentId::from_parts("claude-code", "session-123");
        let cx = AgentId::from_parts("codex", "session-123");
        assert_ne!(cc, cx);
    }

    #[test]
    fn from_parts_has_domain_separator() {
        // ("a", "bc") must NOT hash the same as ("ab", "c") — proves the
        // domain separator between source and opaque_id is doing its job.
        let a = AgentId::from_parts("a", "bc");
        let b = AgentId::from_parts("ab", "c");
        assert_ne!(a, b);
    }

    #[test]
    fn from_transcript_path_routes_through_from_parts() {
        // Backwards-compat: existing CC IDs are exactly
        // `from_parts("claude-code", path)`.
        let a = AgentId::from_transcript_path("/x.jsonl");
        let b = AgentId::from_parts("claude-code", "/x.jsonl");
        assert_eq!(a, b);
    }
}
