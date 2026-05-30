pub mod claude;
pub mod codex;
pub mod io;
pub mod target;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

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
    let is_tty = std::io::stdin().is_terminal();
    let plan = plan_targets(
        args.target.as_deref(),
        args.config.is_some(),
        &detection(),
        is_tty,
    );
    let targets = resolve_plan(plan)?;
    if needs_confirm(&args.target, targets.len(), args.yes, is_tty)
        && !confirm_targets("install pixtuoid hooks into", &targets)
    {
        println!("aborted");
        return Ok(());
    }
    run_each(&targets, "install", |t| {
        run_install(t, args.config.clone(), args.hook_path.clone())
    })
}

pub fn uninstall(args: UninstallArgs) -> Result<()> {
    let is_tty = std::io::stdin().is_terminal();
    let plan = plan_targets(
        args.target.as_deref(),
        args.config.is_some(),
        &detection(),
        is_tty,
    );
    let targets = resolve_plan(plan)?;
    // Symmetric with install(): the destructive op must also confirm before
    // touching >1 auto-detected target (it rewrites configs + deletes backups).
    if needs_confirm(&args.target, targets.len(), args.yes, is_tty)
        && !confirm_targets("remove pixtuoid hooks from", &targets)
    {
        println!("aborted");
        return Ok(());
    }
    run_each(&targets, "uninstall", |t| {
        run_uninstall(t, args.config.clone())
    })
}

fn resolve_plan(plan: Plan) -> Result<Vec<&'static Target>> {
    match plan {
        Plan::Targets(t) => Ok(t),
        Plan::NothingDetected => {
            println!("no supported CLIs detected; pass --target claude|codex|all");
            Ok(vec![])
        }
        Plan::Conflict(msg) => bail!(msg),
    }
}

/// Confirm only on bare auto-detect (no `--target`) hitting >1 target on a TTY.
/// An explicit `--target` (incl. `all`) is intent and skips the prompt; `--yes` skips.
fn needs_confirm(requested: &Option<String>, n: usize, yes: bool, is_tty: bool) -> bool {
    requested.is_none() && n > 1 && !yes && is_tty
}

fn confirm_targets(verb: &str, targets: &[&'static Target]) -> bool {
    let names: Vec<_> = targets.iter().map(|t| t.display_name).collect();
    confirm(&format!("{verb} {}?", names.join(" + ")))
}

/// Run `op` for each target independently. A failure on one target is reported
/// but does NOT abort the others — otherwise a malformed second config (e.g.
/// `--target all` with bad TOML) could hide that the first target was already
/// modified. Returns Err iff any target failed.
fn run_each(
    targets: &[&'static Target],
    verb: &str,
    op: impl Fn(&'static Target) -> Result<()>,
) -> Result<()> {
    let mut failed = 0usize;
    for &t in targets {
        if let Err(e) = op(t) {
            eprintln!("error: {verb} for {} failed: {e:#}", t.display_name);
            failed += 1;
        }
    }
    if failed > 0 {
        bail!("{failed} of {} target(s) failed", targets.len());
    }
    Ok(())
}

fn run_install(t: &Target, config: Option<PathBuf>, hook_path: Option<PathBuf>) -> Result<()> {
    let path = config.unwrap_or_else(|| (t.default_config_path)());
    let binary = hook_path.map(Ok).unwrap_or_else(io::default_hook_binary)?;
    let hook_cmd = (t.hook_command)(&binary)?;
    let content = io::read_config(&path)?;
    let outcome = (t.merge_install)(&content, &hook_cmd)
        .with_context(|| format!("processing {}", path.display()))?;
    // The PATH check is an install-time environment check, independent of whether
    // the file content changed — always surface it (a no-op re-install on a box
    // where pixtuoid-hook isn't on PATH would otherwise warn nothing).
    if t.needs_path_warning && !io::hook_on_path() {
        println!("warn: `pixtuoid-hook` not found on PATH (checked against this shell).");
        println!("      Install it on PATH, e.g. `cargo install --path crates/pixtuoid-hook`.");
    }
    if !outcome.changed {
        println!("[{}] already up to date — {}", t.name, path.display());
        return Ok(());
    }
    let backup = io::backup_once(&path, BACKUP_SUFFIX)?;
    io::write_config_atomic(&path, &outcome.content)?;
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
    if let Some(note) = t.post_install_note {
        println!("{note}");
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
    let outcome =
        (t.merge_uninstall)(&content).with_context(|| format!("processing {}", path.display()))?;
    if !outcome.changed {
        // SEMANTIC no-op (covers file-absent — content == "" — and no managed
        // entries). Never rewrite the file or delete the backup here: the backup
        // is the user's only recovery path. A byte comparison here would falsely
        // fire on any hand-formatted config and destroy the backup.
        println!(
            "[{}] no pixtuoid hooks found in {} — nothing to remove",
            t.name,
            path.display()
        );
        return Ok(());
    }
    io::write_config_atomic(&path, &outcome.content)?;
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
    use crate::install::target::{MergeOutcome, Target, CLAUDE};

    // A second fake target for "both present" rows (avoids depending on Phase 2's CODEX).
    static FAKE: Target = Target {
        name: "fake",
        display_name: "Fake",
        restart_noun: "Fake",
        default_config_path: || std::path::PathBuf::from("/nonexistent/fake"),
        hook_command: |_| Ok("x".into()),
        merge_install: |c, _| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        merge_uninstall: |c| {
            Ok(MergeOutcome {
                content: c.to_string(),
                changed: false,
            })
        },
        needs_path_warning: false,
        post_install_note: None,
    };

    fn present(claude: bool, fake: bool) -> Vec<(&'static Target, bool)> {
        vec![(&CLAUDE, claude), (&FAKE, fake)]
    }

    #[test]
    fn explicit_target_claude_ignores_detection() {
        let p = plan_targets(Some("claude"), false, &present(false, false), false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn explicit_all_with_config_is_conflict() {
        let p = plan_targets(Some("all"), true, &present(true, true), true);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_tty_returns_detected() {
        let p = plan_targets(None, false, &present(true, true), true);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 2));
    }

    #[test]
    fn no_target_non_tty_single_claude_installs_claude() {
        let p = plan_targets(None, false, &present(true, false), false);
        assert!(matches!(p, Plan::Targets(ref t) if t.len() == 1 && t[0].name == "claude"));
    }

    #[test]
    fn no_target_non_tty_multiple_present_is_conflict() {
        let p = plan_targets(None, false, &present(true, true), false);
        assert!(matches!(p, Plan::Conflict(_)));
    }

    #[test]
    fn no_target_nothing_present_is_nothing_detected() {
        let p = plan_targets(None, false, &present(false, false), false);
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
