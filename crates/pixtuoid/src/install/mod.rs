pub mod claude;
pub mod codex;
pub mod io;
pub mod target;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{bail, Result};

use target::{Target, BACKUP_SUFFIX};

pub struct InstallArgs {
    pub hook_path: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub target: Option<String>,
    pub yes: bool,
}

pub struct UninstallArgs {
    pub config: Option<PathBuf>,
    pub target: Option<String>,
    pub yes: bool,
}

pub enum Plan {
    Targets(Vec<&'static Target>),
    NothingDetected,
    Conflict(String),
}

/// Pure policy: decide which targets to act on. No filesystem, no stdin.
/// `present` is the injected detection result; `explicit_config` is whether
/// `--config` was passed (only valid for a single target).
pub fn plan_targets(
    requested: Option<&str>,
    explicit_config: bool,
    present: &[(&'static Target, bool)],
    _yes: bool,
    is_tty: bool,
) -> Plan {
    match requested {
        Some("all") => {
            if explicit_config {
                return Plan::Conflict(
                    "--config applies to a single target; use --target claude|codex".into(),
                );
            }
            let chosen: Vec<_> = present
                .iter()
                .filter(|(_, p)| *p)
                .map(|(t, _)| *t)
                .collect();
            if chosen.is_empty() {
                Plan::NothingDetected
            } else {
                Plan::Targets(chosen)
            }
        }
        Some(name) => match target::by_name(name) {
            Some(t) => Plan::Targets(vec![t]),
            None => Plan::Conflict(format!("unknown target: {name}")),
        },
        None => {
            // `--config`/`--settings` without `--target` is the legacy Claude-only
            // contract (pre-multi-CLI scripts). The supplied path IS the target
            // selection signal — `$HOME` detection is meaningless here — so default
            // to Claude rather than coupling the explicit path to ambient detection.
            if explicit_config {
                return match target::by_name("claude") {
                    Some(t) => Plan::Targets(vec![t]),
                    None => Plan::Conflict("claude target not registered".into()),
                };
            }
            let detected: Vec<_> = present
                .iter()
                .filter(|(_, p)| *p)
                .map(|(t, _)| *t)
                .collect();
            match detected.len() {
                0 => Plan::NothingDetected,
                1 => Plan::Targets(detected), // TTY or not: a single detected target is safe
                _ if is_tty => Plan::Targets(detected), // caller confirms interactively
                _ => {
                    Plan::Conflict("multiple CLIs detected; pass --target claude|codex|all".into())
                }
            }
        }
    }
}

/// Parse a confirm answer: empty/Enter or y/yes → true; anything else → false.
fn parse_confirm(answer: &str) -> bool {
    let a = answer.trim().to_ascii_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    parse_confirm(&line)
}

fn detection() -> Vec<(&'static Target, bool)> {
    target::TARGETS
        .iter()
        .map(|t| (*t, target::is_present(t)))
        .collect()
}

pub fn install(args: InstallArgs) -> Result<()> {
    let present = detection();
    let is_tty = std::io::stdin().is_terminal();
    let plan = plan_targets(
        args.target.as_deref(),
        args.config.is_some(),
        &present,
        args.yes,
        is_tty,
    );
    let targets = resolve_plan(plan, args.target.as_deref())?;
    // Confirm ONLY on bare auto-detect (no --target) resolving to >1 target.
    // An explicit --target (incl. `all`) is treated as intent and skips the prompt.
    if args.target.is_none() && targets.len() > 1 && !args.yes && is_tty {
        let names: Vec<_> = targets.iter().map(|t| t.display_name).collect();
        if !confirm(&format!(
            "install pixtuoid hooks into {}?",
            names.join(" + ")
        )) {
            println!("aborted");
            return Ok(());
        }
    }
    for t in targets {
        run_install(t, args.config.clone(), args.hook_path.clone())?;
    }
    Ok(())
}

pub fn uninstall(args: UninstallArgs) -> Result<()> {
    let present = detection();
    let is_tty = std::io::stdin().is_terminal();
    let plan = plan_targets(
        args.target.as_deref(),
        args.config.is_some(),
        &present,
        args.yes,
        is_tty,
    );
    let targets = resolve_plan(plan, args.target.as_deref())?;
    for t in targets {
        run_uninstall(t, args.config.clone())?;
    }
    Ok(())
}

fn resolve_plan(plan: Plan, _requested: Option<&str>) -> Result<Vec<&'static Target>> {
    match plan {
        Plan::Targets(t) => Ok(t),
        Plan::NothingDetected => {
            println!("no supported CLIs detected; pass --target claude|codex|all");
            Ok(vec![])
        }
        Plan::Conflict(msg) => bail!(msg),
    }
}

fn run_install(t: &Target, config: Option<PathBuf>, hook_path: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let binary = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    let hook_cmd = (t.hook_command)(&binary)?;
    let content = io::read_config(&path)?;
    let merged = (t.merge_install)(&content, &hook_cmd)?;
    if merged == content {
        println!("[{}] already up to date — {}", t.name, path.display());
        return Ok(());
    }
    let backup = io::backup_once(&path, BACKUP_SUFFIX)?;
    io::write_config_atomic(&path, &merged)?;
    println!(
        "ok: installed pixtuoid hooks into {} ({})",
        path.display(),
        t.display_name
    );
    if let Some(b) = backup {
        println!(
            "backup: {} (removed automatically on uninstall-hooks)",
            b.display()
        );
    }
    if t.needs_path_warning && !io::hook_on_path() {
        println!("warn: `pixtuoid-hook` not found on PATH (checked against this shell).");
        println!("      Install it on PATH, e.g. `cargo install --path crates/pixtuoid-hook`.");
    }
    println!(
        "→ start a new {} session for this to take effect.",
        t.restart_noun
    );
    Ok(())
}

fn run_uninstall(t: &Target, config: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let content = io::read_config(&path)?;
    let cleaned = (t.merge_uninstall)(&content)?;
    if cleaned == content {
        // Covers both file-absent (content == "") and no-match. The backup is
        // the user's only recovery path — never destroyed on a no-op.
        println!(
            "[{}] no pixtuoid hooks found in {} — nothing to remove",
            t.name,
            path.display()
        );
        return Ok(());
    }
    io::write_config_atomic(&path, &cleaned)?;
    println!(
        "ok: removed pixtuoid hooks from {} ({})",
        path.display(),
        t.display_name
    );
    if let Some(b) = io::remove_backup(&path, BACKUP_SUFFIX)? {
        println!("removed backup: {}", b.display());
    }
    println!(
        "→ start a new {} session for this to take effect.",
        t.restart_noun
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::target::{Target, CLAUDE};

    // A second fake target for "both present" rows (avoids depending on Phase 2's CODEX).
    static FAKE: Target = Target {
        name: "fake",
        display_name: "Fake",
        restart_noun: "Fake",
        default_config_path: || std::path::PathBuf::from("/nonexistent/fake"),
        hook_command: |_| Ok("x".into()),
        merge_install: |c, _| Ok(c.to_string()),
        merge_uninstall: |c| Ok(c.to_string()),
        needs_path_warning: false,
    };

    fn present(claude: bool, fake: bool) -> Vec<(&'static Target, bool)> {
        vec![(&CLAUDE, claude), (&FAKE, fake)]
    }

    #[test]
    fn explicit_target_claude_ignores_detection() {
        let p = plan_targets(Some("claude"), false, &present(false, false), false, false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn explicit_all_with_config_is_conflict() {
        let p = plan_targets(Some("all"), true, &present(true, true), false, true);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_tty_returns_detected() {
        let p = plan_targets(None, false, &present(true, true), false, true);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 2));
    }

    #[test]
    fn no_target_non_tty_single_claude_installs_claude() {
        let p = plan_targets(None, false, &present(true, false), false, false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn no_target_non_tty_multiple_present_is_conflict() {
        let p = plan_targets(None, false, &present(true, true), false, false);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_nothing_present_is_nothing_detected() {
        let p = plan_targets(None, false, &present(false, false), false, false);
        assert!(matches!(p, Plan::NothingDetected));
    }

    #[test]
    fn confirm_answer_parses_default_yes() {
        assert!(parse_confirm(""));
        assert!(parse_confirm("y"));
        assert!(parse_confirm("YES"));
        assert!(!parse_confirm("n"));
        assert!(!parse_confirm("no"));
        assert!(!parse_confirm("garbage")); // anything not yes/empty → no
    }
}
