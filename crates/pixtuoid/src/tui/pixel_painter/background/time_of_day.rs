//! Time-of-day derived state — glass colors, sunlight spill, weather,
//! sunset strength, and nighttime floor dim overlay.

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::tui::pixel_painter::palette::{blend, lerp_rgb};
use crate::tui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui::pixel_painter) enum Weather {
    Clear,
    Rain,
    Storm,
    Snow,
    Fog,
    Overcast,
    Windy,
}

pub(in crate::tui::pixel_painter) fn weather_state(now: SystemTime) -> Weather {
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cycle = secs / 600;
    let mut h = cycle.wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    match h % 14 {
        0..=5 => Weather::Clear,
        6..=7 => Weather::Rain,
        8 => Weather::Storm,
        9 => Weather::Snow,
        10 => Weather::Fog,
        11..=12 => Weather::Overcast,
        _ => Weather::Windy,
    }
}

pub(in crate::tui::pixel_painter) fn sunset_strength(now: SystemTime) -> f32 {
    use chrono::Timelike;
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    let h = local.hour() as f32 + local.minute() as f32 / 60.0;
    crate::tui::pixel_painter::palette::bell(h, 18.0, 1.5)
        .max(crate::tui::pixel_painter::palette::bell(h, 6.5, 1.0))
}

/// Window glass color + spill intensity + spill slant for the current local
/// hour. `spill_slant` is x-shift per row going down: positive = rightward
/// (morning sun in the east), negative = leftward (evening sun in the west).
/// `darkness` is 1 - daylight, used to drive artificial-light effects.
pub(in crate::tui::pixel_painter) struct TimeOfDayLook {
    pub(in crate::tui::pixel_painter) glass_a: Rgb,
    pub(in crate::tui::pixel_painter) glass_b: Rgb,
    pub(in crate::tui::pixel_painter) spill_strength: f32,
    pub(in crate::tui::pixel_painter) spill_slant: f32,
    pub(in crate::tui::pixel_painter) darkness: f32,
}

pub(in crate::tui::pixel_painter) fn time_of_day_look(
    now: SystemTime,
    theme: &Theme,
) -> TimeOfDayLook {
    use chrono::Timelike;
    let unix_now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    let h = local.hour() as f32 + local.minute() as f32 / 60.0;

    // Daylight intensity: full from 8 to 17, smooth ramp 5..8 and 17..20.
    let day = if !(5.0..20.0).contains(&h) {
        0.0
    } else if h < 8.0 {
        (h - 5.0) / 3.0
    } else if h < 17.0 {
        1.0
    } else {
        1.0 - (h - 17.0) / 3.0
    };

    // Twilight bell at dawn (~6.5) and dusk (~18.5) — adds orange/pink
    // tint that the cyan↔dark-blue base doesn't capture.
    let twilight = crate::tui::pixel_painter::palette::bell(h, 6.5, 1.5)
        .max(crate::tui::pixel_painter::palette::bell(h, 18.5, 1.5));

    let day_a = theme.lighting.day_sky_a;
    let day_b = theme.lighting.day_sky_b;
    let night_a = theme.lighting.night_sky_a;
    let night_b = theme.lighting.night_sky_b;
    let twilight_a = theme.lighting.twilight_a;
    let twilight_b = theme.lighting.twilight_b;

    let glass_a = lerp_rgb(lerp_rgb(night_a, day_a, day), twilight_a, twilight * 0.5);
    let glass_b = lerp_rgb(lerp_rgb(night_b, day_b, day), twilight_b, twilight * 0.5);

    // Spill slant: ±0.7 px per row at peak hours (6am leftmost, 6pm
    // rightmost), zero at noon. Conventional read: morning sun on the east
    // (right of image) casts light westward (leftward shift); evening sun
    // on the west casts eastward (rightward shift).
    let slant = if h < 12.0 {
        -((12.0 - h) / 6.0).clamp(0.0, 1.0) * 0.7
    } else {
        ((h - 12.0) / 6.0).clamp(0.0, 1.0) * 0.7
    };

    TimeOfDayLook {
        glass_a,
        glass_b,
        spill_strength: day,
        spill_slant: slant,
        darkness: 1.0 - day,
    }
}

/// Multiplicative dim applied to floor pixels at night. Pulls everything
/// toward a dark navy so the artificial-light pools have something to
/// stand out against. `strength` is 0..1 (no dim..full dim).
pub(in crate::tui::pixel_painter) fn dim_floor_overlay(
    buf: &mut RgbBuffer,
    top_y: u16,
    bottom_y: u16,
    strength: f32,
    theme: &Theme,
) {
    let night_tint = theme.lighting.night_tint;
    let s = strength.clamp(0.0, 0.55);
    for y in top_y..bottom_y.min(buf.height) {
        for x in 0..buf.width {
            let cur = buf.get(x, y);
            buf.put(
                x,
                y,
                Rgb(
                    blend(cur.0, night_tint.0, s),
                    blend(cur.1, night_tint.1, s),
                    blend(cur.2, night_tint.2, s),
                ),
            );
        }
    }
}
