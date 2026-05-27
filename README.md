<p align="center">
  <img src="docs/images/sprite-banner.png" alt="pixtuoid sprites" width="500" />
</p>

<h1 align="center">pixtuoid</h1>

<p align="center">
  <em>Your AI coding agents, visualized as pixel-art coworkers in a terminal office.</em>
</p>

<p align="center">
  <a href="https://github.com/IvanWng97/pixtuoid/stargazers"><img src="https://img.shields.io/github/stars/IvanWng97/pixtuoid?style=flat-square" alt="Stars" /></a>
  <a href="https://github.com/IvanWng97/pixtuoid/releases"><img src="https://img.shields.io/github/v/release/IvanWng97/pixtuoid?label=version&style=flat-square" alt="Version" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square" alt="License" /></a>
  <a href="https://github.com/IvanWng97/pixtuoid/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/IvanWng97/pixtuoid/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="https://codecov.io/gh/IvanWng97/pixtuoid"><img src="https://img.shields.io/codecov/c/github/IvanWng97/pixtuoid?style=flat-square" alt="Coverage" /></a>
  <a href="https://claude.ai/code"><img src="https://img.shields.io/badge/Built%20with-Claude%20Code-blueviolet?style=flat-square&logo=anthropic" alt="Built with Claude Code" /></a>
  <a href="https://buymeacoffee.com/IvanWng97"><img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-ffdd00?style=flat-square&logo=buy-me-a-coffee&logoColor=black" alt="Buy Me a Coffee" /></a>
</p>

<p align="center">
  <img src="docs/images/demo.gif" alt="pixtuoid animated demo" width="800" />
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> · <a href="#features">Features</a> · <a href="#supported-tools">Supported Tools</a> · <a href="#themes--configuration">Themes & Configuration</a> · <a href="#how-it-works">How It Works</a>
</p>

---

## Why?

Running multiple AI agents in the terminal is like managing a sweatshop you can't see. They type, they wait, they finish — and you have no idea who's doing what unless you scroll through logs like a bureaucrat.

**pixtuoid** puts them all in a tiny pixel-art office you can watch from above. A little bit *Black Mirror*, a little bit *The Sims* — and somehow the most intuitive multi-agent dashboard you'll ever use.

## Quick Start

```bash
brew install IvanWng97/pixtuoid/pixtuoid
pixtuoid install-hooks
pixtuoid
```

In another terminal, start a Claude Code session. A character walks in from the elevator within a second.

**Keyboard shortcuts:** `q` quit · `p` pause · `t` themes · `↑↓/jk/PgUp/PgDn` floors · click to pin tooltip

<details>
<summary><strong>More install methods</strong></summary>

### Pre-built binaries

Download from [GitHub Releases](https://github.com/IvanWng97/pixtuoid/releases/latest):

| Platform | Tarball |
|---|---|
| macOS (Apple Silicon) | `pixtuoid-v*-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `pixtuoid-v*-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64, static) | `pixtuoid-v*-x86_64-unknown-linux-musl.tar.gz` |
| Linux (ARM64) | `pixtuoid-v*-aarch64-unknown-linux-gnu.tar.gz` |

### Cargo

```bash
cargo install pixtuoid
```

### From source

```bash
git clone https://github.com/IvanWng97/pixtuoid && cd pixtuoid
cargo build --release
```

</details>

## Features

| | Feature | Description |
|---|---|---|
| 🏢 | **Multi-agent office** | Each CC session gets a desk; overflow agents auto-fill new floors |
| 🛗 | **Multi-floor office** | PageUp/PageDown/↑↓/jk to navigate floors with slide transition |
| 🎭 | **Animated characters** | Typing, thinking (`···`), waiting (`?`), sleeping (z's), walking with A\*-routed pathfinding |
| 💡 | **Per-tool monitor glow** | Edit = blue, Bash = orange, Read = cyan — scannable at a glance |
| 🎨 | **Per-agent identity** | Deterministic shirt/hair/skin palette from session hash, 16 curated outfits |
| 🌧️ | **Weather effects** | Rain, storm, snow, fog, overcast, windy — cycles every 10 min + sunset golden hour |
| 📊 | **Tooltip stats** | Hover any agent to see session duration, tool call count, and active time % |
| 🏷️ | **Furniture tooltips** | Hover any item — desks, sofas, plants, vending machine, printer — to see its name |
| 🐱 | **Office cat** | Roams desks, pantry, sofas; sleeps near idle agents. Click to pet — pixel-art hearts float up |
| ☕ | **Coffee run** | Idle agents visit the pantry, carry a cup back to their desk. Cup stays while you work; taken on exit |
| 💬 | **Pantry chitchat** | 2+ idle agents at the same waypoint trigger speech bubbles with dev-humor snippets |
| 🪴 | **Desk personalization** | Plant (30min), photo frame (1hr) appear over time |
| 🛡️ | **Hook-safe** | The shim always exits 0 — a stuck visualizer can never block Claude Code |

## Supported Tools

| Tool | Status | Notes |
|---|---|---|
| [**Claude Code**](https://code.claude.com) | ✅ Supported | Hook shim + JSONL watcher |
| [**Antigravity CLI**](https://github.com/antiGravity-AI/antigravity-cli) | ✅ Supported | JSONL watcher |
| [**Codex CLI**](https://github.com/openai/codex) | 🔜 Planned | Same hook pattern as CC |
| [**Copilot CLI**](https://github.com/github/copilot-cli) | 🔜 Planned | Identical event names |
| [**OpenCode**](https://github.com/opencode-ai/opencode) | 🔜 Planned | Any LLM (DeepSeek / GPT / Claude / Gemini) |
| [**Cursor CLI**](https://cursor.com/cli) | 🔜 Planned | NDJSON stream |

> Adding a new tool? Implement the [`Source` trait](#contributing) — one file, one channel, done.

## Themes & Configuration

Press `t` to switch themes with live preview. Your choice persists across sessions. 6 built-in:

<p align="center">
  <img src="docs/images/themes-composite.png" alt="6 themes: Normal, Cyberpunk, Dracula, Tokyo Night, Catppuccin, Gruvbox" width="800" />
</p>

Settings are stored in `~/.config/pixtuoid/config.toml` (respects `$XDG_CONFIG_HOME`):

```toml
theme = "cyberpunk"
max-desks = 8
pack-dir = "~/.config/pixtuoid/packs/robot"
```

| Key | Default | Description |
|-----|---------|-------------|
| `theme` | `"normal"` | Color theme — `normal`, `cyberpunk`, `dracula`, `tokyo-night`, `catppuccin`, `gruvbox` |
| `max-desks` | auto | Cap desks per floor. If unset, auto-computed from terminal size. Excess agents overflow to additional floors. |
| `pack-dir` | — | Custom sprite pack directory. Supports `~` expansion. |

CLI flags override config: `pixtuoid run --theme dracula`

### Custom Sprite Packs

Create your own character sprites:

```bash
pixtuoid init-pack ./my-pack     # extract skeleton template
# edit the .sprite files in ./my-pack
pixtuoid validate-pack ./my-pack # check for missing animations
pixtuoid run --pack-dir ./my-pack
```

A **robot** pack ships as an example at `sprites/robot/`. See the [sprite format docs](CLAUDE.md) for palette keys and animation requirements.

## How It Works

<details>
<summary><strong>Architecture</strong></summary>

```
CC tool call ──► CC fires hook ──► pixtuoid-hook (shim)
                                         │ JSON over Unix socket
                                         ▼
                                  /tmp/pixtuoid-{uid}.sock
                                         │
                       HookSocketListener ─────► ┐
                                                 │ (Transport, AgentEvent)
                       JsonlWatcher       ─────► ┤ shared mpsc channel
                                                 ▼
                       Reducer ──► SceneState (watch channel)
                                         │
                       TuiRenderer ──► draw_scene @ ~30fps
                       (pose → pixel_painter → RgbBuffer → half-block → ratatui)
```

Three Rust crates:

| Crate | Role |
|---|---|
| **pixtuoid-core** | Headless library — no terminal deps. Source trait, reducer, pose, layout, sprites. |
| **pixtuoid** | TUI binary — ratatui + crossterm + tokio. Half-block rendering + theme system. |
| **pixtuoid-hook** | Tiny shim CC invokes from hooks. 200ms timeout, always exits 0. |

</details>

## Migrating from ascii-agents (v0.3.0 → v0.4.0)

**v0.4.0 renames the project from `ascii-agents` to `pixtuoid`** — pix(el) + tu(i) + (agent-)oid.

### What changed

| Before (v0.3.x) | After (v0.4.0) |
|---|---|
| `ascii-agents` binary | `pixtuoid` |
| `ascii-agents-hook` shim | `pixtuoid-hook` |
| `~/.config/ascii-agents/` | `~/.config/pixtuoid/` |
| `~/.cache/ascii-agents/` | `~/.cache/pixtuoid/` |
| `/tmp/ascii-agents-{uid}.sock` | `/tmp/pixtuoid-{uid}.sock` |
| `_ascii_agents` hook key in `settings.json` | `_pixtuoid` |

### Upgrade steps

1. **Install the new version:**
   ```bash
   brew untap IvanWng97/ascii-agents 2>/dev/null
   brew install IvanWng97/pixtuoid/pixtuoid
   # or: cargo install pixtuoid
   ```

2. **Re-register hooks** (replaces old `ascii-agents-hook` entries automatically):
   ```bash
   pixtuoid install-hooks
   ```

3. **Migrate config** (optional — only if you customized `config.toml`):
   ```bash
   mkdir -p ~/.config/pixtuoid
   mv ~/.config/ascii-agents/config.toml ~/.config/pixtuoid/config.toml
   ```

> **GitHub links:** The old `IvanWng97/ascii-agents` URL automatically redirects to `IvanWng97/pixtuoid`. Existing bookmarks and stars carry over.

## Contributing

See [`CLAUDE.md`](CLAUDE.md) for architecture and conventions. PRs welcome — especially new themes and `Source` adapters for other agent CLIs (Codex, Cursor, Gemini).

<details>
<summary><strong>Adding a new agent CLI</strong></summary>

Implement the `Source` trait and plug in via `SourceManager::with_source()`:

```rust
#[async_trait]
pub trait Source: Send + 'static {
    fn name(&self) -> &str;
    async fn run(self: Box<Self>, tx: TaggedSender) -> anyhow::Result<()>;
}
```

</details>

## Acknowledgments

Inspired by [`pixel-agents`](https://github.com/pablodelucca/pixel-agents) (VS Code), [`clawd-on-desk`](https://github.com/rullerzhou-afk/clawd-on-desk) (desktop pet), and Claude Code's [Buddy](https://dev.to/picklepixel/how-i-reverse-engineered-claude-codes-hidden-pet-system-8l7).

## Support

If you enjoy pixtuoid, consider [buying me a coffee](https://buymeacoffee.com/IvanWng97) :)

## License

[MIT](LICENSE)
