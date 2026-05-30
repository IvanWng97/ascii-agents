//! Golden-fixture decode + coalescing harness.
//!
//! For each `tests/fixtures/<source>/<scenario>/` directory, decode the
//! transcript lines (via the source's `LineDecoder`) and the hook payloads
//! (via `decode_hook_payload`), then:
//!   1. snapshot the full decoded `AgentEvent` sequence (insta yaml), and
//!   2. assert every decoded event shares ONE `AgentId` — the hook↔JSONL
//!      coalescing contract that keeps regressing (a mismatch = two sprites
//!      for one session).
//!
//! Adding a CLI = drop a fixture dir + register its decoder in `decoder_for`.
//! No other test code; `cargo insta review` accepts the new snapshot.
//!
//! Snapshots stay portable because the decoder is fed the fixture's *relative*
//! path (a stable logical key), not the machine-specific absolute path —
//! `AgentId` is a deterministic FNV-1a hash of that key.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pixtuoid_core::source::antigravity::decode_ag_line;
use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::codex::decode_codex_line;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::jsonl::LineDecoder;
use pixtuoid_core::source::AgentEvent;

/// Map a fixture's source directory name to its JSONL line decoder.
/// Register a new CLI here (one line) — that plus a fixture dir is all it takes.
fn decoder_for(source: &str) -> LineDecoder {
    match source {
        "codex" => decode_codex_line,
        "claude-code" => decode_cc_line,
        "antigravity" => decode_ag_line,
        other => panic!("unknown fixture source {other:?} — register its decoder in decoder_for"),
    }
}

fn fixtures_root() -> PathBuf {
    // Dedicated subtree — `tests/fixtures/` also holds sprite/hook/jsonl
    // fixtures for other tests that are not per-source decode fixtures.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sources")
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .map(str::to_string)
        .filter(|l| !l.trim().is_empty())
        .collect()
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

/// Decode one fixture dir into the full ordered event stream (JSONL then hooks),
/// using `logical` as the stable transcript key fed to the decoders.
fn decode_fixture(source: &str, dir: &Path) -> Vec<AgentEvent> {
    // The transcript is the lone non-hook .jsonl in the dir.
    let transcript = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("jsonl")
                && p.file_name().and_then(|s| s.to_str()) != Some("hook-payloads.jsonl")
        })
        .unwrap_or_else(|| panic!("no transcript .jsonl in {}", dir.display()));

    // Stable logical key = path relative to fixtures_root (machine-independent).
    let logical = transcript
        .strip_prefix(fixtures_root())
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let decode = decoder_for(source);
    let mut events = Vec::new();

    for line in read_lines(&transcript) {
        let v: serde_json::Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("bad json in {}: {e}", transcript.display()));
        match decode(&logical, source, v) {
            Ok(evs) => events.extend(evs),
            Err(e) => panic!("decode error in {}: {e}", transcript.display()),
        }
    }

    let hooks = dir.join("hook-payloads.jsonl");
    if hooks.exists() {
        for line in read_lines(&hooks) {
            // `{{TRANSCRIPT_PATH}}` lets a path-keyed hook (CC) line up with its
            // transcript; Codex carries it too, to prove it's ignored.
            let line = line.replace("{{TRANSCRIPT_PATH}}", &logical);
            let v: serde_json::Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("bad hook json in {}: {e}", hooks.display()));
            match decode_hook_payload(v) {
                Ok(ev) => events.push(ev),
                Err(e) => panic!("hook decode error in {}: {e}", hooks.display()),
            }
        }
    }
    events
}

#[test]
fn all_source_fixtures_decode_and_coalesce() {
    let root = fixtures_root();
    let mut ran = 0;
    for source_dir in sorted_dirs(&root) {
        let source = source_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for scenario_dir in sorted_dirs(&source_dir) {
            let scenario = scenario_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let events = decode_fixture(&source, &scenario_dir);

            // Contract 1: the decoded event sequence is stable (golden snapshot).
            insta::assert_yaml_snapshot!(format!("{source}__{scenario}"), events);

            // Contract 2: hook + JSONL events for one session coalesce to ONE
            // AgentId. This is the dup-sprite bug class — assert it directly.
            let ids: BTreeSet<_> = events.iter().map(|e| e.agent_id()).collect();
            assert_eq!(
                ids.len(),
                1,
                "{source}/{scenario}: hook+JSONL events must coalesce to ONE agent_id, got {}: {:?}",
                ids.len(),
                ids
            );
            ran += 1;
        }
    }
    assert!(ran > 0, "no fixtures found under {}", root.display());
}
