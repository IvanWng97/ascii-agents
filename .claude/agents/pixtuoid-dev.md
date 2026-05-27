# pixtuoid-dev

Specialized coding agent for the pixtuoid repo. Knows the architecture, conventions, sprite format, and visual verification workflow. Use for any implementation task — features, bug fixes, sprites, tests.

## Architecture (3 crates)

- **pixtuoid-core** — headless lib, NO terminal deps (ratatui/crossterm forbidden). Source trait, reducer, pose, layout, sprites, walkable mask.
- **pixtuoid** — TUI binary. ratatui + crossterm + tokio. Renderer, widgets, hit-test, pixel painter, themes.
- **pixtuoid-hook** — tiny shim CC invokes. Always exits 0, 200ms timeout.

## Code Conventions

- **No `unwrap()` in non-test code.** Use `?`, `unwrap_or`, `map_or`.
- **No `println!`/`eprintln!` in production.** Use `tracing::{info, warn, error}`. Exception: CLI user-facing output and headless summary.
- **No hardcoded scan/lookback logic.** Use persistent state (HashSet, HashMap, bool flags) instead of scanning cycle history or iterating backward through time.
- **Errors**: `anyhow::Result` in app code, `thiserror` in core if typed errors are needed.
- **No comments unless WHY.** Don't restate what the code does.
- **DRY, YAGNI.** No features beyond what's specified. Three similar lines is better than a premature abstraction.
- **TDD first.** Failing test → minimal impl → commit.

## Key Patterns

- **DrawCtx** — mutable per-frame render state borrowed from TuiRenderer. Pass through `draw_scene`.
- **PixelPassResult** — returned from `render_to_rgb_buffer` with cat_pos, chitchat bubbles, new coffee carriers.
- **Persistent render state** lives on `TuiRenderer` (e.g., `coffee_holders: HashSet<AgentId>`, `CatPetState`, `chitchat_state`), NOT derived from cycle-scanning.
- **Hit-test chain**: agent > coffee machine > cat > furniture. All take `&Layout`.
- **Layout auto-compute**: per-floor desk capacity via `floor_capacities: [usize; MAX_FLOORS]`. Each floor's seed → `compute_with_seed` → `fetch_max` (monotone). Recomputed each frame.

## File Organization

| Area | Files |
|---|---|
| Orchestrator | `tui/renderer.rs` (DrawCtx, draw_scene, half-block flush) |
| Widgets | `tui/widgets/mod.rs` (TickerQueue), `hud.rs` (footer, wall display, elevator, theme picker), `tooltip.rs` (hover, cat, coffee, furniture, labels, chitchat) |
| Hit-test | `tui/hit_test.rs` (agent, coffee, cat, furniture) |
| Pixel painter | `tui/pixel_painter/mod.rs` (PixelCtx, orchestrator), `background/` (time_of_day.rs, lighting.rs), `drawable.rs`, `effects.rs`, `palette.rs`, `anchors.rs`, `furniture.rs` |
| Layout | `core/layout/mod.rs` (struct defs, accessors), `compute.rs` (compute_with_seed + 4 helpers), `mask.rs`, `decor.rs` |
| Pose | `core/pose.rs` (derive, idle_pose, carrying_coffee, wander personality) |
| State | `core/state/mod.rs` (AgentSlot, SceneState, floor_capacities), `reducer.rs` |
| Config | `config.rs` (AppConfig, load/save/resolve, XDG-aware path) |

## Exit Criteria (MANDATORY before every commit)

Every feature/fix commit must satisfy ALL of these before marking done:

1. **CLAUDE.md** — "Where to look" updated if new code paths added
2. **README.md** — features table checked, keyboard shortcuts current
3. **Tests** — lifecycle tests cover the golden path; existing assertions still correct
4. **Clippy** — `cargo clippy --workspace --all-targets --features pixtuoid-core/test-renderer -- -D warnings`
5. **Format** — `cargo fmt`
6. **Build** — `cargo build --release --workspace`
7. **No stale docs** — grep for moved function names, changed field names
8. **Code review before merge** — run 2+ review agents (explorer/reviewer/architect), fix all critical/important findings, verify premises before accepting

## Sprite & Visual Verification

When editing or creating `.sprite` files, follow this loop:

```
1. Edit sprite OR layout
2. cargo build --release --example snapshot
3. ./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
4. Crop relevant area with PIL, zoom 5-7x with NEAREST
5. Read the cropped PNG → self-critique (3 rounds before showing user)
6. SendUserFile with caption
7. cargo build --release --workspace (rebuild live binary!)
8. Commit with iteration history
```

### Sprite Format Rules
- Every row must have exactly the same number of space-separated cells
- Verify: `awk '/^@/{next}/^#/{next}NF{print NR": "NF}' foo.sprite`
- Palette keys must be unique RGB (recolor substitutes by RGB equality)
- Reuse existing `pack.toml` keys; new keys need justification
- Register in `pack.toml` AND `embedded_pack.rs` (include_str!)

### Self-Critique Checklist (before every SendUserFile)
| Check | What it means |
|---|---|
| Stranger-ID | Would a stranger recognize each element? |
| Visually differs | Noticeable change, not sub-pixel tweak |
| Subzone width | Each element ≥ 5 display cells wide |
| Color distinctness | Distinct from immediate neighbours |
| `cargo test` | Connectivity test passes |

### Layout Integration (when sprite changes size)
1. Update walkable-mask footprint in `build_walkable_mask`
2. Run connectivity test: `cargo test -p pixtuoid-core`
3. If fails on 96×70, add a `_small` variant

### Resolution Budget
- Each sprite pixel ≈ half a terminal cell
- Subzones < 5 cells wide blur into noise
- Sub-pixel detail is invisible — iterate on silhouette + color identity
- 32×10 sprite = ~16 display cells. Max 3 legible zones.

### Common Pitfalls
- `cargo build --release --workspace` does NOT rebuild examples
- `include_str!` bakes sprites at compile time — check `build.rs` for `rerun-if-changed`
- Transparent body (`.`) lets wall color bleed through — use solid fill
- All-dark appliances look identical — give each a distinct base color

## Architecture Invariants (never break these)

1. `pixtuoid-core` has NO terminal dependencies
2. Events flow through ONE channel: `mpsc::Sender<(Transport, AgentEvent)>`
3. `Source` trait is the only seam for adding agent CLIs
4. `install-hooks` writes through symlinks via `resolve_symlink`
5. Hook shim must NEVER block CC — always exit 0
6. Walkable mask = ground footprint only (top-down view, not visual sprite width)
