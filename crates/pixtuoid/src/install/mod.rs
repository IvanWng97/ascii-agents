pub mod io;
pub mod merge;

use std::path::PathBuf;

use anyhow::Result;

use crate::cli::HookTarget;

pub fn install(
    hook_path: Option<PathBuf>,
    settings: Option<PathBuf>,
    codex_config: Option<PathBuf>,
    target: HookTarget,
) -> Result<()> {
    let hook = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    match target {
        HookTarget::Claude => install_claude(Some(hook), settings)?,
        HookTarget::Codex => install_codex(Some(hook), codex_config)?,
        HookTarget::All => {
            install_claude(Some(hook.clone()), settings)?;
            install_codex(Some(hook), codex_config)?;
        }
    }
    Ok(())
}

fn install_claude(hook_path: Option<PathBuf>, settings: Option<PathBuf>) -> Result<()> {
    let settings_path = settings.unwrap_or_else(io::default_settings_path);
    // Verify the binary exists, but write just the bare name so settings.json
    // stays portable across machines with different install locations.
    let _hook = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    let hook_str = "pixtuoid-hook".to_string();

    let doc = io::read_settings(&settings_path)?;
    let merged = merge::merge_install(doc.clone(), &hook_str);
    let changed = merged != doc;

    if changed {
        // Only snapshot when we're about to mutate the file — a backup of an
        // unchanged file is a silent, pointless side effect.
        let backup = io::backup_once(&settings_path)?;
        io::write_settings_atomic(&settings_path, &merged)?;
        println!(
            "ok: installed pixtuoid hooks into {}",
            settings_path.display()
        );
        if let Some(b) = backup {
            println!(
                "backup: {} (removed automatically on uninstall-hooks)",
                b.display()
            );
        }
    } else {
        println!(
            "already up to date — pixtuoid hooks in {} are current",
            settings_path.display()
        );
    }

    if !io::hook_on_path() {
        // `which` only sees this shell's PATH, not the (often launchd) PATH
        // Claude Code spawns hooks under — so this is a heuristic, not a verdict.
        println!("warn: `pixtuoid-hook` not found on PATH (checked against this shell).");
        println!("      Claude Code spawns hooks via PATH; if it can't find the binary, hooks");
        println!("      won't fire. Install it on PATH, e.g. `cargo install --path crates/pixtuoid-hook`.");
    }
    if changed {
        println!("→ start a new Claude Code session for this to take effect.");
    }
    Ok(())
}

fn install_codex(hook_path: Option<PathBuf>, codex_config: Option<PathBuf>) -> Result<()> {
    let config_path = codex_config.unwrap_or_else(io::default_codex_config_path);
    let hook = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    let hook_str = hook.to_string_lossy().to_string();

    let doc = io::read_toml_config(&config_path)?;
    let merged = merge::merge_codex_install(doc.clone(), &hook_str);
    let changed = merged != doc;

    if changed {
        io::write_toml_atomic(&config_path, &merged)?;
        println!(
            "ok: installed pixtuoid hooks into {}",
            config_path.display()
        );
    } else {
        println!(
            "already up to date — pixtuoid hooks in {} are current",
            config_path.display()
        );
    }

    if changed {
        println!("→ start a new Codex session for this to take effect.");
    }
    Ok(())
}

pub fn uninstall(
    settings: Option<PathBuf>,
    codex_config: Option<PathBuf>,
    target: HookTarget,
) -> Result<()> {
    match target {
        HookTarget::Claude => uninstall_claude(settings)?,
        HookTarget::Codex => uninstall_codex(codex_config)?,
        HookTarget::All => {
            uninstall_claude(settings)?;
            uninstall_codex(codex_config)?;
        }
    }
    Ok(())
}

fn uninstall_claude(settings: Option<PathBuf>) -> Result<()> {
    let settings_path = settings.unwrap_or_else(io::default_settings_path);
    if !settings_path.exists() {
        println!(
            "no settings.json at {} — nothing to do",
            settings_path.display()
        );
        return Ok(());
    }
    let doc = io::read_settings(&settings_path)?;
    let cleaned = merge::merge_uninstall(doc.clone());
    let changed = cleaned != doc;

    if changed {
        io::write_settings_atomic(&settings_path, &cleaned)?;
        println!(
            "ok: removed pixtuoid hooks from {}",
            settings_path.display()
        );
        // Only drop the backup once we've actually reversed our own hooks. On a
        // no-op (nothing matched, or a corrupted sentinel we couldn't match)
        // the backup is the user's only recovery path — never destroy it.
        if let Some(b) = io::remove_backup(&settings_path)? {
            println!("removed backup: {}", b.display());
        }
        println!("→ start a new Claude Code session for this to take effect.");
    } else {
        println!(
            "no pixtuoid hooks found in {} — nothing to remove",
            settings_path.display()
        );
    }
    Ok(())
}

fn uninstall_codex(codex_config: Option<PathBuf>) -> Result<()> {
    let config_path = codex_config.unwrap_or_else(io::default_codex_config_path);
    if !config_path.exists() {
        println!(
            "no config.toml at {} — nothing to do",
            config_path.display()
        );
        return Ok(());
    }
    let doc = io::read_toml_config(&config_path)?;
    let cleaned = merge::merge_codex_uninstall(doc.clone());
    let changed = cleaned != doc;

    if changed {
        io::write_toml_atomic(&config_path, &cleaned)?;
        println!("ok: removed pixtuoid hooks from {}", config_path.display());
        println!("→ start a new Codex session for this to take effect.");
    } else {
        println!(
            "no pixtuoid hooks found in {} — nothing to remove",
            config_path.display()
        );
    }
    Ok(())
}
