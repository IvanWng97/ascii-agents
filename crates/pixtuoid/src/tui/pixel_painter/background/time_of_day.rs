//! Time-of-day derived state — glass colors, sunlight spill, weather,
//! sunset strength, and nighttime floor dim overlay.

use std::time::SystemTime;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::tui::pixel_painter::palette::{blend, lerp_rgb};
use crate::tui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::tui::pixel_painter) enum Weather {
    Clear,
    Rain,
    Storm,
    Snow,
    Fog,
    Overcast,
    Windy,
    Smog,
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
    match h % 15 {
        0..=5 => Weather::Clear,
        6..=7 => Weather::Rain,
        8 => Weather::Storm,
        9 => Weather::Snow,
        10 => Weather::Fog,
        11..=12 => Weather::Overcast,
        13 => Weather::Windy,
        _ => Weather::Smog,
    }
}

/// Atmospheric attenuation of outdoor sunlight reaching the interior.
/// `intensity` is a 0..1 multiplier applied to every sun-driven effect
/// (window spill warmth, sun spot on wall, dust motes, twilight glass
/// tint). `has_direct_beam` gates direct-beam effects (sun spot, dust
/// motes) — both require a clear line of sight from sun to glass; under
/// any kind of overcast the beam scatters into diffuse light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::tui::pixel_painter) struct AtmoAttenuation {
    pub intensity: f32,
    pub has_direct_beam: bool,
}

pub(in crate::tui::pixel_painter) fn atmo_attenuation(w: Weather) -> AtmoAttenuation {
    match w {
        Weather::Clear => AtmoAttenuation {
            intensity: 1.0,
            has_direct_beam: true,
        },
        Weather::Windy => AtmoAttenuation {
            intensity: 1.0,
            has_direct_beam: true,
        },
        Weather::Snow => AtmoAttenuation {
            intensity: 0.7,
            has_direct_beam: false,
        },
        Weather::Overcast => AtmoAttenuation {
            intensity: 0.45,
            has_direct_beam: false,
        },
        Weather::Rain => AtmoAttenuation {
            intensity: 0.4,
            has_direct_beam: false,
        },
        Weather::Fog => AtmoAttenuation {
            intensity: 0.3,
            has_direct_beam: false,
        },
        Weather::Storm => AtmoAttenuation {
            intensity: 0.25,
            has_direct_beam: false,
        },
        Weather::Smog => AtmoAttenuation {
            intensity: 0.55,
            has_direct_beam: false,
        },
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

    // Atmospheric attenuation makes the sky base + twilight blaze respond
    // to outdoor weather. Storm at noon shouldn't read as full day-blue
    // with rain streaks pasted over it — the sky itself goes dim under
    // heavy weather. `day_eff` is the effective daylight reaching the
    // glass; consumers of `spill_strength` / `darkness` see weather
    // automatically applied without each caller having to multiply.
    let atmo = atmo_attenuation(weather_state(now));
    let day_eff = day * atmo.intensity;
    let twilight_eff = twilight * atmo.intensity;

    let day_a = theme.lighting.day_sky_a;
    let day_b = theme.lighting.day_sky_b;
    let night_a = theme.lighting.night_sky_a;
    let night_b = theme.lighting.night_sky_b;
    let twilight_a = theme.lighting.twilight_a;
    let twilight_b = theme.lighting.twilight_b;

    let glass_a = lerp_rgb(
        lerp_rgb(night_a, day_a, day_eff),
        twilight_a,
        twilight_eff * 0.5,
    );
    let glass_b = lerp_rgb(
        lerp_rgb(night_b, day_b, day_eff),
        twilight_b,
        twilight_eff * 0.5,
    );

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
        spill_strength: day_eff,
        spill_slant: slant,
        darkness: 1.0 - day_eff,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui::pixel_painter) enum WallSide {
    East,
    South,
    West,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::tui::pixel_painter) struct SunSpot {
    pub wall: WallSide,
    /// 0.0..=1.0 along the wall (left→right for South, top→bottom for East/West).
    pub along: f32,
    /// 0.0=dim, 1.0=brightest at noon.
    pub intensity: f32,
    /// 0.0=neutral white (noon), 1.0=very warm gold (sunrise/sunset).
    pub warmth: f32,
}

/// Time-of-day sun position projected onto an office wall. Uses local
/// hour-of-day so the sun's wall (East / West) matches what the rendered
/// wall clock shows; same pattern as `paint_clock` / `sunset_strength` /
/// `time_of_day_look`. Returns `None` outside the extended daylight
/// window 5:30–19:30; the extra 30 minutes on each end carry a fade-in
/// / fade-out ramp so the sun spot doesn't pop on/off at the boundary.
pub(in crate::tui::pixel_painter) fn sun_on_wall(now: SystemTime) -> Option<SunSpot> {
    use chrono::Timelike;
    const SUN_RAMP_HOURS: f32 = 0.5;
    const LOWER: f32 = 6.0 - SUN_RAMP_HOURS;
    const UPPER: f32 = 19.0 + SUN_RAMP_HOURS;
    let unix_now = now.duration_since(std::time::UNIX_EPOCH).ok()?;
    let local = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + unix_now);
    let hour = local.hour() as f32 + local.minute() as f32 / 60.0;
    if !(LOWER..=UPPER).contains(&hour) {
        return None;
    }
    // Wall partition uses the position-hour clamped to [6, 19] so the
    // along/warmth/noon formulas stay in their valid ranges through the
    // boundary fade.
    let position_hour = hour.clamp(6.0, 19.0);
    let (wall, along) = if position_hour < 8.5 {
        (WallSide::East, (position_hour - 6.0) / 2.5)
    } else if position_hour < 16.0 {
        (WallSide::South, (position_hour - 8.5) / 7.5)
    } else {
        (WallSide::West, (position_hour - 16.0) / 3.0)
    };
    let noon_distance = (position_hour - 12.0).abs() / 6.0;
    let boundary_fade = if hour < 6.0 {
        ((hour - LOWER) / SUN_RAMP_HOURS).clamp(0.0, 1.0)
    } else if hour > 19.0 {
        ((UPPER - hour) / SUN_RAMP_HOURS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let intensity = (1.0 - noon_distance * 0.7).clamp(0.0, 1.0) * boundary_fade;
    let warmth = noon_distance.clamp(0.0, 1.0);
    Some(SunSpot {
        wall,
        along,
        intensity,
        warmth,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Build a `SystemTime` that corresponds to local hour `h`, minute `m`
    /// on a fixed date — keeps the tests TZ-independent because
    /// `sun_on_wall` decodes the input back into `chrono::Local`.
    fn at_hour(h: u32, m: u32) -> SystemTime {
        chrono::Local
            .with_ymd_and_hms(2026, 1, 1, h, m, 0)
            .single()
            .expect("local time should be unambiguous")
            .into()
    }

    #[test]
    fn sun_on_wall_east_at_morning() {
        let s = sun_on_wall(at_hour(7, 0)).expect("sun should be up at 07:00");
        assert_eq!(s.wall, WallSide::East);
        assert!(s.warmth > 0.5, "morning sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_overhead_at_noon() {
        let s = sun_on_wall(at_hour(12, 0)).expect("sun should be up at 12:00");
        assert_eq!(s.wall, WallSide::South);
        assert!(
            s.intensity > 0.85,
            "noon sun should be intense: {}",
            s.intensity
        );
    }

    #[test]
    fn sun_on_wall_west_at_evening() {
        let s = sun_on_wall(at_hour(18, 0)).expect("sun should be up at 18:00");
        assert_eq!(s.wall, WallSide::West);
        assert!(s.warmth > 0.6, "evening sun should be warm: {}", s.warmth);
    }

    #[test]
    fn sun_on_wall_none_at_midnight() {
        assert!(sun_on_wall(at_hour(0, 0)).is_none());
    }

    #[test]
    fn atmo_clear_has_direct_beam() {
        let a = atmo_attenuation(Weather::Clear);
        assert!(a.has_direct_beam);
        assert_eq!(a.intensity, 1.0);
        let w = atmo_attenuation(Weather::Windy);
        assert!(w.has_direct_beam);
    }

    #[test]
    fn atmo_cloudy_blocks_direct_beam() {
        for w in [
            Weather::Rain,
            Weather::Storm,
            Weather::Snow,
            Weather::Fog,
            Weather::Overcast,
        ] {
            let a = atmo_attenuation(w);
            assert!(!a.has_direct_beam, "{w:?} should block direct beam");
            assert!(a.intensity < 1.0, "{w:?} should dim diffuse light");
        }
    }

    #[test]
    fn atmo_storm_dimmer_than_overcast() {
        assert!(
            atmo_attenuation(Weather::Storm).intensity
                < atmo_attenuation(Weather::Overcast).intensity
        );
    }

    #[test]
    fn smog_dims_diffusely() {
        let a = atmo_attenuation(Weather::Smog);
        assert!(!a.has_direct_beam);
        assert!(a.intensity > 0.4 && a.intensity < 0.7);
    }

    #[test]
    fn weather_state_emits_every_variant_within_a_week() {
        use std::collections::HashSet;
        use std::time::Duration;
        let start = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut seen: HashSet<Weather> = HashSet::new();
        for slot in 0..(7u64 * 24 * 6) {
            seen.insert(weather_state(start + Duration::from_secs(slot * 600)));
        }
        for w in [
            Weather::Clear,
            Weather::Rain,
            Weather::Storm,
            Weather::Snow,
            Weather::Fog,
            Weather::Overcast,
            Weather::Windy,
            Weather::Smog,
        ] {
            assert!(
                seen.contains(&w),
                "weather_state never emitted {w:?} in a week of slots"
            );
        }
    }
}
