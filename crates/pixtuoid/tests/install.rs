use tempfile::TempDir;

#[test]
fn install_then_uninstall_round_trip() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");

    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--settings",
            settings.to_str().unwrap(),
            "--hook-path",
            "/fake/path",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let contents = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert!(v["hooks"]["PreToolUse"][0]["_pixtuoid"].as_bool().unwrap());

    let status = std::process::Command::new(bin)
        .args(["uninstall-hooks", "--settings", settings.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let contents = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&contents).unwrap();
    assert!(v.get("hooks").is_none(), "got {v}");
}

#[test]
fn install_with_config_and_target_flags() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target",
            "claude",
            "--config",
            settings.to_str().unwrap(),
            "--hook-path",
            "/fake/path",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert!(v["hooks"]["PreToolUse"][0]["_pixtuoid"].as_bool().unwrap());
}

#[test]
fn install_codex_writes_toml_with_sentinel_and_backup() {
    let dir = TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, "model = \"o1\"\n").unwrap(); // pre-existing user content
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
            "--hook-path",
            "/fake/pixtuoid-hook",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["model"].as_str().unwrap(), "o1", "user content preserved");
    assert!(v["hooks"]["PreToolUse"][0]["hooks"][0]["_pixtuoid"]
        .as_bool()
        .unwrap());
    assert!(v.get("features").is_none(), "no [features] hooks = true");
    // backup created with the correct multi-dot name
    assert!(dir.path().join("config.toml.pixtuoid.bak").exists());

    // uninstall restores + removes backup
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let v: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(v.get("hooks").is_none());
    assert_eq!(v["model"].as_str().unwrap(), "o1");
    assert!(!dir.path().join("config.toml.pixtuoid.bak").exists());
}

// Regression guard for the byte-vs-semantic no-op bug: uninstall on a config
// that has user content but NO pixtuoid hooks must NOT rewrite the file and must
// NOT delete the user's backup (the only recovery path).
#[test]
fn uninstall_noop_preserves_file_and_backup() {
    let dir = TempDir::new().unwrap();
    let cfg = dir.path().join("config.toml");
    // Hand-formatted user config with a comment + a non-pixtuoid hook, no managed entries.
    let original = "# my codex config\nmodel = \"o1\"\n\n[[hooks.PreToolUse]]\nmatcher = \"*\"\n\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"/usr/bin/mytool\"\n";
    std::fs::write(&cfg, original).unwrap();
    // A backup the user must never lose on a no-op uninstall.
    let bak = dir.path().join("config.toml.pixtuoid.bak");
    std::fs::write(&bak, "sentinel-backup").unwrap();

    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "codex",
            "--config",
            cfg.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    assert_eq!(
        std::fs::read_to_string(&cfg).unwrap(),
        original,
        "no-op uninstall must not rewrite/reformat the file"
    );
    assert!(bak.exists(), "no-op uninstall must NOT delete the backup");
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), "sentinel-backup");
}

// Regression guard: uninstall on a missing Claude settings.json must not CREATE
// the file (the old code's early-return-on-missing behavior, preserved via the
// semantic no-op).
#[test]
fn uninstall_missing_file_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let settings = dir.path().join("settings.json");
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "claude",
            "--config",
            settings.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        !settings.exists(),
        "uninstall must not create a missing config"
    );
}

#[test]
fn install_unknown_target_errors() {
    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args(["install-hooks", "--target", "bogus"])
        .status()
        .unwrap();
    // clap rejects an invalid ValueEnum value → non-zero exit.
    assert!(!status.success());
}
