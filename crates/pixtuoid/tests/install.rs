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
fn codex_install_then_uninstall_round_trip() {
    let dir = TempDir::new().unwrap();
    let config = dir.path().join("config.toml");

    let bin = env!("CARGO_BIN_EXE_pixtuoid");
    let status = std::process::Command::new(bin)
        .args([
            "install-hooks",
            "--target",
            "codex",
            "--codex-config",
            config.to_str().unwrap(),
            "--hook-path",
            "/fake/pixtuoid-hook",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let contents = std::fs::read_to_string(&config).unwrap();
    let v = toml::from_str::<toml::Value>(&contents).unwrap();
    assert_eq!(v["features"]["hooks"].as_bool(), Some(true));
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap(),
        "/fake/pixtuoid-hook"
    );

    let status = std::process::Command::new(bin)
        .args([
            "uninstall-hooks",
            "--target",
            "codex",
            "--codex-config",
            config.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let contents = std::fs::read_to_string(&config).unwrap();
    let v = toml::from_str::<toml::Value>(&contents).unwrap();
    assert!(v.get("hooks").is_none(), "got {v}");
    assert_eq!(
        v["features"]["hooks"].as_bool(),
        Some(true),
        "uninstall should not rewrite unrelated feature settings"
    );
}
