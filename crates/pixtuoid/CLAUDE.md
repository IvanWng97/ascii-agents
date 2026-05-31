# pixtuoid (binary) — agent guide

The **TUI binary**: `ratatui` + `crossterm` + `tokio` + `clap`. Wires sources →
reducer → renderer, owns the CLI subcommands, hook installation, config
persistence, and multi-floor orchestration. The pure render pipeline lives under
`src/tui/` — see its own nested guide:
[`src/tui/CLAUDE.md`](src/tui/CLAUDE.md). Cross-cutting rules:
workspace [`CLAUDE.md`](../../CLAUDE.md); headless-lib detail:
[`../pixtuoid-core/CLAUDE.md`](../pixtuoid-core/CLAUDE.md).

## Layout

```
src/
├── main.rs             entry point; install_crash_hook (panic hook → ~/.cache/pixtuoid/crash.log)
├── cli.rs              clap subcommands (run / install-hooks / uninstall-hooks / validate-pack / init-pack)
├── config.rs           AppConfig persistence (~/.config/pixtuoid/config.toml), XDG-aware
├── runtime.rs          tokio task wiring (source ── (Transport, AgentEvent) ──► reducer ──► renderer),
│                       compute_boot_capacities (queries terminal size at startup), headless summary
├── init_pack.rs        extracts the embedded skeleton pack to a target dir for `init-pack`
├── install/            multi-target (Claude + Codex) hook install via the `Target` registry:
│                       mod.rs (run_install/run_uninstall, plan_targets, interactive_pick),
│                       target.rs (Target trait + TARGETS = [CLAUDE, CODEX]),
│                       claude.rs / codex.rs (per-target hook_command + config path),
│                       io.rs (resolve_symlink, write_config_atomic — advisory lock + atomic rename)
└── tui/                ratatui App + TuiRenderer (Renderer trait impl) — see src/tui/CLAUDE.md

sprites/                character/environment packs (NOT under pixtuoid-hook):
├── default/            coworking-lounge pack (embedded into the binary via include_str!)
├── robot/              proof-of-concept TV-head robot pack (loadable via --pack-dir)
└── skeleton/           template pack for custom sprite creation (extracted via init-pack)
```

## Known sharp edges (don't be surprised by these)

- **Terminal cell aspect drives sprite design.** The half-block ▀ technique assumes ~1:2 cell aspect. Sprites larger than ~16×16 px break on terminals with taller cells (Ghostty default, large Fira Code). The bundled **character** sprites max at **8×12 px** (e.g. `standing`/`walking_*`), safely under the ~16×16 threshold; static environment art (door 16×14, pantry 32×10) is wider but isn't an animated half-block agent. A PNG-loader experiment hit this wall and was deleted in favor of hand-drawn `.sprite` art.
- **`--max-desks` has no hard default.** It's `Option<usize>` (hidden flag / `max-desks` config key) — when absent, per-floor capacity is auto-computed from terminal size at boot. `FALLBACK_DESKS = 16` (`runtime.rs`) is used only in headless mode or when the terminal-size query errors. The auto path clamps each floor to its real layout capacity; if you add an explicit-cap boot path, clamp it the same way (don't seed the floor-capacity atomics above the layout's real capacity — `fetch_max` only grows, so an over-seed leaves agents assigned to non-existent desks until the terminal grows).

## Where to look

- "How do hooks get installed?" → `install::claude::merge_install` for the JSON merge logic, `install::io::write_config_atomic` for the safe filesystem write. Multi-target install via the `install::target::Target` registry (`TARGETS = [CLAUDE, CODEX]`); `install::plan_targets` decides which CLIs to act on (auto-detect + confirm + non-TTY policy). Claude's `hook_command` ignores the resolved binary path (emits bare `pixtuoid-hook`, relying on PATH); Codex embeds the absolute path. Resolution of the hook binary must therefore be soft (warn) for Claude and only fatal for targets that actually need the path.
- "How does the default character pack get into the binary?" → `tui::embedded_pack` does the `include_str!` at compile time (from `crates/pixtuoid/sprites/default/`); `sprite::format::load_pack_from_strings` parses it.
- "How do custom sprite packs work?" → `pixtuoid init-pack ./dir` extracts the skeleton template from `sprites/skeleton/` (embedded via `include_str!`, see `init_pack.rs`). `pixtuoid validate-pack ./dir` loads the pack and checks against `REQUIRED_CHARACTER_ANIMATIONS` / `OPTIONAL_*` registries in `sprite::format`. `--pack-dir` CLI flag or `pack-dir` config key loads a custom pack at runtime. Custom packs only need character sprites — furniture/environment animations are merged from the embedded default via `Pack::merge_from()` (only `OPTIONAL_FURNITURE_ANIMATIONS`, never character poses). The robot pack at `sprites/robot/` is a TV-head character set (10×12 sprites).
- "How does the crash log work?" → `main.rs::install_crash_hook` sets a panic hook that restores the terminal, writes a timestamped backtrace to `~/.cache/pixtuoid/crash.log`.
- "How does config persistence work?" → `config.rs` defines `AppConfig` (theme + optional max-desks cap + pack-dir + enabled-pets), `config_path()` (XDG-aware: `$XDG_CONFIG_HOME/pixtuoid/config.toml` or `~/.config/pixtuoid/config.toml`), `load()` (never crashes — logs warning on malformed TOML), `save()` (advisory-locked atomic tmp+rename), `resolve_theme()` (CLI > config > default). Theme saved on `[t]` picker Enter confirm in `tui/mod.rs`. `max-desks` is an optional cap — when set, auto-compute clamps each floor's capacity to `min(layout_capacity, cap)`. When absent, fully auto-computed from terminal size. `pack-dir` supports `~` expansion via `resolve_pack_dir`.
- "How do multi-floor offices work?" → `tui/floor.rs` defines `FloorCtx` (per-floor render state: router/cache/overlay/history/**light** [LightingState]/motion) + `FloorTransition` (slide animation) + `build_floor_scene()` (agent projection). `tui_renderer/mod.rs` owns `Vec<FloorCtx>` + `Vec<RgbBuffer>` and switches between them. Floor membership is stored on `AgentSlot.floor_idx` (set once by the reducer at desk assignment, immutable thereafter). Each floor's capacity is **boot-seeded from the actual terminal size** via `compute_boot_capacities()` in `runtime.rs` (queries `crossterm::terminal::size()` at startup, falls back to `FALLBACK_DESKS=16` in headless mode or on error). Per-frame, `tui/mod.rs` calls `SceneLayout::compute_with_seed` for each floor's seed and writes the result via per-floor `AtomicUsize::fetch_max` (monotone growth — capacity never shrinks, preventing cumulative-offset shifts that would remap floor 1+ agents to wrong desks). The reducer syncs all 5 capacities into `scene.floor_capacities: [usize; MAX_FLOORS]` each tick. `next_free_desk` in `state/mod.rs` scans `0..total_capacity()`. On terminal shrink, agents beyond the layout's capacity become invisible but stay alive; they reappear when the terminal grows back. PageUp/PageDown/↑↓/jk in `tui/mod.rs`. Agents past a floor's capacity overflow to additional floors (up to `MAX_FLOORS=5`).
