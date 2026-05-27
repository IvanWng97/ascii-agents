# CLAUDE.md

Instructions for Claude Code (or any AI coding agent) working in this repo.

## What this is

Terminal-native, multi-agent pixel-art visualizer for AI coding agents. Each running CC (Claude Code) session shows up as an animated half-block sprite in an ASCII office. Built in Rust as a Cargo workspace of three crates.

User-facing overview: [`README.md`](README.md).
v1 spec: [`docs/superpowers/specs/2026-05-20-ascii-agents-design.md`](docs/superpowers/specs/2026-05-20-ascii-agents-design.md).
v1 plan (28 TDD-shaped tasks): [`docs/superpowers/plans/2026-05-20-ascii-agents-v1.md`](docs/superpowers/plans/2026-05-20-ascii-agents-v1.md).

## Layout

```
crates/
├── ascii-agents-core/      headless lib — no terminal deps (ratatui/crossterm forbidden here)
│   ├── source/             Source trait, hook+jsonl decoders, listeners, SourceManager
│   ├── state/              SceneState + Reducer (with Transport-tagged dedup + Active→Idle debounce)
│   ├── sprite/             .sprite parser, pack.toml loader, half-block blitter, animator
│   ├── render/             Renderer trait + TestRenderer (feature = "test-renderer")
│   ├── layout/             zone-based office geometry (terminal-agnostic):
│   │                       mod.rs (SceneLayout struct, Bounds, Point, constants, accessors),
│   │                       compute.rs (compute_with_seed + 4 private helpers),
│   │                       decor.rs (WaypointKind, WallDecor, PlantKind, PodDecor),
│   │                       mask.rs (build_walkable_mask — obstacle stamping for A*)
│   ├── pose.rs             pure state→pose derivation + wander state machine (no terminal deps)
│   ├── walkable.rs         WalkableMask (static bool grid) + OccupancyOverlay (dynamic per-frame)
│   └── tests/              one integration test per concern
├── ascii-agents/           binary — ratatui + crossterm + tokio + clap
│   ├── cli.rs              clap subcommands (run / install-hooks / uninstall-hooks)
│   ├── config.rs           AppConfig persistence (~/.config/ascii-agents/config.toml), XDG-aware
│   ├── runtime.rs          tokio task wiring (source ── (Transport, AgentEvent) ──► reducer ──► renderer)
│   ├── install/            settings.json merge, atomic write, advisory lock, stow-symlink safe
│   └── tui/                ratatui App + TuiRenderer (Renderer trait impl)
│       ├── renderer.rs     draw_scene orchestrator (DrawCtx struct), half-block flush, terminal lifecycle
│       ├── widgets/        ratatui widget paint fns, split into sub-modules:
│       │                   mod.rs (TickerQueue, shared helpers), hud.rs (footer, wall display,
│       │                   elevator indicator, theme picker), tooltip.rs (hover, cat, coffee,
│       │                   furniture, labels, chitchat bubbles)
│       ├── hit_test.rs     mouse hit-test: agent hover, coffee machine click, furniture tooltips
│       ├── tui_renderer.rs Renderer trait impl — owns cross-frame state (RgbBuffer, FrameCache, Router, PoseHistory, TickerQueue, Theme, cached Layout)
│       ├── theme/          color theme system — one file per theme, Theme struct in mod.rs
│       │                   mod.rs (struct defs + ALL_THEMES registry), normal.rs, cyberpunk.rs,
│       │                   dracula.rs, tokyo_night.rs, catppuccin.rs, gruvbox.rs
│       ├── pose.rs         routed pose layer (PoseHistory, derive_with_routing, snap-back) — re-exports core::pose
│       ├── pathfind.rs     Router trait + AStarRouter with selective cache invalidation
│       └── pixel_painter/  pure-pixel pass — split into focused child modules:
│                           mod.rs (PixelCtx struct, orchestrator), background/ (weather, sunset, skyline),
│                           drawable.rs (y-sort), effects.rs (glow/z's/dots/steam/dust/bubble),
│                           palette.rs (tool_glow_tint), anchors.rs (breath, walk position, character_anchor),
│                           furniture.rs (coffee table, area rug, side table, pantry table/chair)
└── ascii-agents-hook/      tiny shim CC invokes — stdin JSON → Unix socket, 200ms write timeout
│   └── sprites/
│       ├── default/        coworking-lounge pack (embedded via include_str!)
│       ├── robot/          proof-of-concept robot character pack (loadable via --pack-dir)
│       └── skeleton/       template pack for custom sprite creation (extracted via init-pack)
scripts/                    preflight.sh (CI mirror), crop-snapshot.py (visual verification)
```

## Build & test

```
cargo build --workspace                                              # debug build
cargo build --release --workspace                                    # release build
cargo test --workspace --features ascii-agents-core/test-renderer    # all tests (200+)
cargo run --release --example snapshot -- /tmp/snap.png              # render TUI to PNG
./target/release/ascii-agents run --headless --projects-root ~/.claude/projects   # live test against real CC
```

The `test-renderer` feature is needed for the `e2e.rs` integration test. The dev workspace test alias is just `cargo test`.

### Visual verification (Python venv)

```
python3 -m venv .venv
.venv/bin/pip install -r requirements-dev.txt
cargo build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/snap.png --scale 3
```

See `.claude/skills/beautify-decoration/SKILL.md` for the full iteration loop, self-critique checklist, and sprite-format pitfalls.

### Pre-commit / pre-push preflight

`scripts/preflight.sh` mirrors `.github/workflows/ci.yml` (rustfmt + cargo-machete +
cargo-deny + clippy with `-D warnings` + workspace tests). Run it locally to avoid
the round-trip of "push → wait for CI → red → fix → push again."

`.githooks/pre-commit` runs `cargo fmt --all --check` only (sub-second).
`.githooks/pre-push` runs the full preflight before pushing.

Activate hooks **once per clone**:

```
git config core.hooksPath .githooks
```

Bypass in an emergency with `git commit --no-verify` or `SKIP_PREFLIGHT=1 git push`.

## Conventions

- **TDD first.** Plan and existing tests are TDD-shaped — failing test → minimal impl → commit. Don't add code without a test that exercises it.
- **DRY, YAGNI.** No features beyond what v1 specifies. v2 items are deferred — adding them in v1 code is a regression.
- **No comments unless WHY.** Don't write comments that restate what the code does. Comment only when a future reader can't tell from the code why something is the way it is (a workaround, a non-obvious constraint, a surprising invariant).
- **Errors propagate via `anyhow::Result` in app code, `thiserror` in core if a typed error becomes load-bearing.** The hook listener and JSONL watcher log + continue on malformed input — they never panic.
- **No `unwrap()` in non-test code.** Tests can unwrap freely.
- **Match the surrounding shell:** scripts in this repo target zsh (interactive) or POSIX sh. `shellcheck` any `.sh` you touch.
- **macOS first.** BSD-flavored CLI, brew, launchd for daemons. The hook shim is Unix-socket specific (`std::os::unix::net::UnixStream`).
- **Keep docs current.** When a change alters module structure, architecture, developer workflow, or the public API surface, update CLAUDE.md and README.md in the same commit. Stale docs cost more than the 5 minutes to update them.
- **Sprite changes require visual verification.** After editing any `.sprite` file: (1) rebuild the snapshot example, (2) render at `--cols 192 --rows 80`, (3) crop the relevant quadrant with `scripts/crop-snapshot.py --scale 3`, (4) read the cropped PNG and self-critique — does a stranger recognize the intended pose/object? Iterate until it reads at half-block scale. Then rebuild the release binary (`cargo build --release --workspace`). Commit messages should include iteration history (which designs were tried and why they were rejected). See `.claude/skills/beautify-decoration/SKILL.md` for the full checklist.

## Architecture invariants

These are load-bearing; don't break them without updating the spec.

1. **`ascii-agents-core` has no terminal dependencies.** No `ratatui`, no `crossterm`, no `stdout` writes. If you need one, the abstraction belongs behind the `Renderer` trait.
2. **Events flow through ONE channel** typed `mpsc::Sender<(Transport, AgentEvent)>`. The `Transport` tag is load-bearing — the reducer uses it for hook-wins dedup. Do not hardcode `Transport::Hook` on the consumer side; the producer (each Source impl) tags its own events.
3. **`Source` trait is the only seam for adding agent CLIs** (Codex / Cursor / Copilot). Don't bypass it. Per-source JSONL format knowledge lives in the source's own decoder fn (injected into `JsonlWatcher` via fn pointers), not in a shared decoder.
4. **`install-hooks` writes through symlinks.** `resolve_symlink` in `install/io.rs` is critical for stow-managed `~/.claude/settings.json`. Don't replace it with `fs::rename` on the symlink path.
5. **The hook shim must never block CC.** Always exit 0 silently on any error. The 200ms write timeout is non-negotiable.
6. **Walkable mask = ground footprint only.** This is a top-down view. Visual sprites can be wider/taller than their ground footprint (elevation effects, shadows, wall trim). The walkable mask must only block the ground-level projection — e.g., a 3px-wide wall visual has a 1px walkable mask because the wall's base is 1px. Characters walk right next to walls, not 3px away.

## Known sharp edges (don't be surprised by these)

- **CC hook payloads DO include `tool_use_id`** in `PreToolUse` and `PostToolUse` (verified by sniffing live payloads). The decoder reads it; the reducer's hook-wins dedup actually fires.
- **CC hook `transcript_path` always points to the PARENT'S transcript**, even when a subagent is the actor — so subagent hook events hash to the parent's `AgentId`. The reducer's `active_tasks: HashMap<AgentId, HashSet<String>>` suppresses hook `ActivityStart`/`End` for any agent currently inside a `Task` tool; JSONL has correct subagent attribution via the per-subagent transcript file at `<parent_uuid>/subagents/agent-<id>.jsonl`. The Task signal travels as `ToolDetail::Task` (a typed enum variant on `AgentEvent::ActivityStart.detail`, set by `decoder::make_tool_detail` whenever `tool_name == "Task"`); the reducer pattern-matches on `d.is_task()` rather than scanning a free-form string.
- **JSONL watcher skips historical transcripts on startup.** `initial_seed_root` in `source/jsonl.rs` only emits `SessionStart` for `.jsonl` files with mtime within `DEFAULT_INITIAL_WINDOW` (currently 1 hour; configurable via `JsonlWatcher::with_initial_window`); older files have their cursor seeded at end-of-file. Without this, ~hundreds of stale sessions saturate the desk allocator (default `--max-desks=16`). Long-idle live sessions only re-appear after they next write. The window was bumped from 10 min after users hit "I had a CC session open but it had been idle a while; nothing showed up until I made a new tool call."
- **Subagent display names come from `attributionAgent` in JSONL.** The decoder strips the plugin prefix (`feature-dev:code-explorer` → `code-explorer`) and emits `AgentEvent::Rename` so labels read meaningfully. Parents get their `cwd` basename instead.
- **`AgentSlot.state_started_at` is `std::time::SystemTime`** — process-local in practice (no wall-clock anchoring), but the type is already serializable, so the v2 daemon split won't need a type swap. The pose system computes elapsed time relative to it for animation timing.
- **`ActivityState::Active` ≠ "tool is currently executing".** CC fires PreToolUse → PostToolUse around every individual tool call, so without debouncing the slot flickers Active/Idle on every tool. The reducer treats `ActivityEnd` as "arm pending idle" (`AgentSlot.pending_idle_at`) instead of an immediate flip; the actual transition to `Idle` is realized by `reducer::tick` after `ACTIVE_GRACE_WINDOW = 1500 ms`. Any `ActivityStart` inside the window cancels the pending idle. Net: the slot reads as continuously Active for chained tool work, and visible Idle lags real Idle by up to 1.5 s + the 1 s sweep interval (≈ 2.5 s worst case). Don't add code that depends on `Active → Idle` being instant.
- **`draw_scene` is called through `TuiRenderer`** (the `Renderer` trait impl), which owns the cross-frame state via `DrawCtx` (RgbBuffer, FrameCache, Router, OccupancyOverlay, PoseHistory, theme, mouse state). `draw_scene` returns `Result<Option<Layout>>` — the computed layout is cached on `TuiRenderer.cached_layout` so hit-test functions can use it without recomputing. During floor transitions, `cached_layout` is cleared to `None`.
- **`recolor_frame` substitutes by RGB equality.** Works because each palette key in the default pack maps to a unique RGB. If you add a sprite pack where two keys share a color, swap to a palette-key-indexed approach instead.
- **Terminal cell aspect drives sprite design.** The half-block ▀ technique assumes ~1:2 cell aspect. Sprites larger than ~16×16 px break on terminals with taller cells (Ghostty default, large Fira Code). The bundled 12×14 pack is the safe ceiling. A PNG-loader experiment hit this wall and was deleted in favor of hand-drawn `.sprite` art.

## Things NOT to do

- Don't add `ratatui` / `crossterm` / terminal anything to `ascii-agents-core`.
- Don't write to `~/.claude/settings.json` directly. Always go through `install/io.rs::write_settings_atomic` (advisory lock + atomic rename + symlink resolution).
- Don't add `println!` / `eprintln!` to any production path other than the headless summary and explicit user-facing CLI output. Use `tracing::{info, warn, error}` instead.
- Don't relax the hook shim's "always exit 0" contract. Blocking CC = breaking the user's primary workflow.
- Don't add `--no-verify` / hook-skipping flags to any git operations performed in this repo.
- Don't generate a README / CLAUDE.md / CHANGELOG / docs in PRs unless explicitly asked.
- Don't `git push` without explicit user confirmation, even after committing.
- Don't merge a PR without running the code review process (2+ agents: explorer/reviewer/architect). No exceptions — PR #23 was merged without review and had a critical path-traversal vulnerability.
- Don't blindly accept reviewer findings. Verify the premise before coding a fix — the reviewer may have incomplete context about design intent. Check "Known sharp edges" and existing comments first. If a fix contradicts an earlier design decision, trace the code path manually.

## Where to look

- "How does a CC tool call become a moving sprite?" → trace `runtime::run_async` → `SourceManager::spawn` → `ClaudeCodeSource::run` → `HookSocketListener::run` → `decoder::decode_hook_payload` → `reducer::Reducer::apply` → `TuiRenderer::render` → `draw_scene` (top-down, cubicle grid).
- "How is the office laid out?" → `core::layout::SceneLayout::compute_with_seed` is the coordinator; it calls 4 private helpers: `compute_pod_desks` (pod grid), `compute_pod_decor` (aisle decor), `compute_room_walls` (wall segments + door gaps), `compute_waypoints` (all waypoint assembly). Re-exported as `tui::layout::Layout`. `core::pose::derive` for pure state→pose mapping including the Idle wander state machine (`WANDER_CYCLE_BASE_MS=7000` + per-agent jitter); `tui::pose::derive_with_routing` for the routed variant (A*-routed polylines + snap-back walks); `tui::renderer::draw_scene` for the terminal-flush pass (half-block + widgets + status footer) → `tui::pixel_painter::render_to_rgb_buffer` for the pure-pixel pass. The pixel pass is split: `pixel_painter/background/` (floor/walls/windows/clock/corridor/lighting/shadow via `mod.rs` + `time_of_day.rs` + `lighting.rs`), `pixel_painter/drawable.rs` (y-sort `Drawable` enum + dispatch), `pixel_painter/effects.rs` (chair-behind/screen glow/sleep z/steam/dust/bubble), `pixel_painter/palette.rs` (agent palette + recolor + `tool_glow_tint` for per-tool monitor color), `pixel_painter/anchors.rs` (per-pose sprite anchors + breath + walking_position + character_anchor), `pixel_painter/furniture.rs` (procedural furniture paint helpers).
- "How do overflow agents get rendered?" → agents past a floor's capacity overflow to additional floors (up to `MAX_FLOORS=5`). Each floor has its own `FloorCtx` (router/cache/overlay/history) and `FloorMeta` (floor_idx/altitude/floor_seed/sunlight_boost). `render_to_rgb_buffer` takes `FloorMeta` to drive per-floor cityview height, cat path, lighting, and decoration rotation via `SceneLayout::compute_with_seed(floor_seed)`.
- "Why is the subagent's sprite the right one and not the parent?" → `reducer::Reducer::apply` does subagent-leak suppression via `active_tasks` before applying. `claude_code::decode_cc_line` emits `AgentEvent::Rename` from `attributionAgent`.
- "How does multi-source decoding work?" → `JsonlWatcher` takes `LineDecoder` and `LabelDeriver` fn pointers (Strategy pattern via fn pointers, not traits). CC wires `claude_code::decode_cc_line` + `cc_derive_label`; Antigravity wires `antigravity::decode_ag_line` + `derive_ag_label`. `decoder.rs` holds shared utilities (`describe_tool_target`, `make_tool_detail`, `decode_hook_payload`). Each source owns its own JSONL format knowledge. `AgentId::from_parts(source, path)` namespaces IDs per source. Labels show source prefix: `cc·project` / `ag·project`.
- "Why don't old idle sessions show on startup?" → `source::jsonl::initial_seed_walk`. Checks `check_session_ended` (tail-scans last 8KB for `session_end`/`SessionEnd` markers) and skips files not modified in 5+ min. mtime > `DEFAULT_INITIAL_WINDOW` (1 hour) → cursor seeded at EOF, no `SessionStart`.
- "How does the default character pack get into the binary?" → `tui::embedded_pack` does the `include_str!` at compile time; `sprite::format::load_pack_from_strings` parses it.
- "How do custom sprite packs work?" → `ascii-agents init-pack ./dir` extracts the skeleton template from `sprites/skeleton/` (embedded via `include_str!`). `ascii-agents validate-pack ./dir` loads the pack and checks against `REQUIRED_CHARACTER_ANIMATIONS` / `OPTIONAL_*` registries in `sprite::format`. `--pack-dir` CLI flag or `pack-dir` config key loads a custom pack at runtime. Custom packs only need character sprites — furniture/environment animations are merged from the embedded default via `Pack::merge_from()` (only `OPTIONAL_FURNITURE_ANIMATIONS`, never character poses). The robot pack at `sprites/robot/` is a TV-head character set (10×12 sprites).
- "How do hooks get installed?" → `install::merge::merge_install` for the JSON merge logic, `install::io::write_settings_atomic` for the safe filesystem write.
- "How does the neon wall display work?" → `pixel_painter/background/lighting.rs::paint_neon_panel` paints the dark panel with pulsing cyan border in the pixel buffer; `widgets/hud.rs::paint_wall_display` overlays ratatui text (branding, state dots, scrolling ticker); `widgets/mod.rs::TickerQueue` manages the persistent scrolling message buffer.
- "How do pets work?" → `tui/pet.rs::PetKind` enum (Cat, Dog) with per-kind static data (sprite names, hitboxes, behavior). One pet per floor selected via `select_pet_for_floor(floor_seed, enabled_pets)`. Config: `enabled-pets = ["cat", "dog"]` (absent = all, empty = none). `pixel_painter/drawable.rs::pet_position` — 40s cycle, picks a destination from all spots (desks, pantry, sofas, couch, corridor), walks there (35%), sits/sleeps (65%). Cat sleeps with z's near idle agents; dog sits near active agents, both sleep when all agents idle. Sprites per kind: `*_walk` (8×6), `*_sit` (6×6), `*_sleep` (6×4). Click to pet → hearts animation via `PetState` on `TuiRenderer`. `hit_test_pet` / `paint_pet_tooltip` parameterized on `PetKind`.
- "How does desk personalization work?" → `drawable.rs::paint_desk_personalization` — procedural pixel items appear on desks based on `session_age_secs`: coffee cup (event-driven, after pantry visit), plant (30min), photo frame (1hr).
- "How does the coffee run work?" → `Pose::Walking.carrying_coffee` set in `idle_pose` walk-back from Pantry → `walking_coffee` sprite selected in pixel_painter → `coffee_holders: HashSet<AgentId>` on `TuiRenderer` tracks which agents have visited the pantry (inserted when the pixel pass sees `carrying_coffee: true`) → cup persists on desk until agent exits → exit walk overrides `carrying_coffee` from `coffee_holders` in the pixel painter → `coffee_fetched_at` timestamps drive 120s steam window.
- "How does the crash log work?" → `main.rs::install_crash_hook` sets a panic hook that restores the terminal, writes a timestamped backtrace to `~/.cache/ascii-agents/crash.log`.
- "How does the theme system work?" → `tui/theme/mod.rs` defines the `Theme` struct (~100 color roles in 7 groups). Each theme is a `pub static Theme` in its own file (e.g. `theme/cyberpunk.rs`). `ALL_THEMES` is the registry slice. `--theme` CLI flag resolves via `theme_by_name()`. The `&'static Theme` threads through `TuiRenderer` → `draw_scene` → `render_to_rgb_buffer` → all paint functions. Press `[t]` in the TUI for a live preview picker (j/k or ↑↓ to navigate). `set_theme()` flushes the `FrameCache` so character recolors update immediately. 6 themes: normal, cyberpunk, dracula, tokyo-night, catppuccin, gruvbox.
- "How does weather work?" → `pixel_painter/background/time_of_day.rs::weather_state` picks from 7 variants (Clear/Rain/Storm/Snow/Fog/Overcast/Windy) via splitmix64 hash of `wallclock / 600` (changes every 10 min). Effects paint on window glass after the skyline. `sunset_strength()` adds a time-based golden-hour tint at ~6am/6pm, scaled down by existing twilight intensity to avoid double-orange. City light twinkle is 6–14s cycles at 70% lit.
- "How does the thinking pose work?" → `core::pose::derive` returns `Pose::SeatedThinking` when an Idle agent's `last_event_at` is within `THINKING_WINDOW_SECS = 20s` AND `last_event_at > created_at` (excludes freshly spawned agents). Renders as `seated` sprite + screen glow + animated `···` dots (3-phase, 800ms cycle via `effects::paint_thinking_dots`). Screen glow only paints when the agent's derived pose is seated (precomputed pose map avoids double A*).
- "How do tooltip stats work?" → `AgentSlot.tool_call_count` increments on `ActivityStart` (excludes Task delegation). `AgentSlot.active_ms` accumulates on the next `ActivityStart` (measuring the previous span) and on `expire_pending_idles` (measuring to `pending_idle_at`, not `now`, to avoid grace-window inflation). Tooltip shows `⏱ duration · N calls · X% active`. Fresh agents (<5s) show `--% active`.
- "How does the coffee machine Easter egg work?" → `hit_test.rs::hit_test_coffee_machine` checks if a click falls on the coffee machine section of the pantry counter sprite (x offset 11–18 for large, 8–13 for small). Hover shows `widgets/tooltip.rs::paint_coffee_tooltip` ("☕ Buy Ivan a coffee"), click opens BMC via `open::that`. Both take `&Layout` (cached on `TuiRenderer`).
- "How do furniture hover tooltips work?" → `hit_test.rs::hit_test_furniture` tests mouse coords against all layout positions (desks, waypoints, plants, wall decor, pod decor, meeting sofas/table, coat rack, doormat, water cooler, trash bin, elevator). Returns `Option<&'static str>` label. `widgets/tooltip.rs::paint_furniture_tooltip` renders it. Checked after agent tooltip and coffee machine in the draw closure priority chain.
- "How do the corridor appliances work?" → Vending machine (4×6) and printer (5×4) are painted as y-sorted `Drawable` variants in `pixel_painter/drawable.rs`. Both are `WaypointKind` variants so idle agents can wander to them. Placement is conditional on corridor height (vending ≥10px, printer ≥9px). Positions stored as centre-point waypoints (same convention as Pantry/Couch).
- "How does config persistence work?" → `config.rs` defines `AppConfig` (theme + optional max-desks cap), `config_path()` (XDG-aware: `$XDG_CONFIG_HOME/ascii-agents/config.toml` or `~/.config/ascii-agents/config.toml`), `load()` (never crashes — logs warning on malformed TOML), `save()` (advisory-locked atomic tmp+rename), `resolve_theme()` (CLI > config > default). Theme saved on `[t]` picker Enter confirm in `tui/mod.rs`. `max-desks` is an optional cap — when set, auto-compute clamps each floor's capacity to `min(layout_capacity, cap)`. When absent, fully auto-computed from terminal size.
- "How do multi-floor offices work?" → `tui/floor.rs` defines `FloorCtx` (per-floor render state), `FloorTransition` (slide animation), `build_floor_scene()` (agent projection). `tui_renderer.rs` owns `Vec<FloorCtx>` + `Vec<RgbBuffer>` and switches between them. Floor membership is stored on `AgentSlot.floor_idx` (set once by the reducer at desk assignment, immutable thereafter). Each floor's capacity is auto-computed per-frame: `tui/mod.rs` calls `SceneLayout::compute_with_seed` for each floor's seed and writes the result via per-floor `AtomicUsize::fetch_max` (monotone growth). The reducer syncs all 5 capacities into `scene.floor_capacities: [usize; MAX_FLOORS]` each tick. `next_free_desk` in `state/mod.rs` scans `0..total_capacity()`. On terminal shrink, agents beyond the layout's capacity become invisible but stay alive; they reappear when the terminal grows back. PageUp/PageDown/↑↓/jk in `tui/mod.rs`. Slide transition composites two buffers via `flush_buffer_to_term_at_offset`.

## When refactoring

If you change anything in the channel type, `Source` trait, `AgentEvent` enum, or reducer signature, update **all four** test files that exercise them: `tests/reducer.rs`, `tests/e2e.rs`, `tests/hook_socket.rs`, `tests/jsonl_watcher.rs`, plus `runtime.rs` on the binary side. The `AgentEvent::agent_id()` method in `source/mod.rs` needs a new arm too if you add a variant.
