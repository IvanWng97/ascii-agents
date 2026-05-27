# pixtuoid — Design Spec

**Status:** Draft v1
**Date:** 2026-05-20
**Working name:** `pixtuoid`

## 1. Overview

`pixtuoid` is a terminal-native, multi-agent visualizer for AI coding agents. Each running session of Claude Code (and, in later versions, Codex / Cursor / Copilot / Gemini / others) appears as a pixel-art human sitting at a desk inside a single ASCII office rendered in the terminal. Characters animate based on what their agent is currently doing — typing during tool calls, idle between actions, raising a speech bubble when the agent is waiting for user input or permission.

The project is inspired by [`pablodelucca/pixel-agents`](https://github.com/pablodelucca/pixel-agents), which renders the same concept as a pixel-art scene inside a VS Code webview. `pixtuoid` differs by being terminal-native: pure half-block + 24-bit color rendering, no Electron, no browser, runs over SSH.

### Prior art survey

Surveyed 2026-05-20. Closest projects in the multi-agent-visualizer space:

| Project | Medium | Notes |
|---|---|---|
| `pablodelucca/pixel-agents` | VS Code webview | The inspiration. Single canvas, multi-agent. |
| `rullerzhou-afk/clawd-on-desk` | Desktop pet (Electron-style window) | Closest in concept. Multi-agent (Claude, Codex, Cursor, Copilot, Gemini, Kimi, Kiro, opencode, OpenClaw, Hermes). Hook-primary integration. |
| `JamsusMaximus/codemap` ("CodeMap Hotel") | Web | Pixel-art hotel, up to 10 agents. |
| `DBell-workshop/AgentFleet` | Python + React + Phaser3 | RPG office, multi-agent collab. |
| `rolandal/pixel-agents-standalone` | Web fork of `pixel-agents` | |
| `asheshgoplani/agent-deck` | Go TUI | Multi-agent *session manager* — functional, not animated. |
| Claude Code "Buddy" (built-in) | Inline terminal ASCII | Single character, ASCII, 18 species, lives next to the input box. |

**Gap filled by `pixtuoid`:** terminal-native, multi-agent, pixel-art office. No equivalent exists today. Closest neighbors are desktop/web GUIs (clawd-on-desk and friends) and Claude Code's single-character Buddy.

## 2. Scope

### 2.1 v1 — first releasable cut

- Single Rust binary `pixtuoid`. Workspace with `pixtuoid-core` (library) + `pixtuoid` (binary) + `pixtuoid-hook` (tiny CC hook shim).
- **Source:** Claude Code only.
  - Primary: hooks registered in `~/.claude/settings.json`, dispatched to a Unix socket via `pixtuoid-hook`.
  - Fallback: tail-watch `~/.claude/projects/**/*.jsonl` transcripts for sessions where hooks aren't installed.
  - Deduplicated by `(session_id, event_id)` in the reducer so both running simultaneously is safe.
- **Scene:** one ASCII office in one terminal window. Each running CC session = one pixel-art human at a desk. Auto-assigned desks. Sessions despawn on `SessionEnd`.
- **Animation states:** `idle`, `typing` (any tool_use), `waiting` (Notification / permission). v1 collapses `Reading` and `Thinking` into `Typing` for simplicity.
- **Rendering:** tier-3 pixel-art sprites via half-block (`▀`) + 24-bit color. One bundled default character pack (~6 silhouettes). Per-agent shirt/hair recolor via session_id hash for visual distinguishability.
- **CLI surface:**
  - `pixtuoid` — runs the TUI.
  - `pixtuoid install-hooks` — merges hook entries into `~/.claude/settings.json`.
  - `pixtuoid uninstall-hooks` — removes them.
- **Interaction:** `q` / Ctrl-C to quit. No mouse. No input forwarded to agents.

### 2.2 v2 — deferred, design accommodates

- Source adapters: Codex (`~/.codex/hooks/`), Cursor (`~/.cursor/hooks.json`), Copilot (`~/.copilot/hooks/hooks.json`), Gemini (`~/.gemini/settings.json`), others.
- Walking + BFS pathfinding to assigned desks.
- More animation states: `reading` (Read/Grep/Glob), `thinking` (UserPromptSubmit), `celebrating` (Stop).
- Daemon mode (`pixtuoidd`) + detachable viewer; multiple viewers.
- Alternate renderers (web, GIF export, OBS source).
- Office layout editor.

### 2.3 Explicit non-goals

- Modifying or proxying Claude Code. Pure observer.
- Sending text/input back into agent sessions.
- In-app sprite editor — users edit `.sprite` / `pack.toml` externally.
- Cross-machine event collection.
- Sound.

## 3. Architecture

### 3.1 Workspace layout

```
pixtuoid/
├── Cargo.toml                       (workspace manifest)
├── crates/
│   ├── pixtuoid-core/           library — no terminal deps
│   ├── pixtuoid/                binary — ratatui + tokio + CLI
│   └── pixtuoid-hook/           tiny shim binary — runs from CC hook
├── assets/
│   └── sprites/
│       └── default/
│           ├── pack.toml            palette + animation manifest
│           ├── idle.sprite
│           ├── typing_0.sprite
│           ├── typing_1.sprite
│           ├── typing_2.sprite
│           └── waiting.sprite
└── docs/
    └── superpowers/specs/
        └── 2026-05-20-pixtuoid-design.md
```

### 3.2 `pixtuoid-core` — pure logic, headless-testable

```
src/
├── lib.rs                exposed traits + types
├── source/
│   ├── mod.rs            trait Source + AgentEvent enum
│   ├── claude_code.rs    impl Source for ClaudeCodeSource
│   ├── hook.rs           Unix-socket listener (shared infra)
│   └── jsonl.rs          fallback transcript tail-watcher
├── state/
│   ├── mod.rs            SceneState, AgentSlot, ActivityState
│   └── reducer.rs        AgentEvent → state mutation
├── sprite/
│   ├── mod.rs            Sprite, Frame, Palette, animator
│   ├── format.rs         .sprite + pack.toml loader
│   └── blit.rs           half-block blitter → RgbBuffer
└── render/
    └── mod.rs            trait Renderer
```

No `ratatui`, `crossterm`, or terminal-specific dependencies in this crate.

### 3.3 `pixtuoid` — binary

```
src/
├── main.rs               clap CLI dispatch
├── cli.rs                install-hooks / uninstall-hooks / run
├── runtime.rs            tokio runtime — tasks: source listener, reducer, renderer, input
├── tui/
│   ├── mod.rs            ratatui App, event loop, quit handling
│   └── renderer.rs       impl Renderer for TuiRenderer
└── install.rs            settings.json read/merge/write, atomic + advisory lock
```

### 3.4 `pixtuoid-hook` — shim

A < 100-LOC binary invoked by Claude Code as a hook command. Reads CC event JSON from stdin, augments it with `{event_type, matcher, ts, transcript_path}`, opens `/tmp/pixtuoid.sock`, writes one JSON line, exits.

Implemented as a separate binary (not a shell pipeline like `jq | nc`) so users need no runtime dependencies.

### 3.5 Key traits

```rust
// pixtuoid_core::source
#[async_trait]
pub trait Source: Send {
    fn name(&self) -> &str;
    async fn run(self, tx: mpsc::Sender<AgentEvent>) -> Result<()>;
}

pub enum AgentEvent {
    SessionStart  { agent_id: AgentId, source: String, session_id: String, cwd: PathBuf },
    ActivityStart { agent_id: AgentId, activity: Activity, detail: Option<String> },
    ActivityEnd   { agent_id: AgentId },
    Waiting       { agent_id: AgentId, reason: String },
    SessionEnd    { agent_id: AgentId },
}

pub enum Activity { Typing, Reading, Thinking } // v1 emits Typing only

// pixtuoid_core::render
pub trait Renderer {
    fn render(&mut self, scene: &SceneState) -> Result<()>;
}
```

`Source` is the only abstraction needed to add Codex / Cursor / Gemini in v2.
`Renderer` is the only abstraction needed to add web / GIF / daemon-client renderers later.

## 4. Data flow

### 4.1 End-to-end

```
CC tool call ──► CC fires hook ──► pixtuoid-hook (shim)
                                         │
                                         │  JSON line over Unix socket
                                         ▼
                                  /tmp/pixtuoid.sock
                                         │
                                         ▼
                       source::hook::SocketListener (in core)
                                         │  parses → AgentEvent
                                         ▼
                            mpsc::Sender<AgentEvent>
                                         │
                                         ▼
                    state::reducer (apply event → mutate SceneState)
                                         │
                                         ▼
                       render loop tick (~30 fps)
                                         │
                                         ▼
              TuiRenderer: pick sprite frame, blit half-blocks,
                          draw via ratatui + crossterm
```

### 4.2 Tokio tasks (binary)

1. **Hook socket listener** — accepts on `/tmp/pixtuoid.sock`, decodes JSON lines, forwards on channel.
2. **JSONL fallback watcher** — `notify` watches `~/.claude/projects/`, tails new `.jsonl` files, decodes, forwards on the same channel. Deduplicated against hook events in the reducer: hook events take precedence; a JSONL-derived event is dropped if a hook event from the same `session_id` arrived within a short window (`HOOK_WINS_WINDOW_MS`, default 500ms) carrying the same `tool_use_id`.
3. **Reducer task** — consumes `AgentEvent`s, mutates `Arc<RwLock<SceneState>>`.
4. **Render task** — fixed ~16ms tick. Ratatui's diff renderer skips unchanged cells.
5. **Input task** — crossterm event stream. Handles `q` / Ctrl-C.

### 4.3 Hook payload mapping (Claude Code → AgentEvent)

| CC event | `matcher` | AgentEvent produced (v1) |
|---|---|---|
| `SessionStart` | — | `SessionStart { agent_id = hash(transcript_path) }` |
| `PreToolUse` | `*` | `ActivityStart { activity: Typing, detail: "<tool>: <target>" }` |
| `PostToolUse` | `*` | `ActivityEnd` |
| `Notification` | — | `Waiting { reason }` |
| `SessionEnd` | — | `SessionEnd` |

v1 registers only those five events. v2 will distinguish `Read`/`Grep`/`Glob` → `Reading`, `UserPromptSubmit` → `Thinking`, etc.

### 4.4 `install-hooks`

1. Read `~/.claude/settings.json` (or create `{}`).
2. If file exists but doesn't parse as JSON, **abort** with a clear message. Do not overwrite.
3. Backup once on first install: copy to `settings.json.pixtuoid.bak`. Uninstall does not delete the backup.
4. Locate `pixtuoid-hook` (default: `$(which pixtuoid-hook)`, override with `--hook-path`).
5. Merge entries under `hooks.SessionStart`, `hooks.PreToolUse`, `hooks.PostToolUse`, `hooks.Notification`, `hooks.SessionEnd`. Each entry tagged with `"_pixtuoid": true` sentinel field for safe identification later.
6. Acquire advisory file lock (`fs2::FileExt::try_lock_exclusive`).
7. Write to `settings.json.tmp`, fsync, rename atomically.
8. Print confirmation listing each entry added.

`uninstall-hooks`: same locking + atomic-write pattern. Remove entries by sentinel field, write back.

## 5. Sprite & animation engine

### 5.1 Asset format

Text-based, version-controllable, hand-editable. Swapping to PNG later is additive (write a `PngSpriteLoader`).

**`assets/sprites/default/pack.toml`:**

```toml
[pack]
name    = "default"
version = "1"

[palette]
"." = "transparent"
"H" = "#2a1a0e"   # hair
"S" = "#f4c79a"   # skin
"e" = "#1a1a1a"   # eyes
"m" = "#a04040"   # mouth
"B" = "#2e62cf"   # shirt   ← per-agent recolor target

[animations.idle]
frames   = ["idle.sprite"]
frame_ms = 500

[animations.typing]
frames   = ["typing_0.sprite", "typing_1.sprite", "typing_2.sprite"]
frame_ms = 120

[animations.waiting]
frames   = ["waiting.sprite"]
frame_ms = 400
```

**`assets/sprites/default/idle.sprite`:** plain text. One palette key per pixel (single character), space-separated, one sprite row per file line. `@frame N` markers between frames. Comments via `#`. Trailing whitespace ignored.

### 5.2 Per-agent variation

On `SessionStart`, hash `session_id` → pick one of N preset shirt colors → override the `"B"` palette entry for that agent's render. Same for hair (`"H"`) using a separate hash bit. No additional sprite authoring required to make agents visually distinct.

### 5.3 Half-block blit math

Each terminal cell renders `▀` with:
- `fg = upper pixel RGB`
- `bg = lower pixel RGB`

A 12×16-pixel sprite → 12×8 terminal cells. Transparent pixels (`.`) inherit underlying cell content (scene background, desk surface).

### 5.4 Animation timing

Each agent slot carries `state: ActivityState` + `state_started_at: Instant`. Per render tick:

```rust
let elapsed_ms = (now - slot.state_started_at).as_millis();
let frame_idx  = (elapsed_ms / anim.frame_ms as u128) as usize % anim.frames.len();
```

Drift-free, no per-agent timers.

### 5.5 Scene composition (v1, no tile engine)

```
┌─────────── pixtuoid ─── 4 sessions ─── 23:04 ─────────────┐
│                                                               │
│   ░░░░░░░░  ░░░░░░░░  ░░░░░░░░  ░░░░░░░░                      │  ← desks
│   [sprite]  [sprite]  [sprite]  [sprite]                      │  ← characters
│   ▔▔▔▔▔▔   ▔▔▔▔▔▔    ▔▔▔▔▔▔   ▔▔▔▔▔▔                          │  ← desk surface
│   cc#1     cc#2       cc#3     cc#4                           │  ← labels
│   typing   idle       waiting? typing                         │
│                                                               │
└──────────────────────────────────────────────────── [q] quit ─┘
```

Up to 8 desks by default (configurable). Sessions occupy the leftmost free slot. Speech bubble overlays draw *after* sprite blit so they layer cleanly above the character.

## 6. Testing strategy

| Layer | Test type |
|---|---|
| `state::reducer` | Pure unit tests: feed `AgentEvent` sequence, assert `SceneState`. |
| `source::claude_code` decoder | Unit tests over canned hook-JSON + canned JSONL fixtures. |
| `sprite::format` loader | Unit tests over fixture `.sprite` + `pack.toml`. |
| `sprite::blit` | Snapshot tests: blit fixture sprite → assert `RgbBuffer` matches golden. |
| `source::hook` socket | Integration test: spin listener on temp socket, write JSON, assert channel output. |
| `cli install-hooks` | Integration test: temp `settings.json`, run install + uninstall, assert merge / restore. |
| End-to-end | `tests/e2e.rs` with a `MockSource` + a `TestRenderer` that captures `SceneState` snapshots. |

A `TestRenderer` implementation lives in `pixtuoid-core` (feature-gated) so end-to-end tests do not depend on ratatui.

## 7. Error handling

| Failure mode | Behavior |
|---|---|
| Hook socket malformed line | Log + skip. Listener keeps running. |
| Hook socket I/O error | Log + reconnect loop. |
| JSONL parse error | Log + skip line. Never blocks. |
| Sprite asset missing at startup | Hard fail with clear message. |
| Sprite animation key missing at render time | Log once, fall back to `idle`. Never panic. |
| `~/.claude/settings.json` exists but is invalid JSON | Abort install with message; do not overwrite. |
| Concurrent installs | Advisory file lock — second install waits or aborts cleanly. |
| First-time install | Write `settings.json.pixtuoid.bak` once. Uninstall leaves the backup in place. |

## 8. Dependencies

### `pixtuoid-core`

```toml
tokio        = { version = "1",  features = ["sync", "rt", "macros", "fs", "net", "time"] }
serde        = { version = "1",  features = ["derive"] }
serde_json   = "1"
notify       = "6"
anyhow       = "1"
thiserror    = "1"
async-trait  = "0.1"
toml         = "0.8"
tracing      = "0.1"
```

### `pixtuoid` (binary)

```toml
pixtuoid-core  = { path = "../pixtuoid-core" }
ratatui            = "0.28"
crossterm          = "0.28"
clap               = { version = "4", features = ["derive"] }
tokio              = { version = "1", features = ["rt-multi-thread", "macros", "signal"] }
tracing-subscriber = "0.3"
fs2                = "0.4"   # advisory file lock for settings.json
```

### `pixtuoid-hook` (shim)

```toml
serde_json = "1"
```

## 9. Open questions for future work

These are *not* v1 blockers. Logged here so they aren't lost when v2 planning starts.

- **Hook payload schema stability.** CC's hook JSON shape can drift across versions. v2 should consider a small adapter layer per CC version with capability detection.
- **Daemon transport choice.** Unix socket for v1 hook ingest is fine, but a daemon split (v2) may benefit from a richer protocol (length-prefixed framing or a tiny RPC like `tarpc`). Punted.
- **Sprite asset distribution.** v1 bundles the default pack inside the binary via `include_str!`. v2 should consider a `~/.config/pixtuoid/packs/` discovery path for user packs.
- **Multi-window terminal support.** If a user runs CC sessions in tmux windows + iTerm tabs simultaneously, current design shows them all in one office; that is intentional, but UX may want filtering controls later.
