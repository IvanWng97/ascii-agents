# Pet System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the hardcoded singleton cat into a generic multi-pet system that supports cat + dog (and future pets), with one pet per floor selected from a user-configurable enabled list.

**Architecture:** `PetKind` enum in `tui/pet.rs` encodes all per-kind static data (sprite names, hitboxes, behavior flags). One pet per floor is selected deterministically via `enabled_pets[floor_seed % len]`. Config adds `enabled-pets = ["cat", "dog"]` (absent = all, empty = none). State changes from singleton `cat_pet`/`last_cat_pos` to `active_pet: Option<PetPetState>` + `last_pet_pos: Option<(Point, &'static str, PetKind)>`.

**Tech Stack:** Rust, ratatui, serde/toml, `.sprite` pixel art format

---

## File Structure

### New files
- `crates/pixtuoid/src/tui/pet.rs` — `PetKind` enum, `select_pet_for_floor()`, all per-kind static data
- `crates/pixtuoid/sprites/default/dog_walk_0.sprite` — 8x6 dog walking frame 0
- `crates/pixtuoid/sprites/default/dog_walk_1.sprite` — 8x6 dog walking frame 1
- `crates/pixtuoid/sprites/default/dog_sit.sprite` — 6x6 dog sitting
- `crates/pixtuoid/sprites/default/dog_sleep.sprite` — 6x4 dog sleeping

### Modified files
- `crates/pixtuoid/src/tui/mod.rs` — add `pub mod pet;`, update click handler
- `crates/pixtuoid/src/config.rs` — add `enabled_pets` field + `resolve_pets()`
- `crates/pixtuoid/src/tui/renderer.rs` — rename `CatPetState` → `PetPetState`, update `DrawCtx`
- `crates/pixtuoid/src/tui/tui_renderer.rs` — rename fields, add `enabled_pets`, update accessors
- `crates/pixtuoid/src/tui/pixel_painter/drawable.rs` — `DrawableKind::Cat` → `Pet`, `cat_position` → `pet_position`
- `crates/pixtuoid/src/tui/pixel_painter/mod.rs` — update `PixelPassResult`, `PixelCtx`, cat block
- `crates/pixtuoid/src/tui/hit_test.rs` — `hit_test_cat` → `hit_test_pet`
- `crates/pixtuoid/src/tui/widgets/tooltip.rs` — `paint_cat_tooltip` → `paint_pet_tooltip`
- `crates/pixtuoid-core/src/sprite/format.rs` — add dog to `OPTIONAL_FURNITURE_ANIMATIONS`
- `crates/pixtuoid/sprites/default/pack.toml` — add dog palette + animations
- `crates/pixtuoid/src/tui/embedded_pack.rs` — add `include_str!` for dog sprites
- `crates/pixtuoid/src/runtime.rs` — thread `enabled_pets` to `run_tui`
- `crates/pixtuoid/src/main.rs` — resolve pets from config, pass to `runtime::run`

---

### Task 1: PetKind enum and floor selection

**Files:**
- Create: `crates/pixtuoid/src/tui/pet.rs`
- Modify: `crates/pixtuoid/src/tui/mod.rs:1-13` (add module declaration)

- [ ] **Step 1: Write failing tests for PetKind**

Add to `crates/pixtuoid/src/tui/pet.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PetKind {
    Cat,
    Dog,
}

impl PetKind {
    pub const ALL: &'static [PetKind] = &[PetKind::Cat, PetKind::Dog];

    pub fn config_name(self) -> &'static str {
        match self {
            PetKind::Cat => "cat",
            PetKind::Dog => "dog",
        }
    }

    pub fn from_config_name(s: &str) -> Option<Self> {
        match s {
            "cat" => Some(PetKind::Cat),
            "dog" => Some(PetKind::Dog),
            _ => None,
        }
    }

    pub fn walk_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_walk",
            PetKind::Dog => "dog_walk",
        }
    }

    pub fn sit_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_sit",
            PetKind::Dog => "dog_sit",
        }
    }

    pub fn sleep_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_sleep",
            PetKind::Dog => "dog_sleep",
        }
    }

    /// Cat sleeps near idle agents; dog sits near active agents.
    pub fn sleeps_near_idle(self) -> bool {
        match self {
            PetKind::Cat => true,
            PetKind::Dog => false,
        }
    }

    pub fn hitbox(self, anim_name: &str) -> (u16, u16) {
        match (self, anim_name) {
            (PetKind::Cat, "cat_walk") => (8, 6),
            (PetKind::Cat, "cat_sit") => (6, 6),
            (PetKind::Cat, "cat_sleep") => (6, 4),
            (PetKind::Dog, "dog_walk") => (8, 6),
            (PetKind::Dog, "dog_sit") => (6, 6),
            (PetKind::Dog, "dog_sleep") => (6, 4),
            _ => (6, 6),
        }
    }
}

pub fn select_pet_for_floor(floor_seed: u64, enabled_pets: &[PetKind]) -> Option<PetKind> {
    if enabled_pets.is_empty() {
        return None;
    }
    Some(enabled_pets[(floor_seed as usize) % enabled_pets.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_name_roundtrip() {
        for &kind in PetKind::ALL {
            assert_eq!(PetKind::from_config_name(kind.config_name()), Some(kind));
        }
    }

    #[test]
    fn from_config_name_unknown_returns_none() {
        assert_eq!(PetKind::from_config_name("hamster"), None);
    }

    #[test]
    fn select_pet_empty_returns_none() {
        assert_eq!(select_pet_for_floor(42, &[]), None);
    }

    #[test]
    fn select_pet_single_always_returns_it() {
        assert_eq!(select_pet_for_floor(0, &[PetKind::Dog]), Some(PetKind::Dog));
        assert_eq!(select_pet_for_floor(99, &[PetKind::Dog]), Some(PetKind::Dog));
    }

    #[test]
    fn select_pet_two_pets_alternates_by_seed() {
        let pets = vec![PetKind::Cat, PetKind::Dog];
        let floor0 = select_pet_for_floor(0, &pets);
        let floor1 = select_pet_for_floor(1, &pets);
        assert_ne!(floor0, floor1);
    }

    #[test]
    fn anim_names_match_kind() {
        assert!(PetKind::Cat.walk_anim().starts_with("cat_"));
        assert!(PetKind::Dog.walk_anim().starts_with("dog_"));
    }
}
```

- [ ] **Step 2: Add module declaration**

In `crates/pixtuoid/src/tui/mod.rs`, add `pub mod pet;` after line 6 (after `pub mod hit_test;`).

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p pixtuoid -- tui::pet --nocapture`
Expected: All 6 tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/pixtuoid/src/tui/pet.rs crates/pixtuoid/src/tui/mod.rs
git commit -m "feat(pet): add PetKind enum and select_pet_for_floor"
```

---

### Task 2: Config layer — enabled-pets

**Files:**
- Modify: `crates/pixtuoid/src/config.rs:5-16` (AppConfig struct)

- [ ] **Step 1: Write failing tests for resolve_pets**

Add to `crates/pixtuoid/src/config.rs` test module:

```rust
#[test]
fn enabled_pets_none_returns_all() {
    let cfg = AppConfig::default();
    let pets = resolve_pets(&cfg);
    assert_eq!(pets.len(), crate::tui::pet::PetKind::ALL.len());
}

#[test]
fn enabled_pets_empty_returns_none() {
    let cfg = AppConfig {
        enabled_pets: Some(vec![]),
        ..AppConfig::default()
    };
    let pets = resolve_pets(&cfg);
    assert!(pets.is_empty());
}

#[test]
fn enabled_pets_filters_unknown() {
    let cfg = AppConfig {
        enabled_pets: Some(vec!["cat".into(), "hamster".into()]),
        ..AppConfig::default()
    };
    let pets = resolve_pets(&cfg);
    assert_eq!(pets, vec![crate::tui::pet::PetKind::Cat]);
}

#[test]
fn enabled_pets_loaded_from_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "enabled-pets = [\"dog\"]\n").unwrap();
    let cfg = load(&path);
    assert_eq!(cfg.enabled_pets, Some(vec!["dog".to_string()]));
}

#[test]
fn save_preserves_enabled_pets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"normal\"\nenabled-pets = [\"cat\", \"dog\"]\n").unwrap();
    save(&path, "cyberpunk").unwrap();
    let cfg = load(&path);
    assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
    assert_eq!(cfg.enabled_pets, Some(vec!["cat".to_string(), "dog".to_string()]));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p pixtuoid -- config::tests --nocapture`
Expected: FAIL — `enabled_pets` field and `resolve_pets` not found

- [ ] **Step 3: Implement config changes**

In `crates/pixtuoid/src/config.rs`, add to `AppConfig` struct (after `pack_dir` field at line 15):

```rust
    #[serde(rename = "enabled-pets", default, skip_serializing_if = "Option::is_none")]
    pub enabled_pets: Option<Vec<String>>,
```

Add `resolve_pets` function after `resolve_theme` (after line 110):

```rust
pub fn resolve_pets(config: &AppConfig) -> Vec<crate::tui::pet::PetKind> {
    match &config.enabled_pets {
        None => crate::tui::pet::PetKind::ALL.to_vec(),
        Some(names) => names
            .iter()
            .filter_map(|n| {
                let kind = crate::tui::pet::PetKind::from_config_name(n);
                if kind.is_none() {
                    tracing::warn!(pet = %n, "unknown pet in config — skipping");
                }
                kind
            })
            .collect(),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p pixtuoid -- config::tests --nocapture`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/pixtuoid/src/config.rs
git commit -m "feat(pet): add enabled-pets config field and resolve_pets"
```

---

### Task 3: Rename CatPetState → PetPetState and update DrawCtx

**Files:**
- Modify: `crates/pixtuoid/src/tui/renderer.rs:48-99` (struct + DrawCtx)
- Modify: `crates/pixtuoid/src/tui/tui_renderer.rs:29-50,53-72,132-142,163-166,386-408`
- Modify: `crates/pixtuoid/src/tui/mod.rs:176-183` (click handler)
- Modify: `crates/pixtuoid/src/tui/pixel_painter/mod.rs:66-82` (PixelCtx)

These files are tightly coupled — change them together.

- [ ] **Step 1: Rename CatPetState → PetPetState in renderer.rs**

In `crates/pixtuoid/src/tui/renderer.rs`:

Rename `CatPetState` to `PetPetState` (lines 53-71). Add `kind: PetKind` field:

```rust
pub struct PetPetState {
    pub petted_at: SystemTime,
    pub pet_pos: Point,
    pub kind: PetKind,
}
```

Add import at top: `use crate::tui::pet::PetKind;`

Update `DrawCtx` (lines 87-89):
- `cat_pet: Option<&'a CatPetState>` → `active_pet: Option<&'a PetPetState>`
- `last_cat_pos: Option<(Point, &'static str)>` → `last_pet_pos: Option<(Point, &'static str, PetKind)>`

Update re-exports (lines 38-39):
- `hit_test_cat` → `hit_test_pet`
- `paint_cat_tooltip` → `paint_pet_tooltip`

Update `draw_scene()`:
- Line 224 in PixelCtx: `cat_pet: ctx.cat_pet` → `active_pet: ctx.active_pet`
- Line 229: `ctx.last_cat_pos = pixel_result.cat_pos` → `ctx.last_pet_pos = pixel_result.pet_pos`
- Lines 301-304: update tooltip dispatch to use `last_pet_pos` tuple with `PetKind`

- [ ] **Step 2: Update TuiRenderer fields in tui_renderer.rs**

In `crates/pixtuoid/src/tui/tui_renderer.rs`:

Rename fields (lines 41-42):
- `cat_pet: Option<CatPetState>` → `active_pet: Option<PetPetState>`
- `last_cat_pos: Option<(Point, &'static str)>` → `last_pet_pos: Option<(Point, &'static str, PetKind)>`

Add field: `enabled_pets: Vec<PetKind>`

Update `new()` to accept `enabled_pets: Vec<PetKind>` parameter and store it.

Rename accessors (lines 132-142):
- `set_cat_pet` → `set_active_pet`
- `cat_pet` → `active_pet_ref`
- `cached_cat_pos` → `cached_pet_pos` (return type becomes `Option<(Point, &'static str, PetKind)>`)

Update pet expiry (lines 163-166): `self.cat_pet` → `self.active_pet`

Update DrawCtx assembly (lines 386-406):
- `cat_pet: self.cat_pet.as_ref()` → `active_pet: self.active_pet.as_ref()`
- `last_cat_pos: None` → `last_pet_pos: None`
- Post-draw: `self.last_cat_pos = draw_ctx.last_cat_pos` → `self.last_pet_pos = draw_ctx.last_pet_pos`

- [ ] **Step 3: Update PixelCtx and PixelPassResult in pixel_painter/mod.rs**

In `crates/pixtuoid/src/tui/pixel_painter/mod.rs`:

`PixelPassResult` (line 33): `cat_pos: Option<(Point, &'static str)>` → `pet_pos: Option<(Point, &'static str, PetKind)>`

`PixelCtx` (line 78): `cat_pet: Option<&'a CatPetState>` → `active_pet: Option<&'a PetPetState>`

Variable (line 130): `resolved_cat_pos` → `resolved_pet_pos` (type matches new `pet_pos`)

Return (line 1026): `cat_pos: resolved_cat_pos` → `pet_pos: resolved_pet_pos`

- [ ] **Step 4: Update click handler in tui/mod.rs**

In `crates/pixtuoid/src/tui/mod.rs` (lines 176-183):

```rust
} else if let Some((pet_pos, anim, kind)) = renderer.cached_pet_pos() {
    if renderer.active_pet_ref().map_or(true, |p| !p.is_active(now))
        && renderer::hit_test_pet(kind, pet_pos, anim, m.column, m.row)
    {
        renderer.set_active_pet(Some(renderer::PetPetState {
            petted_at: now,
            pet_pos,
            kind,
        }));
    } else {
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check --workspace`
Expected: compiles (some warnings about unused `hit_test_pet`/`paint_pet_tooltip` are OK — they're renamed in the next task)

- [ ] **Step 6: Run tests**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: All tests PASS

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(pet): rename CatPetState → PetPetState, update DrawCtx and TuiRenderer"
```

---

### Task 4: Generalize drawable — DrawableKind::Pet and pet_position()

**Files:**
- Modify: `crates/pixtuoid/src/tui/pixel_painter/drawable.rs:143-149,156-266,561-587`
- Modify: `crates/pixtuoid/src/tui/pixel_painter/mod.rs:762-797`

- [ ] **Step 1: Rename DrawableKind::Cat → DrawableKind::Pet**

In `crates/pixtuoid/src/tui/pixel_painter/drawable.rs`:

Add import: `use crate::tui::pet::PetKind;`

Replace variant (line 143-149):
```rust
Pet {
    kind: PetKind,
    pos: Point,
    flip: bool,
    anim_name: &'static str,
    frame_idx: usize,
    pet_elapsed_ms: Option<u64>,
},
```

Update paint arm (line 561): `DrawableKind::Cat {` → `DrawableKind::Pet { kind, pos, flip, anim_name, frame_idx, pet_elapsed_ms }` and change `"cat_sleep"` check to `*anim_name == kind.sleep_anim()`.

- [ ] **Step 2: Rename cat_position → pet_position**

Change signature (line 156):
```rust
pub(super) fn pet_position(
    kind: PetKind,
    layout: &Layout,
    pack: &Pack,
    now: SystemTime,
    idle_desk_indices: &[usize],
    all_idle: bool,
    pet_seed: u64,
) -> Option<(Point, bool, &'static str, usize)>
```

Inside the function body:
- Line 164: `pack.animation("cat_walk")?` → `pack.animation(kind.walk_anim())?`
- Lines 260-264: Replace the anim selection logic:

```rust
let anim = if kind.sleeps_near_idle() && (all_idle || is_idle_spot) {
    kind.sleep_anim()
} else {
    kind.sit_anim()
};
```

- [ ] **Step 3: Update the cat block in pixel_painter/mod.rs**

In `crates/pixtuoid/src/tui/pixel_painter/mod.rs` (lines 762-797):

Replace the cat block with a pet block:

```rust
if let Some(kind) = ctx.floor_pet_kind {
    let active_pet = ctx.active_pet.filter(|p| p.is_active(ctx.now) && p.kind == kind);
    let pet_data = if let Some(pet) = active_pet {
        Some((pet.pet_pos, false, kind.sit_anim(), 0usize, Some(pet.elapsed_ms(ctx.now))))
    } else {
        pet_position(kind, ctx.layout, ctx.pack, ctx.now, &idle_desk_indices, all_idle, ctx.floor.floor_seed)
            .map(|(pos, flip, anim, frame)| (pos, flip, anim, frame, None))
    };
    if let Some((pos, flip, anim_name, frame_idx, pet_elapsed)) = pet_data {
        resolved_pet_pos = Some((pos, anim_name, kind));
        drawables.push(Drawable {
            anchor_y: pos.y + 3,
            kind: DrawableKind::Pet { kind, pos, flip, anim_name, frame_idx, pet_elapsed_ms: pet_elapsed },
        });
    }
}
```

Add `floor_pet_kind: Option<PetKind>` to `PixelCtx` struct.

- [ ] **Step 4: Thread floor_pet_kind through DrawCtx and PixelCtx**

In `renderer.rs` `DrawCtx`: add `pub floor_pet_kind: Option<PetKind>`.

In `renderer.rs` `draw_scene()` PixelCtx construction: add `floor_pet_kind: ctx.floor_pet_kind`.

In `tui_renderer.rs` DrawCtx construction: add `floor_pet_kind: crate::tui::pet::select_pet_for_floor(FloorMeta::for_floor(self.current_floor, nf).floor_seed, &self.enabled_pets)`.

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(pet): generalize DrawableKind::Cat → Pet, cat_position → pet_position"
```

---

### Task 5: Generalize hit-test and tooltip

**Files:**
- Modify: `crates/pixtuoid/src/tui/hit_test.rs:287-305,393-423`
- Modify: `crates/pixtuoid/src/tui/widgets/tooltip.rs:295-343`
- Modify: `crates/pixtuoid/src/tui/renderer.rs:35-45` (re-exports)

- [ ] **Step 1: Rename hit_test_cat → hit_test_pet**

In `crates/pixtuoid/src/tui/hit_test.rs`:

Add import: `use crate::tui::pet::PetKind;`

Rename function and add `kind` parameter:
```rust
pub fn hit_test_pet(kind: PetKind, pet_pos: crate::tui::layout::Point, anim_name: &str, mx: u16, my: u16) -> bool {
    let (w, h) = kind.hitbox(anim_name);
    let tl_x = pet_pos.x.saturating_sub(w / 2);
    let tl_y = pet_pos.y.saturating_sub(h / 2);
    let cell_y = my * 2;
    mx >= tl_x && mx < tl_x.saturating_add(w) && cell_y >= tl_y && cell_y < tl_y.saturating_add(h)
}
```

Update existing tests to use `hit_test_pet(PetKind::Cat, ...)`.

- [ ] **Step 2: Rename paint_cat_tooltip → paint_pet_tooltip**

In `crates/pixtuoid/src/tui/widgets/tooltip.rs`:

Add import: `use crate::tui::pet::PetKind;`

Rename function and add `kind` parameter:
```rust
pub(crate) fn paint_pet_tooltip(
    f: &mut ratatui::Frame<'_>,
    kind: PetKind,
    anim_name: &str,
    is_on_cooldown: bool,
    mx: u16,
    my: u16,
    scene_rect: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let text = if is_on_cooldown {
        match kind {
            PetKind::Cat => " purr... ",
            PetKind::Dog => " woof! ",
        }
    } else {
        match (kind, anim_name) {
            (PetKind::Cat, "cat_sleep") | (PetKind::Dog, "dog_sleep") => " Shhh... sleeping ",
            (PetKind::Cat, "cat_sit") | (PetKind::Dog, "dog_sit") => " Pet me! ",
            (PetKind::Cat, _) => " Office Cat ",
            (PetKind::Dog, _) => " Office Dog ",
        }
    };
    // ... rest of tooltip rendering unchanged
}
```

- [ ] **Step 3: Update re-exports in renderer.rs**

Change `hit_test_cat` → `hit_test_pet` and `paint_cat_tooltip` → `paint_pet_tooltip` in the `pub use` blocks.

- [ ] **Step 4: Update draw_scene tooltip dispatch**

In `renderer.rs` `draw_scene()`, the tooltip section (around lines 301-304) should use the new signatures:

```rust
if let Some((cat_pos, anim, kind)) = ctx.last_pet_pos {
    if hit_test_cat(cat_pos, anim, mx, my) {
```
→
```rust
if let Some((pet_pos, anim, kind)) = ctx.last_pet_pos {
    if hit_test_pet(kind, pet_pos, anim, mx, my) {
```

And the tooltip call should pass `kind`:
```rust
paint_pet_tooltip(f, kind, anim, on_cooldown, mx, my, actual_scene, theme);
```

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(pet): generalize hit_test_cat → hit_test_pet, paint_cat_tooltip → paint_pet_tooltip"
```

---

### Task 6: Wire config through to TuiRenderer

**Files:**
- Modify: `crates/pixtuoid/src/main.rs:48-69` (Cmd::Run handler)
- Modify: `crates/pixtuoid/src/runtime.rs:30-60,99` (run function + run_tui call)
- Modify: `crates/pixtuoid/src/tui/mod.rs:27-34` (run_tui signature)

- [ ] **Step 1: Add enabled_pets to runtime::run signature**

In `crates/pixtuoid/src/runtime.rs`, add `enabled_pets: Vec<crate::tui::pet::PetKind>` parameter to `run()`.

Thread it to the `run_tui` call:
```rust
crate::tui::run_tui(scene_rx, pack_dir, floor_caps, theme, config_path, desk_cap, enabled_pets).await
```

- [ ] **Step 2: Update run_tui signature and TuiRenderer construction**

In `crates/pixtuoid/src/tui/mod.rs`, add `enabled_pets: Vec<pet::PetKind>` to `run_tui()` signature.

Pass `enabled_pets` to `TuiRenderer::new()`.

- [ ] **Step 3: Resolve pets in main.rs**

In `crates/pixtuoid/src/main.rs`, inside the `Cmd::Run` handler (around line 58), add:
```rust
let enabled_pets = config::resolve_pets(&cfg);
```

Pass to `runtime::run(...)`:
```rust
runtime::run(
    socket,
    projects_root,
    pack_dir,
    desk_cap,
    headless,
    theme_name,
    cfg_path,
    enabled_pets,
)
```

- [ ] **Step 4: Handle headless mode**

In `runtime.rs`, the headless path doesn't use TuiRenderer, so `enabled_pets` is just ignored there. No changes needed — just ensure the parameter is accepted.

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(pet): wire enabled-pets config through main → runtime → TuiRenderer"
```

---

### Task 7: Dog sprites and pack registration

**Files:**
- Create: `crates/pixtuoid/sprites/default/dog_walk_0.sprite`
- Create: `crates/pixtuoid/sprites/default/dog_walk_1.sprite`
- Create: `crates/pixtuoid/sprites/default/dog_sit.sprite`
- Create: `crates/pixtuoid/sprites/default/dog_sleep.sprite`
- Modify: `crates/pixtuoid/sprites/default/pack.toml:7-40,115-125`
- Modify: `crates/pixtuoid/src/tui/embedded_pack.rs:96-143`
- Modify: `crates/pixtuoid-core/src/sprite/format.rs:270-303`

- [ ] **Step 1: Add dog to OPTIONAL_FURNITURE_ANIMATIONS**

In `crates/pixtuoid-core/src/sprite/format.rs`:

Add `"dog_walk"`, `"dog_sit"`, `"dog_sleep"` to `OPTIONAL_FURNITURE_ANIMATIONS` (after the cat entries around line 277).

Add `("dog_walk", 2)` to `MULTI_FRAME_REQUIREMENTS` (after `("cat_walk", 2)` at line 302).

- [ ] **Step 2: Create dog sprite files**

`crates/pixtuoid/sprites/default/dog_walk_0.sprite`:
```
# 8x6 dog walking (frame 0). Side view — floppy ears, wagging tail up.
# Palette: x = tan fur (#c8a060), z = dark brown (#7a5030)
@frame 0
. z x x x . . .
. x x x x . . z
. x e x x x x z
. x x x x x x .
. . x . . x . .
. . . . . . . .
```

`crates/pixtuoid/sprites/default/dog_walk_1.sprite`:
```
# 8x6 dog walking (frame 1). Alternate legs, tail mid.
@frame 0
. z x x x . . .
. x x x x . z .
. x e x x x x z
. x x x x x x .
. x . . x . . .
. . . . . . . .
```

`crates/pixtuoid/sprites/default/dog_sit.sprite`:
```
# 6x6 dog sitting — front view, ears floppy, tongue out.
@frame 0
z x . x z .
x x x x x .
x e x e x .
x x r x x .
. x x x . .
. . . . . .
```

`crates/pixtuoid/sprites/default/dog_sleep.sprite`:
```
# 6x4 dog sleeping — curled up, nose tucked.
@frame 0
. z x z . .
. x x x x .
. x x x x .
. . z z . .
```

- [ ] **Step 3: Add palette keys and animations to pack.toml**

In `crates/pixtuoid/sprites/default/pack.toml`:

Add palette entries (in the `[palette]` section):
```toml
"x" = "#c8a060"   # dog fur (tan)
"z" = "#7a5030"   # dog dark (ears, nose, tail)
```

Add animation entries (after the cat animations):
```toml
[animations.dog_walk]
frames   = ["dog_walk_0.sprite", "dog_walk_1.sprite"]
frame_ms = 220

[animations.dog_sit]
frames   = ["dog_sit.sprite"]
frame_ms = 600

[animations.dog_sleep]
frames   = ["dog_sleep.sprite"]
frame_ms = 600
```

- [ ] **Step 4: Add include_str! entries in embedded_pack.rs**

In `crates/pixtuoid/src/tui/embedded_pack.rs`, after the cat includes (around line 99):

```rust
let dog_0 = include_str!("../../sprites/default/dog_walk_0.sprite");
let dog_1 = include_str!("../../sprites/default/dog_walk_1.sprite");
let dog_sit = include_str!("../../sprites/default/dog_sit.sprite");
let dog_sleep = include_str!("../../sprites/default/dog_sleep.sprite");
```

And add to the tuple array (after cat entries around line 143):
```rust
("dog_walk_0.sprite", dog_0),
("dog_walk_1.sprite", dog_1),
("dog_sit.sprite", dog_sit),
("dog_sleep.sprite", dog_sleep),
```

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: All tests PASS

- [ ] **Step 6: Visual verification**

```bash
cargo build --release --example snapshot
./target/release/examples/snapshot --cols 192 --rows 80 /tmp/snap.png
.venv/bin/python3 scripts/crop-snapshot.py /tmp/snap.png --scale 3
```

Read the cropped PNG and verify the dog sprite renders correctly at half-block scale. If the dog is not recognizable, iterate on the sprite design.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(pet): add dog sprites, pack registration, and embedded_pack includes"
```

---

### Task 8: Full preflight and cleanup

**Files:**
- Modify: `CLAUDE.md` (update "Where to look" → cat section to mention pet system)

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace --features pixtuoid-core/test-renderer`
Expected: All tests PASS (330+)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace --all-targets --features pixtuoid-core/test-renderer -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run full preflight**

Run: `./scripts/preflight.sh`
Expected: All checks pass (fmt, machete, deny, clippy, tests)

- [ ] **Step 4: Build release and test manually**

```bash
cargo build --release --workspace
./target/release/pixtuoid run
```

Verify:
- Cat appears on some floors, dog on others
- Both pet types can be clicked/petted (hearts animation)
- Tooltips show correct text ("Office Cat" vs "Office Dog", "purr..." vs "woof!")
- Cat sleeps near idle agents, dog sits near active agents
- Resize doesn't crash

- [ ] **Step 5: Test config**

Create `~/.config/pixtuoid/config.toml`:
```toml
enabled-pets = ["dog"]
```

Restart — verify only dogs appear on all floors.

Change to `enabled-pets = []` — verify no pets appear.

Remove the key — verify both pets appear again.

- [ ] **Step 6: Commit and update docs**

Update CLAUDE.md "How does the cat behave?" section to mention the pet system, PetKind, and floor selection.

```bash
git add -A
git commit -m "docs: update CLAUDE.md for generic pet system"
```
