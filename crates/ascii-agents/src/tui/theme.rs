use ascii_agents_core::sprite::Rgb;

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
    pub city_lit_window: Rgb,
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
    pub neon_brand: Rgb,
    pub neon_star: Rgb,
    pub neon_ticker: Rgb,
}

pub fn theme_by_name(name: &str) -> Option<&'static Theme> {
    match name {
        "normal" => Some(&NORMAL),
        "cyberpunk" => Some(&CYBERPUNK),
        _ => None,
    }
}

pub static NORMAL: Theme = Theme {
    name: "normal",
    surface: SurfaceColors {
        wall: Rgb(56, 56, 70),
        wall_trim: Rgb(80, 80, 100),
        baseboard: Rgb(40, 40, 52),
        carpet_base: Rgb(150, 110, 72),
        carpet_light: Rgb(178, 138, 96),
        carpet_dark: Rgb(118, 82, 50),
        window_frame: Rgb(24, 24, 32),
        bg_fallback: Rgb(28, 32, 40),
    },
    office: OfficeColors {
        room_wall_body: Rgb(72, 74, 90),
        room_wall_trim_light: Rgb(110, 112, 128),
        room_wall_trim_dark: Rgb(40, 42, 54),
        cubicle_divider: Rgb(72, 82, 104),
        runner_base: Rgb(160, 120, 70),
        runner_stripe: Rgb(140, 100, 55),
        runner_edge: Rgb(90, 60, 35),
        neon_panel_bg: Rgb(12, 14, 22),
        neon_frame_base: Rgb(20, 60, 80),
        building_dark: Rgb(20, 22, 32),
        building_light: Rgb(60, 65, 82),
        city_lit_window: Rgb(252, 215, 110),
        city_dark_window: Rgb(30, 32, 44),
        clock_rim: Rgb(200, 200, 210),
        clock_face: Rgb(240, 240, 240),
        clock_hand: Rgb(20, 20, 25),
        shadow: Rgb(30, 25, 18),
    },
    lighting: LightingColors {
        day_sky_a: Rgb(120, 160, 200),
        day_sky_b: Rgb(160, 190, 220),
        night_sky_a: Rgb(18, 26, 52),
        night_sky_b: Rgb(28, 36, 70),
        twilight_a: Rgb(220, 130, 80),
        twilight_b: Rgb(240, 170, 110),
        sun_spill: Rgb(255, 230, 160),
        ceiling_pool: Rgb(255, 246, 215),
        floor_lamp_halo: Rgb(255, 210, 130),
        night_tint: Rgb(18, 22, 38),
    },
    furniture: FurnitureColors {
        wood_top: Rgb(132, 88, 52),
        wood_trim: Rgb(78, 52, 28),
        rug_field: Rgb(140, 60, 50),
        rug_trim: Rgb(90, 40, 35),
        rug_accent: Rgb(190, 130, 80),
        magazine: Rgb(98, 122, 178),
        magazine_trim: Rgb(50, 60, 92),
        chair_seat: Rgb(96, 68, 44),
        chair_trim: Rgb(60, 40, 22),
        coffee_cup: Rgb(200, 190, 170),
        coffee_cup_shadow: Rgb(180, 160, 130),
        desk_plant_light: Rgb(100, 180, 100),
        desk_plant_dark: Rgb(60, 140, 60),
        desk_plant_pot: Rgb(140, 100, 70),
        photo_frame: Rgb(120, 100, 80),
        photo_bg: Rgb(160, 200, 230),
    },
    effects: EffectColors {
        monitor_frame_lit: Rgb(180, 200, 200),
        sleep_z: Rgb(110, 110, 140),
        coffee_steam: Rgb(190, 190, 210),
        walking_dust: Rgb(150, 120, 85),
        waiting_bubble: Rgb(255, 215, 70),
    },
    tool_glow: ToolGlowColors {
        edit: Rgb(100, 160, 255),
        read: Rgb(80, 220, 240),
        bash: Rgb(240, 170, 80),
        agent: Rgb(200, 140, 255),
        grep: Rgb(180, 220, 120),
        default: Rgb(140, 240, 170),
    },
    ui: UiColors {
        label_active: Rgb(60, 220, 60),
        label_waiting: Rgb(220, 200, 50),
        label_idle: Rgb(140, 140, 140),
        label_exiting: Rgb(80, 80, 80),
        tooltip_bg: Rgb(20, 22, 30),
        neon_brand: Rgb(80, 240, 255),
        neon_star: Rgb(255, 100, 200),
        neon_ticker: Rgb(180, 220, 255),
    },
};

pub static CYBERPUNK: Theme = Theme {
    name: "cyberpunk",
    surface: SurfaceColors {
        wall: Rgb(22, 18, 35),
        wall_trim: Rgb(50, 40, 70),
        baseboard: Rgb(15, 12, 25),
        carpet_base: Rgb(45, 42, 55),
        carpet_light: Rgb(60, 55, 72),
        carpet_dark: Rgb(32, 28, 42),
        window_frame: Rgb(18, 14, 28),
        bg_fallback: Rgb(12, 10, 20),
    },
    office: OfficeColors {
        room_wall_body: Rgb(35, 28, 55),
        room_wall_trim_light: Rgb(70, 55, 95),
        room_wall_trim_dark: Rgb(20, 16, 32),
        cubicle_divider: Rgb(50, 40, 75),
        runner_base: Rgb(40, 35, 55),
        runner_stripe: Rgb(60, 30, 80),
        runner_edge: Rgb(25, 20, 38),
        neon_panel_bg: Rgb(8, 6, 16),
        neon_frame_base: Rgb(80, 20, 60),
        building_dark: Rgb(12, 10, 22),
        building_light: Rgb(30, 25, 50),
        city_lit_window: Rgb(255, 60, 180),
        city_dark_window: Rgb(18, 14, 30),
        clock_rim: Rgb(120, 80, 200),
        clock_face: Rgb(20, 15, 35),
        clock_hand: Rgb(0, 255, 200),
        shadow: Rgb(10, 8, 18),
    },
    lighting: LightingColors {
        day_sky_a: Rgb(40, 20, 80),
        day_sky_b: Rgb(60, 30, 100),
        night_sky_a: Rgb(10, 6, 25),
        night_sky_b: Rgb(20, 12, 45),
        twilight_a: Rgb(180, 40, 120),
        twilight_b: Rgb(220, 60, 160),
        sun_spill: Rgb(200, 100, 255),
        ceiling_pool: Rgb(120, 60, 255),
        floor_lamp_halo: Rgb(0, 200, 255),
        night_tint: Rgb(8, 6, 18),
    },
    furniture: FurnitureColors {
        wood_top: Rgb(50, 45, 65),
        wood_trim: Rgb(30, 25, 42),
        rug_field: Rgb(40, 15, 60),
        rug_trim: Rgb(25, 10, 38),
        rug_accent: Rgb(150, 40, 120),
        magazine: Rgb(60, 180, 255),
        magazine_trim: Rgb(30, 90, 130),
        chair_seat: Rgb(45, 40, 58),
        chair_trim: Rgb(28, 24, 38),
        coffee_cup: Rgb(80, 70, 100),
        coffee_cup_shadow: Rgb(55, 48, 72),
        desk_plant_light: Rgb(0, 255, 140),
        desk_plant_dark: Rgb(0, 180, 100),
        desk_plant_pot: Rgb(60, 50, 80),
        photo_frame: Rgb(70, 50, 100),
        photo_bg: Rgb(255, 60, 180),
    },
    effects: EffectColors {
        monitor_frame_lit: Rgb(100, 60, 200),
        sleep_z: Rgb(0, 200, 255),
        coffee_steam: Rgb(0, 255, 140),
        walking_dust: Rgb(60, 50, 80),
        waiting_bubble: Rgb(255, 60, 180),
    },
    tool_glow: ToolGlowColors {
        edit: Rgb(60, 120, 255),
        read: Rgb(255, 60, 180),
        bash: Rgb(255, 140, 0),
        agent: Rgb(180, 0, 255),
        grep: Rgb(0, 255, 140),
        default: Rgb(0, 255, 200),
    },
    ui: UiColors {
        label_active: Rgb(57, 255, 20),
        label_waiting: Rgb(255, 60, 180),
        label_idle: Rgb(80, 70, 120),
        label_exiting: Rgb(40, 35, 60),
        tooltip_bg: Rgb(10, 8, 20),
        neon_brand: Rgb(255, 0, 200),
        neon_star: Rgb(0, 255, 200),
        neon_ticker: Rgb(120, 60, 255),
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_theme_resolves() {
        assert!(theme_by_name("normal").is_some());
        assert_eq!(theme_by_name("normal").unwrap().name, "normal");
    }

    #[test]
    fn cyberpunk_theme_resolves() {
        assert!(theme_by_name("cyberpunk").is_some());
        assert_eq!(theme_by_name("cyberpunk").unwrap().name, "cyberpunk");
    }

    #[test]
    fn unknown_theme_returns_none() {
        assert!(theme_by_name("doesnotexist").is_none());
    }
}
