# Version Update Popup — Design Spec

## Overview

Show a modal popup on TUI startup when the user has upgraded to a new version. Displays hardcoded release notes (3-5 bullet points). Dismissed with Esc or Enter, which persists the current version to config so the popup doesn't reappear.

## Trigger

Compare `env!("CARGO_PKG_VERSION")` with `last_seen_version` from `AppConfig`. Show popup only if:
1. Current version is strictly newer (semver integer comparison)
2. `release_notes(current_ver)` returns `Some` (versions without notes are silent)

First launch (`last_seen_version == None`) also shows the popup if notes exist.

## Components

### `src/version.rs` (new)

- `is_newer_version(current, last_seen) -> bool` — split on `.`, parse as `(u64, u64, u64)`, compare. Returns `false` on parse error.
- `release_notes(version) -> Option<&'static [&'static str]>` — match on version string, returns bullet points.

### `config.rs` changes

- Add `last_seen_version: Option<String>` to `AppConfig` (serde rename `last-seen-version`).
- Add `save_version(path, version)` — same lock+atomic-rename pattern as `save`.

### `tui/widgets/hud.rs` changes

- Add `paint_version_popup(f, version, notes, bounds, theme)` — centered modal, width 54, variable height. Title: `" What's new in vX.Y.Z — Esc/Enter to dismiss "`. Bullets prefixed with `·`.

### `tui/mod.rs` changes

- `version_popup: bool` local, initialized from startup check.
- Input handler: highest priority (before theme picker). Esc/Enter dismisses and calls `save_version`.
- Passes state via `renderer.set_version_popup(version_popup)` each frame.

### `renderer.rs` / `tui_renderer.rs` changes

- `version_popup: bool` threaded through `DrawCtx` and `TuiRenderer`.
- Painted after theme picker in both normal and transition draw paths.

## UX

```
┌─ What's new in v0.4.0 — Esc/Enter to dismiss ─┐
│                                                  │
│  · Renamed from ascii-agents to pixtuoid         │
│  · Run `pixtuoid install-hooks` to update hooks  │
│  · New env vars: PIXTUOID_SOCKET/HOOK/LOG        │
│  · Flaky startup test fixed + 250ms rescan       │
│                                                  │
└──────────────────────────────────────────────────┘
```

## Testing

- `version.rs`: `is_newer_version` positive/negative/edge cases, `release_notes` known/unknown versions.
- `config.rs`: `save_version` persists, preserves existing fields, works on missing file.
- No widget visual tests needed (follows theme picker pattern which is already tested).
