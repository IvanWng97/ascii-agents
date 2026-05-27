pub mod io;
pub mod merge;

use std::path::PathBuf;

use anyhow::Result;

pub fn install(hook_path: Option<PathBuf>, settings: Option<PathBuf>) -> Result<()> {
    let settings_path = settings.unwrap_or_else(io::default_settings_path);
    // Verify the binary exists, but write just the bare name so settings.json
    // stays portable across machines with different install locations.
    let _hook = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    let hook_str = "pixtuoid-hook".to_string();

    let backup = io::backup_once(&settings_path)?;
    let doc = io::read_settings(&settings_path)?;
    let merged = merge::merge_install(doc, &hook_str);
    io::write_settings_atomic(&settings_path, &merged)?;

    println!(
        "ok: installed pixtuoid hooks into {}",
        settings_path.display()
    );
    if let Some(b) = backup {
        println!("backup: {}", b.display());
    }
    Ok(())
}

pub fn uninstall(settings: Option<PathBuf>) -> Result<()> {
    let settings_path = settings.unwrap_or_else(io::default_settings_path);
    if !settings_path.exists() {
        println!(
            "no settings.json at {} — nothing to do",
            settings_path.display()
        );
        return Ok(());
    }
    let doc = io::read_settings(&settings_path)?;
    let cleaned = merge::merge_uninstall(doc);
    io::write_settings_atomic(&settings_path, &cleaned)?;
    println!(
        "ok: removed pixtuoid hooks from {}",
        settings_path.display()
    );
    Ok(())
}
