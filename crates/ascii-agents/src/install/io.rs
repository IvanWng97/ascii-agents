use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use serde_json::Value;

pub fn default_settings_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(format!("{home}/.claude/settings.json"))
}

pub fn default_hook_binary() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("ASCII_AGENTS_HOOK") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(p) = which::which("ascii-agents-hook") {
        return Ok(p);
    }
    let exe = std::env::current_exe().context("current_exe")?;
    let dir = exe.parent().ok_or_else(|| anyhow!("exe has no parent"))?;
    let candidate = dir.join("ascii-agents-hook");
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(anyhow!(
        "could not locate ascii-agents-hook; pass --hook-path"
    ))
}

pub fn read_settings(path: &Path) -> Result<Value> {
    let target = resolve_symlink(path);
    if !target.exists() {
        return Ok(serde_json::json!({}));
    }
    let mut s = String::new();
    File::open(&target)?.read_to_string(&mut s)?;
    if s.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&s).with_context(|| {
        format!(
            "{} is not valid JSON — refusing to overwrite",
            target.display()
        )
    })
}

/// Atomic write that follows symlinks: writes the temp file beside the *target*
/// of `path` (resolving any symlink), then renames onto the target. This avoids
/// destroying a stow-managed `~/.claude/settings.json` symlink.
pub fn write_settings_atomic(path: &Path, doc: &Value) -> Result<()> {
    let target = resolve_symlink(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = target.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|e| anyhow!("could not lock {}: {e}", lock_path.display()))?;

    let tmp = target.with_extension("json.tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        let serialized = serde_json::to_string_pretty(doc)?;
        f.write_all(serialized.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &target)?;
    fs2::FileExt::unlock(&lock).ok();
    Ok(())
}

pub fn backup_once(path: &Path) -> Result<Option<PathBuf>> {
    let target = resolve_symlink(path);
    if !target.exists() {
        return Ok(None);
    }
    let bak = target.with_extension("json.ascii-agents.bak");
    if bak.exists() {
        return Ok(Some(bak));
    }
    std::fs::copy(&target, &bak)?;
    Ok(Some(bak))
}

/// Follow symlink chain to the final target, even if that target doesn't exist
/// yet (stow creates the link before the dotfiles repo is fully set up).
/// `canonicalize` fails on a dangling symlink, so we walk `read_link` manually.
pub fn resolve_symlink(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    for _ in 0..32 {
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(&cur) {
                Ok(target) => {
                    cur = if target.is_relative() {
                        cur.parent().unwrap_or(Path::new(".")).join(&target)
                    } else {
                        target
                    };
                }
                Err(_) => return cur,
            },
            _ => return cur,
        }
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_symlink_regular_file_returns_as_is() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("plain.json");
        std::fs::write(&file, "{}").unwrap();
        assert_eq!(resolve_symlink(&file), file);
    }

    #[test]
    fn resolve_symlink_nonexistent_returns_as_is() {
        let path = PathBuf::from("/tmp/ascii-agents-test-nonexistent-xyz");
        assert_eq!(resolve_symlink(&path), path);
    }

    #[test]
    fn resolve_symlink_follows_single_hop() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

    #[test]
    fn resolve_symlink_follows_chain() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let mid = dir.path().join("mid.json");
        std::os::unix::fs::symlink(&target, &mid).unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&mid, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

    #[test]
    fn resolve_symlink_dangling_returns_target() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("nonexistent.json");
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(resolve_symlink(&link), target);
    }

    #[test]
    fn resolve_symlink_relative_target() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let target = sub.join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(Path::new("sub/real.json"), &link).unwrap();
        let resolved = resolve_symlink(&link);
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&target).unwrap()
        );
    }

    #[test]
    fn read_settings_missing_file_returns_empty_object() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nope.json");
        let v = read_settings(&path).unwrap();
        assert_eq!(v, serde_json::json!({}));
    }

    #[test]
    fn read_settings_empty_file_returns_empty_object() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.json");
        std::fs::write(&path, "").unwrap();
        let v = read_settings(&path).unwrap();
        assert_eq!(v, serde_json::json!({}));
    }

    #[test]
    fn read_settings_through_symlink() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, r#"{"key":"val"}"#).unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let v = read_settings(&link).unwrap();
        assert_eq!(v["key"], serde_json::json!("val"));
    }

    #[test]
    fn write_settings_atomic_through_symlink_preserves_link() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_settings_atomic(&link, &serde_json::json!({"a": 1})).unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(v["a"], serde_json::json!(1));
    }

    #[test]
    fn backup_once_no_file_returns_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        assert_eq!(backup_once(&path).unwrap(), None);
    }

    #[test]
    fn backup_once_creates_bak_and_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"v":1}"#).unwrap();
        let bak = backup_once(&path).unwrap().unwrap();
        assert!(bak.exists());
        let bak2 = backup_once(&path).unwrap().unwrap();
        assert_eq!(bak, bak2);
    }
}
