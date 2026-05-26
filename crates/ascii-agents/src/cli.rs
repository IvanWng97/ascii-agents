use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ascii-agents",
    version,
    about = "Terminal pixel-art office for AI coding agents"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,

    #[arg(long, global = true, default_value = "info")]
    pub log_level: String,

    /// Color theme: normal, cyberpunk, dracula, tokyo-night, catppuccin, gruvbox.
    #[arg(long, global = true)]
    pub theme: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Run the TUI (default if no subcommand given).
    Run {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        projects_root: Option<PathBuf>,
        #[arg(long)]
        pack_dir: Option<PathBuf>,
        /// Initial desk capacity seed (auto-computed from terminal size).
        #[arg(long, default_value_t = 16, hide = true)]
        max_desks: usize,
        /// Skip the TUI entirely — useful for CI / scripting.
        /// Prints a JSON snapshot of SceneState every 200ms when it changes.
        #[arg(long, default_value_t = false)]
        headless: bool,
    },
    /// Install Claude Code hooks into ~/.claude/settings.json.
    InstallHooks {
        #[arg(long)]
        hook_path: Option<PathBuf>,
        #[arg(long)]
        settings: Option<PathBuf>,
    },
    /// Remove ascii-agents hook entries from settings.json.
    UninstallHooks {
        #[arg(long)]
        settings: Option<PathBuf>,
    },
}

impl Cli {
    pub fn cmd_or_default(self) -> (String, Option<String>, Cmd) {
        let level = self.log_level;
        let theme = self.theme;
        let cmd = self.cmd.unwrap_or(Cmd::Run {
            socket: None,
            projects_root: None,
            pack_dir: None,
            max_desks: 16,
            headless: false,
        });
        (level, theme, cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_or_default_returns_run_when_no_subcommand() {
        let cli = Cli {
            cmd: None,
            log_level: "info".into(),
            theme: None,
        };
        let (level, theme, cmd) = cli.cmd_or_default();
        assert_eq!(level, "info");
        assert!(theme.is_none());
        assert!(matches!(
            cmd,
            Cmd::Run {
                headless: false,
                max_desks: 16,
                ..
            }
        ));
    }

    #[test]
    fn cmd_or_default_preserves_explicit_subcommand() {
        let cli = Cli {
            cmd: Some(Cmd::UninstallHooks { settings: None }),
            log_level: "debug".into(),
            theme: Some("cyberpunk".into()),
        };
        let (level, theme, cmd) = cli.cmd_or_default();
        assert_eq!(level, "debug");
        assert_eq!(theme.as_deref(), Some("cyberpunk"));
        assert!(matches!(cmd, Cmd::UninstallHooks { .. }));
    }
}
