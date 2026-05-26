use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    pub theme: Option<String>,
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("toml.lock");
    let lock_file = std::fs::File::create(&lock_path)?;
    fs2::FileExt::lock_exclusive(&lock_file)?;

    let mut cfg = if path.exists() {
        load(path)
    } else {
        AppConfig::default()
    };
    cfg.theme = Some(theme_name.to_string());

    let contents = toml::to_string_pretty(&cfg)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &contents)?;
    std::fs::rename(&tmp, path)?;
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
        };
        let theme = resolve_theme(&cfg, Some("dracula".into()));
        assert_eq!(theme, "dracula");
    }

    #[test]
    fn resolve_config_wins_over_default() {
        let cfg = AppConfig {
            theme: Some("gruvbox".into()),
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
}
