//! Codex CLI source. Watches the Codex session transcript
//! (`~/.codex/sessions/**/rollout-<ts>-<UUID>.jsonl`) via `JsonlWatcher`.
//! Codex hooks already arrive through the shared hook socket (the shim stamps
//! `source=codex`); this source adds the JSONL lifecycle signals hooks lack —
//! most importantly the post-approval resume (`function_call_output`).
//!
//! Coalescing: hook events key `AgentId` on the hook `session_id`; this source
//! keys on the trailing UUID of the rollout filename. Verified equal
//! (hook.session_id == session_meta.id == filename UUID), so both transports
//! merge onto one sprite.
#![allow(unused_imports)]

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::source::decoder::make_tool_detail;
use crate::source::jsonl::JsonlWatcher;
use crate::source::{Activity, AgentEvent, Source, TaggedSender};
use crate::AgentId;

pub const SOURCE_NAME: &str = "codex";

/// Trailing canonical UUID (`8-4-4-4-12`) of a `rollout-<ts>-<UUID>.jsonl`
/// filename. Equals the hook payload's `session_id`, so hook and JSONL events
/// coalesce. Falls back to the full stem if no trailing UUID is present.
pub fn codex_id_from_path(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let tail = &stem[stem.len().saturating_sub(36)..];
    if is_uuid(tail) {
        tail.to_string()
    } else {
        stem.to_string()
    }
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_from_rollout_path_is_trailing_uuid() {
        let p = Path::new(
            "/Users/me/.codex/sessions/2026/05/29/rollout-2026-05-29T22-36-52-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl",
        );
        // Must equal the hook session_id for coalescing.
        assert_eq!(codex_id_from_path(p), "019e7762-9ded-7e33-be41-946ecf105bf4");
    }

    #[test]
    fn id_falls_back_to_stem_without_uuid() {
        let p = Path::new("/tmp/notarollout.jsonl");
        assert_eq!(codex_id_from_path(p), "notarollout");
    }
}
