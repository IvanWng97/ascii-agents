pub fn is_newer_version(current: &str, last_seen: &str) -> bool {
    parse_semver(current)
        .zip(parse_semver(last_seen))
        .is_some_and(|(c, l)| c > l)
}

/// Parses `major.minor.patch[-prerelease]` into a tuple where the 4th
/// component is `0` for a prerelease and `1` for a release, so that
/// `0.5.0-rc1 < 0.5.0` per semver precedence rules.
fn parse_semver(v: &str) -> Option<(u64, u64, u64, u8)> {
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_str = parts.next().unwrap_or("0");
    let (patch_num, is_release) = match patch_str.split_once('-') {
        Some((num, _prerelease)) => (num.parse().ok()?, 0u8),
        None => (patch_str.parse().ok()?, 1u8),
    };
    Some((major, minor, patch_num, is_release))
}

pub fn release_notes(version: &str) -> Option<&'static [&'static str]> {
    match version {
        "0.4.0" => Some(&[
            "Renamed from ascii-agents to pixtuoid",
            "Run `pixtuoid install-hooks` to update hooks",
            "New env vars: PIXTUOID_SOCKET/HOOK/LOG",
            "Flaky startup test fixed + 250ms rescan",
        ]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_detected() {
        assert!(is_newer_version("0.2.0", "0.1.0"));
    }

    #[test]
    fn same_version_not_newer() {
        assert!(!is_newer_version("0.1.0", "0.1.0"));
    }

    #[test]
    fn older_not_newer() {
        assert!(!is_newer_version("0.1.0", "0.2.0"));
    }

    #[test]
    fn major_bump_detected() {
        assert!(is_newer_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn minor_bump_detected() {
        assert!(is_newer_version("0.5.0", "0.4.0"));
    }

    #[test]
    fn patch_bump_detected() {
        assert!(is_newer_version("0.4.1", "0.4.0"));
    }

    #[test]
    fn bad_input_safe() {
        assert!(!is_newer_version("not-semver", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "garbage"));
        assert!(!is_newer_version("", ""));
    }

    #[test]
    fn prerelease_newer_than_older_release() {
        assert!(is_newer_version("0.5.0-alpha", "0.4.0"));
    }

    #[test]
    fn release_newer_than_prerelease_of_same_version() {
        assert!(is_newer_version("0.5.0", "0.5.0-rc1"));
        assert!(!is_newer_version("0.5.0-rc1", "0.5.0"));
    }

    #[test]
    fn release_notes_known_version() {
        assert!(release_notes("0.4.0").is_some());
    }

    #[test]
    fn release_notes_unknown_version() {
        assert!(release_notes("9.9.9").is_none());
    }

    /// Guards against a silent regression: bumping `Cargo.toml` without
    /// adding a matching `release_notes` arm would make the popup
    /// permanently invisible for the new release. This test fails fast.
    #[test]
    fn current_version_has_release_notes() {
        let current = env!("CARGO_PKG_VERSION");
        assert!(
            release_notes(current).is_some(),
            "release_notes({current:?}) returned None — add an arm for the current version"
        );
    }
}
