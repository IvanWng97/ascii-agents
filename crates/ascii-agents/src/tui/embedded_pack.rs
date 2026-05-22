//! Embeds the bundled top-down sprite pack into the binary at compile time.

use anyhow::Result;
use ascii_agents_core::sprite::format::{load_pack_from_strings, Pack};

pub fn load_default_pack() -> Result<Pack> {
    let pack_toml = include_str!("../../../../assets/sprites/default/pack.toml");
    let seated     = include_str!("../../../../assets/sprites/default/seated.sprite");
    let typing_0   = include_str!("../../../../assets/sprites/default/typing_0.sprite");
    let typing_1   = include_str!("../../../../assets/sprites/default/typing_1.sprite");
    let standing   = include_str!("../../../../assets/sprites/default/standing.sprite");
    let walking_0  = include_str!("../../../../assets/sprites/default/walking_0.sprite");
    let walking_1  = include_str!("../../../../assets/sprites/default/walking_1.sprite");
    let desk       = include_str!("../../../../assets/sprites/default/desk.sprite");
    let plant      = include_str!("../../../../assets/sprites/default/plant.sprite");
    let plant_tall = include_str!("../../../../assets/sprites/default/plant_tall.sprite");
    let plant_fl   = include_str!("../../../../assets/sprites/default/plant_flower.sprite");
    let plant_suc  = include_str!("../../../../assets/sprites/default/plant_succulent.sprite");
    let floor_lamp = include_str!("../../../../assets/sprites/default/floor_lamp.sprite");
    let trash_bin  = include_str!("../../../../assets/sprites/default/trash_bin.sprite");
    let couch      = include_str!("../../../../assets/sprites/default/couch.sprite");
    let coffee     = include_str!("../../../../assets/sprites/default/coffee.sprite");
    let sitting      = include_str!("../../../../assets/sprites/default/sitting_couch.sprite");
    let sleeping_seat= include_str!("../../../../assets/sprites/default/seated_sleeping.sprite");
    let sleeping_cch = include_str!("../../../../assets/sprites/default/sitting_couch_sleeping.sprite");
    let holding      = include_str!("../../../../assets/sprites/default/holding_coffee.sprite");
    let water_cooler = include_str!("../../../../assets/sprites/default/water_cooler.sprite");
    let whiteboard   = include_str!("../../../../assets/sprites/default/whiteboard.sprite");
    let bookshelf    = include_str!("../../../../assets/sprites/default/bookshelf.sprite");

    load_pack_from_strings(
        pack_toml,
        &[
            ("seated.sprite", seated),
            ("typing_0.sprite", typing_0),
            ("typing_1.sprite", typing_1),
            ("standing.sprite", standing),
            ("walking_0.sprite", walking_0),
            ("walking_1.sprite", walking_1),
            ("desk.sprite", desk),
            ("plant.sprite", plant),
            ("plant_tall.sprite", plant_tall),
            ("plant_flower.sprite", plant_fl),
            ("plant_succulent.sprite", plant_suc),
            ("floor_lamp.sprite", floor_lamp),
            ("trash_bin.sprite", trash_bin),
            ("couch.sprite", couch),
            ("coffee.sprite", coffee),
            ("sitting_couch.sprite", sitting),
            ("seated_sleeping.sprite", sleeping_seat),
            ("sitting_couch_sleeping.sprite", sleeping_cch),
            ("holding_coffee.sprite", holding),
            ("water_cooler.sprite", water_cooler),
            ("whiteboard.sprite", whiteboard),
            ("bookshelf.sprite", bookshelf),
        ],
    )
}
