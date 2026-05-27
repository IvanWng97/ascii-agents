//! Per-agent palette (shirt / hair / skin) + frame recolor + color math
//! primitives (blend / lerp / bell / mix_lab).
//!
//! `agent_palette` picks a deterministic shirt/hair/skin from per-agent
//! hashes; `recolor_frame` rewrites a frame's pixels by RGB-equality
//! against the base pack palette. The color-math helpers live here too
//! because the palette tint code uses them directly and they're widely
//! shared with background/effects.

use pixtuoid_core::sprite::{Frame, Palette, Pixel, Rgb};
use pixtuoid_core::AgentSlot;

use crate::tui::pose;

/// A complete shirt + pants combo. We pick *outfits* per agent rather
/// than independent shirt and pants colors so the result is always a
/// harmonious pairing (designed together by someone who knows color)
/// instead of a random clash. Sources: Wes Anderson stills, Studio
/// Ghibli character art, modern office capsule-wardrobe palettes.
#[derive(Clone, Copy)]
struct Outfit {
    shirt: Rgb,
    pants: Rgb,
}

/// Warm / extroverted outfits — earthy reds, ochres, terracottas paired
/// with deep neutrals. Used for agents with higher trip_chance_pct.
const OUTFITS_WARM: &[Outfit] = &[
    // Wes Anderson — Grand Budapest concierge (cream + plum)
    Outfit {
        shirt: Rgb(0xee, 0xe1, 0xc6),
        pants: Rgb(0x4a, 0x2b, 0x3d),
    },
    // Ghibli earthy — terracotta + sand
    Outfit {
        shirt: Rgb(0xc9, 0x7b, 0x5e),
        pants: Rgb(0x6b, 0x57, 0x3d),
    },
    // 70s academic — mustard + olive
    Outfit {
        shirt: Rgb(0xc9, 0xa2, 0x4b),
        pants: Rgb(0x4a, 0x52, 0x34),
    },
    // Burgundy + warm stone (moody academic)
    Outfit {
        shirt: Rgb(0x8a, 0x2c, 0x36),
        pants: Rgb(0x5a, 0x4e, 0x42),
    },
    // Mediterranean — coral + dark navy
    Outfit {
        shirt: Rgb(0xd7, 0x7a, 0x61),
        pants: Rgb(0x27, 0x33, 0x4a),
    },
    // Camel + chocolate (luxury minimal)
    Outfit {
        shirt: Rgb(0xb8, 0x99, 0x68),
        pants: Rgb(0x3d, 0x2a, 0x1f),
    },
    // Rust + cream (autumn)
    Outfit {
        shirt: Rgb(0xa5, 0x4f, 0x2c),
        pants: Rgb(0xcd, 0xc0, 0xa3),
    },
    // Salmon + warm charcoal
    Outfit {
        shirt: Rgb(0xe0, 0x90, 0x7c),
        pants: Rgb(0x3a, 0x32, 0x2e),
    },
];

/// Cool / homebody outfits — sages, slates, indigos paired with deeper
/// neutrals. Used for agents with lower trip_chance_pct.
const OUTFITS_COOL: &[Outfit] = &[
    // Modern minimal — sage + charcoal
    Outfit {
        shirt: Rgb(0xa4, 0xb5, 0x95),
        pants: Rgb(0x33, 0x36, 0x3d),
    },
    // Professional — pale blue + slate
    Outfit {
        shirt: Rgb(0x9b, 0xb5, 0xc8),
        pants: Rgb(0x3c, 0x44, 0x52),
    },
    // Soft moody — lavender + espresso
    Outfit {
        shirt: Rgb(0xa2, 0x90, 0xb0),
        pants: Rgb(0x3c, 0x2a, 0x1e),
    },
    // Outdoorsy — forest green + khaki
    Outfit {
        shirt: Rgb(0x3f, 0x61, 0x4c),
        pants: Rgb(0x7a, 0x67, 0x48),
    },
    // Confident — teal + cream
    Outfit {
        shirt: Rgb(0x3e, 0x7a, 0x85),
        pants: Rgb(0xc7, 0xb6, 0x96),
    },
    // Preppy — indigo + warm grey
    Outfit {
        shirt: Rgb(0x3f, 0x4a, 0x75),
        pants: Rgb(0x8a, 0x84, 0x7a),
    },
    // Nordic — dusty blue + navy
    Outfit {
        shirt: Rgb(0x6b, 0x84, 0xa0),
        pants: Rgb(0x2a, 0x33, 0x4a),
    },
    // Mossy — pine + bone
    Outfit {
        shirt: Rgb(0x47, 0x69, 0x5a),
        pants: Rgb(0xb8, 0xae, 0x95),
    },
];

/// 8 hair colors — was 5. Added silver/grey for older-coded agents,
/// ginger / strawberry blonde / jet black for more silhouette variety.
const HAIR_PRESETS: &[Rgb] = &[
    Rgb(0x14, 0x0a, 0x06), // jet black
    Rgb(0x2a, 0x1a, 0x0e), // near-black brown
    Rgb(0x52, 0x32, 0x10), // dark brown
    Rgb(0x8a, 0x5a, 0x36), // light brown
    Rgb(0xc7, 0xa3, 0x4a), // blond
    Rgb(0xd8, 0x68, 0x32), // ginger
    Rgb(0x7a, 0x32, 0x10), // auburn
    Rgb(0xa8, 0xa8, 0xb0), // silver-grey
];
const SKIN_PRESETS: &[Rgb] = &[
    Rgb(0xf4, 0xc7, 0x9a), // light peach (matches base palette S)
    Rgb(0xe0, 0xa8, 0x70), // medium
    Rgb(0xb8, 0x80, 0x50), // tan
    Rgb(0x8a, 0x5a, 0x36), // deep brown
    Rgb(0xc8, 0x9a, 0x64), // warm tan
];

/// Build the per-agent palette. `glow_tint` carries the monitor-glow
/// color when the agent is seated at a lit screen (SeatedTyping). The
/// skin blends 18% toward that tint so the eye reads "the monitor is
/// lighting them up." `None` means no glow — skin stays natural.
///
/// The color varies by tool type so scanning a row of typing agents
/// gives an at-a-glance read of what they're working on:
///   green  = generic / default
///   blue   = Edit / Write
///   cyan   = Read
///   orange = Bash
///   purple = Agent / Task
pub(super) fn agent_palette(base: &Palette, agent: &AgentSlot, glow_tint: Option<Rgb>) -> Palette {
    let seed = agent.agent_id.raw() as usize;
    let p = pose::personality_for(agent.agent_id);
    let outfits = if p.trip_chance_pct >= 30 {
        OUTFITS_WARM
    } else {
        OUTFITS_COOL
    };
    let outfit = outfits[seed % outfits.len()];
    let hair = HAIR_PRESETS[(seed / 7) % HAIR_PRESETS.len()];
    let skin = SKIN_PRESETS[(seed / 13) % SKIN_PRESETS.len()];
    let final_skin = if let Some(tint) = glow_tint {
        Rgb(
            blend(skin.0, tint.0, 0.18),
            blend(skin.1, tint.1, 0.18),
            blend(skin.2, tint.2, 0.18),
        )
    } else {
        skin
    };
    base.with_override('B', Some(outfit.shirt))
        .with_override('H', Some(hair))
        .with_override('S', Some(final_skin))
        .with_override('P', Some(outfit.pants))
}

/// Map an agent's active tool detail to a monitor glow color.
/// Returns `None` for non-Active states (no glow).
pub(super) fn tool_glow_tint(
    agent: &AgentSlot,
    glow: &crate::tui::theme::ToolGlowColors,
) -> Option<Rgb> {
    use pixtuoid_core::state::ActivityState;
    let detail = match &agent.state {
        ActivityState::Active { detail, .. } => detail.as_deref(),
        _ => return None,
    };
    let token = detail
        .and_then(|d| d.split(|c: char| !c.is_alphanumeric()).next())
        .unwrap_or("");
    Some(match token {
        "Edit" | "Write" | "MultiEdit" => glow.edit,
        "Read" => glow.read,
        "Bash" => glow.bash,
        "Agent" | "Task" | "Delegating" => glow.agent,
        "Grep" | "Glob" => glow.grep,
        _ => glow.default,
    })
}

pub(super) fn recolor_frame(frame: &Frame, pal: &Palette, base_pal: &Palette) -> Frame {
    let base_shirt = base_pal.get('B').flatten();
    let base_hair = base_pal.get('H').flatten();
    let base_skin = base_pal.get('S').flatten();
    let base_pants = base_pal.get('P').flatten();
    let agent_shirt = pal.get('B').flatten();
    let agent_hair = pal.get('H').flatten();
    let agent_skin = pal.get('S').flatten();
    let agent_pants = pal.get('P').flatten();
    let pixels: Vec<Pixel> = frame
        .pixels
        .iter()
        .map(|p| match p {
            Some(rgb) if Some(*rgb) == base_shirt => agent_shirt,
            Some(rgb) if Some(*rgb) == base_hair => agent_hair,
            Some(rgb) if Some(*rgb) == base_skin => agent_skin,
            Some(rgb) if Some(*rgb) == base_pants => agent_pants,
            other => *other,
        })
        .collect();
    Frame {
        width: frame.width,
        height: frame.height,
        pixels,
    }
}

// --- Color math primitives -----------------------------------------------

pub(super) fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    mix_lab(a, b, t)
}

/// Bell curve centered at `c` with half-width `w` (so the bell is 0 at
/// `c ± w` and 1 at `c`). Used for dawn/dusk twilight tint.
pub(super) fn bell(x: f32, c: f32, w: f32) -> f32 {
    let d = (x - c) / w;
    (1.0 - d * d).max(0.0)
}

/// Per-channel sRGB lerp. Cheap; used for low-strength tints where
/// perceptual error doesn't matter (e.g. agent skin glow).
pub(super) fn blend(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) * (1.0 - t) + (b as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Perceptually-correct Lab-space mix between two sRGB colors. Twilight
/// (orange → navy) and dim overlays travel cleanly through Lab without the
/// muddy desaturated midpoint that naive sRGB lerp produces. Slower than
/// `blend()` but only used where the perceptual difference is visible.
pub(super) fn mix_lab(a: Rgb, b: Rgb, t: f32) -> Rgb {
    use palette::{FromColor, IntoColor, Lab, Mix, Srgb};
    let sa = Srgb::new(a.0 as f32 / 255.0, a.1 as f32 / 255.0, a.2 as f32 / 255.0);
    let sb = Srgb::new(b.0 as f32 / 255.0, b.1 as f32 / 255.0, b.2 as f32 / 255.0);
    let la = Lab::from_color(sa);
    let lb = Lab::from_color(sb);
    let mixed: Srgb = la.mix(lb, t.clamp(0.0, 1.0)).into_color();
    Rgb(
        (mixed.red.clamp(0.0, 1.0) * 255.0).round() as u8,
        (mixed.green.clamp(0.0, 1.0) * 255.0).round() as u8,
        (mixed.blue.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}
