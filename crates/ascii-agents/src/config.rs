use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    pub theme: Option<String>,
    /// Optional per-floor desk cap. When set, each floor holds at most
    /// this many desks — excess agents overflow to additional floors.
    /// When absent, capacity is fully auto-computed from terminal size.
    #[serde(rename = "max-desks")]
    pub max_desks: Option<usize>,
    /// Custom sprite pack directory. Supports ~ expansion.
    #[serde(rename = "pack-dir")]
    pub pack_dir: Option<String>,
    #[serde(
        rename = "enabled-pets",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub enabled_pets: Option<Vec<String>>,
}

pub fn resolve_pack_dir(config: &AppConfig, cli_pack_dir: Option<PathBuf>) -> Option<PathBuf> {
    cli_pack_dir.or_else(|| {
        config.pack_dir.as_ref().map(|p| {
            let expanded = if p.starts_with('~') {
                if let Ok(home) = std::env::var("HOME") {
                    p.replacen('~', &home, 1)
                } else {
                    p.clone()
                }
            } else {
                p.clone()
            };
            PathBuf::from(expanded)
        })
    })
}

pub fn config_path() -> PathBuf {
    if let Ok(base) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(base).join("ascii-agents").join("config.toml");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("ascii-agents")
            .join("config.toml");
    }
    PathBuf::from(".config/ascii-agents/config.toml")
}

pub fn load(path: &Path) -> AppConfig {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AppConfig::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), %e, "cannot read config — using defaults");
            return AppConfig::default();
        }
    };
    match toml::from_str(&contents) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!(path = %path.display(), %e, "malformed config — using defaults");
            AppConfig::default()
        }
    }
}

pub fn save(path: &Path, theme_name: &str) -> Result<()> {
    // Resolve symlinks so atomic rename targets the real file,
    // not the symlink itself (critical for stow-managed configs).
    // canonicalize handles relative symlink targets correctly.
    let real_path = if path.is_symlink() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    if let Some(parent) = real_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = real_path.with_extension("toml.lock");
    let lock_file = std::fs::File::create(&lock_path)?;
    fs2::FileExt::try_lock_exclusive(&lock_file)
        .map_err(|e| anyhow::anyhow!("config lock held by another process: {e}"))?;

    let mut cfg = if real_path.exists() {
        load(&real_path)
    } else {
        AppConfig::default()
    };
    cfg.theme = Some(theme_name.to_string());

    let contents = toml::to_string_pretty(&cfg)?;
    let tmp = real_path.with_extension("toml.tmp");
    std::fs::write(&tmp, &contents)?;
    std::fs::rename(&tmp, &real_path)?;
    // Lock released on drop
    Ok(())
}

pub fn resolve_theme(config: &AppConfig, cli_theme: Option<String>) -> String {
    let config_theme = config.theme.as_deref().and_then(|t| {
        if crate::tui::theme::theme_by_name(t).is_some() {
            Some(t.to_string())
        } else {
            tracing::warn!(theme = %t, "unknown theme in config — ignoring");
            None
        }
    });
    cli_theme
        .or(config_theme)
        .unwrap_or_else(|| "normal".to_string())
}

pub fn resolve_pets(config: &AppConfig) -> Vec<crate::tui::pet::PetKind> {
    match &config.enabled_pets {
        None => crate::tui::pet::PetKind::ALL.to_vec(),
        Some(names) => {
            let pets: Vec<_> = names
                .iter()
                .filter_map(|n| {
                    let kind = crate::tui::pet::PetKind::from_config_name(n);
                    if kind.is_none() {
                        tracing::warn!(pet = %n, "unknown pet in config — skipping");
                    }
                    kind
                })
                .collect();
            if pets.is_empty() && !names.is_empty() {
                tracing::warn!("all enabled-pets names were unknown — no pets will appear");
            }
            pets
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_returns_defaults() {
        let cfg = load(Path::new("/nonexistent/path/config.toml"));
        assert!(cfg.theme.is_none());
    }

    #[test]
    fn load_malformed_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not valid { toml }}}").unwrap();
        let cfg = load(&path);
        assert!(cfg.theme.is_none());
    }

    #[test]
    fn load_partial_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
    }

    #[test]
    fn load_ignores_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\nfuture-key = 42\n").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("normal"));
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save(&path, "dracula").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("dracula"));
    }

    #[test]
    fn resolve_cli_wins_over_config() {
        let cfg = AppConfig {
            theme: Some("normal".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, Some("dracula".into()));
        assert_eq!(theme, "dracula");
    }

    #[test]
    fn resolve_config_wins_over_default() {
        let cfg = AppConfig {
            theme: Some("gruvbox".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "gruvbox");
    }

    #[test]
    fn resolve_all_none_uses_default() {
        let cfg = AppConfig::default();
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "normal");
    }

    #[test]
    fn resolve_invalid_config_theme_falls_back_to_default() {
        let cfg = AppConfig {
            theme: Some("does-not-exist".into()),
            ..AppConfig::default()
        };
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "normal");
    }

    #[test]
    fn full_config_flow_file_drives_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        let theme = resolve_theme(&cfg, None);
        assert_eq!(theme, "cyberpunk");
    }

    #[test]
    fn full_config_flow_cli_overrides_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"cyberpunk\"\n").unwrap();
        let cfg = load(&path);
        let theme = resolve_theme(&cfg, Some("dracula".into()));
        assert_eq!(theme, "dracula");
    }

    // --- max-desks cap flow -----------------------------------------------

    #[test]
    fn max_desks_config_set_no_cli() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "max-desks = 8\n").unwrap();
        let cfg = load(&path);
        let cli_max_desks: Option<usize> = None;
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, Some(8));
    }

    #[test]
    fn max_desks_cli_overrides_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "max-desks = 8\n").unwrap();
        let cfg = load(&path);
        let cli_max_desks: Option<usize> = Some(4);
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, Some(4));
    }

    #[test]
    fn max_desks_neither_set() {
        let cfg = AppConfig::default();
        let cli_max_desks: Option<usize> = None;
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, None);
    }

    #[test]
    fn max_desks_no_config_file() {
        let cfg = load(Path::new("/nonexistent/path/config.toml"));
        let cli_max_desks: Option<usize> = None;
        let desk_cap = cli_max_desks.or(cfg.max_desks);
        assert_eq!(desk_cap, None);
    }

    #[test]
    fn save_preserves_max_desks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"normal\"\nmax-desks = 8\n").unwrap();
        save(&path, "cyberpunk").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
        assert_eq!(cfg.max_desks, Some(8));
    }

    // --- pack-dir resolution -----------------------------------------------

    #[test]
    fn pack_dir_cli_wins_over_config() {
        let cfg = AppConfig {
            pack_dir: Some("/config/pack".into()),
            ..AppConfig::default()
        };
        let result = resolve_pack_dir(&cfg, Some(PathBuf::from("/cli/pack")));
        assert_eq!(result, Some(PathBuf::from("/cli/pack")));
    }

    #[test]
    fn pack_dir_config_used_when_no_cli() {
        let cfg = AppConfig {
            pack_dir: Some("/config/pack".into()),
            ..AppConfig::default()
        };
        let result = resolve_pack_dir(&cfg, None);
        assert_eq!(result, Some(PathBuf::from("/config/pack")));
    }

    #[test]
    fn pack_dir_neither_returns_none() {
        let cfg = AppConfig::default();
        let result = resolve_pack_dir(&cfg, None);
        assert_eq!(result, None);
    }

    #[test]
    fn pack_dir_config_expands_tilde() {
        let cfg = AppConfig {
            pack_dir: Some("~/my-pack".into()),
            ..AppConfig::default()
        };
        let result = resolve_pack_dir(&cfg, None);
        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(result, Some(PathBuf::from(format!("{home}/my-pack"))));
        }
    }

    #[test]
    fn pack_dir_loaded_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "pack-dir = \"/custom/sprites\"\n").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.pack_dir.as_deref(), Some("/custom/sprites"));
    }

    // --- enabled-pets -------------------------------------------------------

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
        std::fs::write(
            &path,
            "theme = \"normal\"\nenabled-pets = [\"cat\", \"dog\"]\n",
        )
        .unwrap();
        save(&path, "cyberpunk").unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.theme.as_deref(), Some("cyberpunk"));
        assert_eq!(
            cfg.enabled_pets,
            Some(vec!["cat".to_string(), "dog".to_string()])
        );
    }

    #[test]
    fn enabled_pets_all_unknown_returns_empty() {
        let cfg = AppConfig {
            enabled_pets: Some(vec!["hamster".into(), "parrot".into()]),
            ..AppConfig::default()
        };
        let pets = resolve_pets(&cfg);
        assert!(pets.is_empty());
    }
}
