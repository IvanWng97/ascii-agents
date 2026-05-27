mod catppuccin;
mod cyberpunk;
mod dracula;
mod gruvbox;
mod normal;
mod tokyo_night;

use pixtuoid_core::sprite::Rgb;

pub use catppuccin::CATPPUCCIN;
pub use cyberpunk::CYBERPUNK;
pub use dracula::DRACULA;
pub use gruvbox::GRUVBOX;
pub use normal::NORMAL;
pub use tokyo_night::TOKYO_NIGHT;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub surface: SurfaceColors,
    pub office: OfficeColors,
    pub lighting: LightingColors,
    pub furniture: FurnitureColors,
    pub effects: EffectColors,
    pub ui: UiColors,
    pub tool_glow: ToolGlowColors,
}

#[derive(Debug, Clone)]
pub struct SurfaceColors {
    pub wall: Rgb,
    pub wall_trim: Rgb,
    pub baseboard: Rgb,
    pub carpet_base: Rgb,
    pub carpet_light: Rgb,
    pub carpet_dark: Rgb,
    pub window_frame: Rgb,
    pub bg_fallback: Rgb,
}

#[derive(Debug, Clone)]
pub struct OfficeColors {
    pub room_wall_body: Rgb,
    pub room_wall_trim_light: Rgb,
    pub room_wall_trim_dark: Rgb,
    pub cubicle_divider: Rgb,
    pub runner_base: Rgb,
    pub runner_stripe: Rgb,
    pub runner_edge: Rgb,
    pub neon_panel_bg: Rgb,
    pub neon_frame_base: Rgb,
    pub building_dark: Rgb,
    pub building_light: Rgb,
    pub city_lit_windows: [Rgb; 3],
    pub city_dark_window: Rgb,
    pub clock_rim: Rgb,
    pub clock_face: Rgb,
    pub clock_hand: Rgb,
    pub shadow: Rgb,
}

#[derive(Debug, Clone)]
pub struct LightingColors {
    pub day_sky_a: Rgb,
    pub day_sky_b: Rgb,
    pub night_sky_a: Rgb,
    pub night_sky_b: Rgb,
    pub twilight_a: Rgb,
    pub twilight_b: Rgb,
    pub sun_spill: Rgb,
    pub ceiling_pool: Rgb,
    pub floor_lamp_halo: Rgb,
    pub night_tint: Rgb,
}

#[derive(Debug, Clone)]
pub struct FurnitureColors {
    pub wood_top: Rgb,
    pub wood_trim: Rgb,
    pub rug_field: Rgb,
    pub rug_trim: Rgb,
    pub rug_accent: Rgb,
    pub magazine: Rgb,
    pub magazine_trim: Rgb,
    pub chair_seat: Rgb,
    pub chair_trim: Rgb,
    pub coffee_cup: Rgb,
    pub coffee_cup_shadow: Rgb,
    pub desk_plant_light: Rgb,
    pub desk_plant_dark: Rgb,
    pub desk_plant_pot: Rgb,
    pub photo_frame: Rgb,
    pub photo_bg: Rgb,
}

#[derive(Debug, Clone)]
pub struct EffectColors {
    pub monitor_frame_lit: Rgb,
    pub sleep_z: Rgb,
    pub coffee_steam: Rgb,
    pub walking_dust: Rgb,
    pub waiting_bubble: Rgb,
}

#[derive(Debug, Clone)]
pub struct ToolGlowColors {
    pub edit: Rgb,
    pub read: Rgb,
    pub bash: Rgb,
    pub agent: Rgb,
    pub grep: Rgb,
    pub default: Rgb,
}

#[derive(Debug, Clone)]
pub struct UiColors {
    pub label_active: Rgb,
    pub label_waiting: Rgb,
    pub label_idle: Rgb,
    pub label_exiting: Rgb,
    pub tooltip_bg: Rgb,
    pub tooltip_title: Rgb,
    pub tooltip_text: Rgb,
    pub tooltip_dim: Rgb,
    pub neon_brand: Rgb,
    pub neon_star: Rgb,
    pub neon_ticker: Rgb,
}

pub static ALL_THEMES: &[&Theme] = &[
    &NORMAL,
    &CYBERPUNK,
    &DRACULA,
    &TOKYO_NIGHT,
    &CATPPUCCIN,
    &GRUVBOX,
];

pub fn theme_by_name(name: &str) -> Option<&'static Theme> {
    ALL_THEMES.iter().find(|t| t.name == name).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_themes_resolve_by_name() {
        for t in ALL_THEMES {
            assert!(
                theme_by_name(t.name).is_some(),
                "theme '{}' not found",
                t.name
            );
        }
    }

    #[test]
    fn unknown_theme_returns_none() {
        assert!(theme_by_name("doesnotexist").is_none());
    }
}
