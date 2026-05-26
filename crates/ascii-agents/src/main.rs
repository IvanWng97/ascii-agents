use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use ascii_agents::cli::{Cli, Cmd};
use ascii_agents::{config, install, runtime};
use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    install_crash_hook();
    let (log_level, cli_theme, cmd) = Cli::parse().cmd_or_default();
    let make_filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&log_level));

    // Log routing:
    //   TUI mode: silent by default (no file, no stderr). Only logs when
    //     $ASCII_AGENTS_LOG is set or --log-level is debug/trace.
    //     Crash reporting is handled separately by the panic hook.
    //   Non-TUI (install-hooks, uninstall-hooks, --headless): stderr.
    let tui_active = matches!(&cmd, Cmd::Run { headless, .. } if !*headless);
    let wants_verbose = matches!(log_level.as_str(), "debug" | "trace");
    let explicit_log_file = std::env::var("ASCII_AGENTS_LOG").is_ok();

    if tui_active && (wants_verbose || explicit_log_file) {
        if let Ok(path) = log_file_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                let writer = Arc::new(Mutex::new(f));
                tracing_subscriber::fmt()
                    .with_env_filter(make_filter())
                    .with_ansi(false)
                    .with_writer(move || MutexFileWriter(writer.clone()))
                    .init();
            }
        }
    } else if !tui_active {
        tracing_subscriber::fmt()
            .with_env_filter(make_filter())
            .with_writer(std::io::stderr)
            .init();
    }

    match cmd {
        Cmd::Run {
            socket,
            projects_root,
            pack_dir,
            max_desks,
            headless,
        } => {
            let cfg_path = config::config_path();
            let cfg = config::load(&cfg_path);
            let theme_name = config::resolve_theme(&cfg, cli_theme);
            runtime::run(
                socket,
                projects_root,
                pack_dir,
                max_desks,
                headless,
                theme_name,
                cfg_path,
            )
        }
        Cmd::InstallHooks {
            hook_path,
            settings,
        } => install::install(hook_path, settings),
        Cmd::UninstallHooks { settings } => install::uninstall(settings),
    }
}

fn install_crash_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stderr(),
            crossterm::event::DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen
        );

        let crash_path = crash_log_path();
        let mut report = String::new();
        report.push_str(&format!(
            "ascii-agents crashed at {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        report.push_str(&format!("{info}\n"));
        let bt = std::backtrace::Backtrace::force_capture();
        report.push_str(&format!("{bt}\n"));

        if let Some(parent) = crash_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_path)
        {
            use std::io::Write;
            let _ = f.write_all(report.as_bytes());
        }

        eprintln!(
            "\nascii-agents crashed. Report saved to: {}",
            crash_path.display()
        );
        eprintln!("Please report at: https://github.com/IvanWng97/ascii-agents/issues\n");
        default_hook(info);
    }));
}

fn crash_log_path() -> PathBuf {
    if let Ok(state) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(format!("{state}/ascii-agents/crash.log"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(format!("{home}/.cache/ascii-agents/crash.log"));
    }
    PathBuf::from("/tmp/ascii-agents-crash.log")
}

fn log_file_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("ASCII_AGENTS_LOG") {
        return Ok(PathBuf::from(p));
    }
    if let Ok(state) = std::env::var("XDG_STATE_HOME") {
        return Ok(PathBuf::from(format!("{state}/ascii-agents/log")));
    }
    let home = std::env::var("HOME")?;
    Ok(PathBuf::from(format!("{home}/.cache/ascii-agents/log")))
}

/// Adapter that gives `tracing-subscriber` a `Write`-able file behind a Mutex.
struct MutexFileWriter(Arc<Mutex<std::fs::File>>);

impl std::io::Write for MutexFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .flush()
    }
}
