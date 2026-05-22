//! Sprite pack loader.
//!
//! Tries the user-config path first (XDG-style) so power users can drop in a
//! custom pack without recompiling. Falls back to the embedded default pack
//! (compile-time `include_str!`) so the binary ships standalone.
//!
//! ## Custom pack layout
//!
//! Drop a directory at `${XDG_CONFIG_HOME:-~/.config}/ascii-agents/sprites/`
//! containing `pack.toml` + each `.sprite` file referenced from the TOML.
//! See `assets/sprites/default/` for the canonical example.
//!
//! ## Sharp edge — palette RGB uniqueness
//!
//! The renderer's per-agent recolor (`recolor_frame` in `tui::renderer`)
//! substitutes the H/S/B palette colors by RGB equality. If a custom pack
//! reuses the same RGB for two palette keys, the recolor pass will substitute
//! both, producing visual artifacts. Each palette key MUST map to a unique
//! RGB triple.

use std::path::PathBuf;

use anyhow::Result;
use ascii_agents_core::sprite::format::{load_pack, load_pack_from_strings, Pack};

/// Resolve the user's sprite-pack directory if XDG settings point at one.
/// Returns the directory only when `pack.toml` exists inside it — otherwise
/// the caller falls back to the embedded pack.
fn xdg_pack_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let dir = base.join("ascii-agents").join("sprites");
    if dir.join("pack.toml").is_file() {
        Some(dir)
    } else {
        None
    }
}

pub fn load_default_pack() -> Result<Pack> {
    if let Some(dir) = xdg_pack_dir() {
        match load_pack(&dir) {
            Ok(p) => {
                tracing::info!(path = %dir.display(), "loaded user sprite pack");
                return Ok(p);
            }
            Err(e) => {
                tracing::warn!(
                    path = %dir.display(),
                    error = %e,
                    "user sprite pack failed to load; falling back to embedded default"
                );
            }
        }
    }
    load_embedded_pack()
}

fn load_embedded_pack() -> Result<Pack> {
    let pack_toml = include_str!("../../../../assets/sprites/default/pack.toml");
    let seated = include_str!("../../../../assets/sprites/default/seated.sprite");
    let typing_0 = include_str!("../../../../assets/sprites/default/typing_0.sprite");
    let typing_1 = include_str!("../../../../assets/sprites/default/typing_1.sprite");
    let standing = include_str!("../../../../assets/sprites/default/standing.sprite");
    let walking_0 = include_str!("../../../../assets/sprites/default/walking_0.sprite");
    let walking_1 = include_str!("../../../../assets/sprites/default/walking_1.sprite");
    let walking_back_0 = include_str!("../../../../assets/sprites/default/walking_back_0.sprite");
    let walking_back_1 = include_str!("../../../../assets/sprites/default/walking_back_1.sprite");
    let desk = include_str!("../../../../assets/sprites/default/desk.sprite");
    let plant = include_str!("../../../../assets/sprites/default/plant.sprite");
    let plant_tall = include_str!("../../../../assets/sprites/default/plant_tall.sprite");
    let plant_fl = include_str!("../../../../assets/sprites/default/plant_flower.sprite");
    let plant_suc = include_str!("../../../../assets/sprites/default/plant_succulent.sprite");
    let floor_lamp = include_str!("../../../../assets/sprites/default/floor_lamp.sprite");
    let trash_bin = include_str!("../../../../assets/sprites/default/trash_bin.sprite");
    let door = include_str!("../../../../assets/sprites/default/door.sprite");
    let bulletin = include_str!("../../../../assets/sprites/default/bulletin_board.sprite");
    let exit_sign = include_str!("../../../../assets/sprites/default/exit_sign.sprite");
    let filing = include_str!("../../../../assets/sprites/default/filing_cabinet.sprite");
    let cat_0 = include_str!("../../../../assets/sprites/default/cat_walk_0.sprite");
    let cat_1 = include_str!("../../../../assets/sprites/default/cat_walk_1.sprite");
    let floor_seat = include_str!("../../../../assets/sprites/default/seated_floor.sprite");
    let floor_slp = include_str!("../../../../assets/sprites/default/seated_floor_sleeping.sprite");
    let couch = include_str!("../../../../assets/sprites/default/couch.sprite");
    let coffee = include_str!("../../../../assets/sprites/default/coffee.sprite");
    let sitting = include_str!("../../../../assets/sprites/default/sitting_couch.sprite");
    let back_couch = include_str!("../../../../assets/sprites/default/back_couch.sprite");
    let sleeping_seat = include_str!("../../../../assets/sprites/default/seated_sleeping.sprite");
    let sleeping_cch =
        include_str!("../../../../assets/sprites/default/sitting_couch_sleeping.sprite");
    let holding = include_str!("../../../../assets/sprites/default/holding_coffee.sprite");
    let pantry = include_str!("../../../../assets/sprites/default/pantry.sprite");
    let whiteboard = include_str!("../../../../assets/sprites/default/whiteboard.sprite");
    let bookshelf = include_str!("../../../../assets/sprites/default/bookshelf.sprite");

    load_pack_from_strings(
        pack_toml,
        &[
            ("seated.sprite", seated),
            ("typing_0.sprite", typing_0),
            ("typing_1.sprite", typing_1),
            ("standing.sprite", standing),
            ("walking_0.sprite", walking_0),
            ("walking_1.sprite", walking_1),
            ("walking_back_0.sprite", walking_back_0),
            ("walking_back_1.sprite", walking_back_1),
            ("desk.sprite", desk),
            ("plant.sprite", plant),
            ("plant_tall.sprite", plant_tall),
            ("plant_flower.sprite", plant_fl),
            ("plant_succulent.sprite", plant_suc),
            ("floor_lamp.sprite", floor_lamp),
            ("trash_bin.sprite", trash_bin),
            ("door.sprite", door),
            ("bulletin_board.sprite", bulletin),
            ("exit_sign.sprite", exit_sign),
            ("filing_cabinet.sprite", filing),
            ("cat_walk_0.sprite", cat_0),
            ("cat_walk_1.sprite", cat_1),
            ("seated_floor.sprite", floor_seat),
            ("seated_floor_sleeping.sprite", floor_slp),
            ("couch.sprite", couch),
            ("coffee.sprite", coffee),
            ("sitting_couch.sprite", sitting),
            ("back_couch.sprite", back_couch),
            ("seated_sleeping.sprite", sleeping_seat),
            ("sitting_couch_sleeping.sprite", sleeping_cch),
            ("holding_coffee.sprite", holding),
            ("pantry.sprite", pantry),
            ("whiteboard.sprite", whiteboard),
            ("bookshelf.sprite", bookshelf),
        ],
    )
}
