//! autotrim: notice what is holding your memory, and reclaim it safely.

mod agents;
mod browser;
mod config;
mod daemon;
mod fmt;
mod groups;
mod notify;
mod openfiles;
mod paths;
mod ports;
mod procs;
mod report;
mod rules;
mod service;
mod system;
mod transcripts;
mod watch;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use config::Config;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::System;

#[derive(Parser)]
#[command(
    name = "autotrim",
    version,
    about = "Notice what is holding your memory, and reclaim it safely."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-shot report: memory holders, agent sessions, browsers, ports, advice.
    Scan(ScanArgs),
    /// Sample continuously, keep history, notify, and log advice as it changes.
    Daemon(DaemonArgs),
    /// Show the daemon's latest snapshot without sampling anything.
    Status(StatusArgs),
    /// Live terminal view of the latest snapshot and the daemon log.
    Watch(WatchArgs),
    /// Print the daemon log.
    Log(LogArgs),
    /// Show the effective settings, or write a commented config file.
    Config {
        #[command(subcommand)]
        action: Option<ConfigCmd>,
    },
    /// Install, remove, restart, or inspect the login service (macOS launchd).
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
}

#[derive(Subcommand, Clone, Copy)]
enum ConfigCmd {
    /// Write config.toml with every setting and its default, commented.
    Init {
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Clone, Copy)]
enum ServiceCmd {
    /// Write the launch agent and start it now and at every login.
    Install,
    /// Stop the daemon and remove the launch agent. Data is kept.
    Uninstall,
    /// Restart the running daemon, for example after rebuilding.
    Restart,
    /// Show whether the launch agent is loaded and running.
    Status,
}

#[derive(Args, Clone, Default)]
struct ScanArgs {
    /// Emit the snapshot as JSON instead of text.
    #[arg(long)]
    json: bool,
    /// How long to sample CPU before reporting, in milliseconds.
    #[arg(long)]
    sample_ms: Option<u64>,
    /// A quiet session idle longer than this is reported as stale.
    #[arg(long)]
    stale_after_hours: Option<f64>,
}

#[derive(Args, Clone)]
struct DaemonArgs {
    /// Seconds between samples.
    #[arg(long)]
    interval: Option<u64>,
    /// Rolling window, in seconds, over which a session's CPU is averaged.
    #[arg(long)]
    window: Option<u64>,
    /// Days of history to keep on disk.
    #[arg(long)]
    retention_days: Option<u64>,
    /// A quiet session idle longer than this is reported as stale.
    #[arg(long)]
    stale_after_hours: Option<f64>,
    /// Minutes a session must be observed quiet before it can be called stale.
    #[arg(long)]
    min_quiet_minutes: Option<u64>,
    /// CPU percent below which a session counts as quiet.
    #[arg(long)]
    quiet_cpu: Option<f32>,
    /// Take a single sample, write it, and exit.
    #[arg(long)]
    once: bool,
    /// Do not send native notifications; only log advice to stdout.
    #[arg(long)]
    no_notify: bool,
    /// Hours before persisting advice is notified again.
    #[arg(long)]
    remind_every_hours: Option<f64>,
}

#[derive(Args, Clone)]
struct WatchArgs {
    /// Seconds between refreshes.
    #[arg(long, default_value_t = 5)]
    interval: u64,
    /// Lines of daemon log to show under the report.
    #[arg(long, default_value_t = 8)]
    log_lines: usize,
}

#[derive(Args, Clone)]
struct LogArgs {
    /// Lines to print from the end of the log.
    #[arg(short = 'n', long, default_value_t = 50)]
    lines: usize,
    /// Keep printing as the daemon writes.
    #[arg(short = 'f', long)]
    follow: bool,
}

#[derive(Args, Clone)]
struct StatusArgs {
    /// Emit the snapshot as JSON instead of text.
    #[arg(long)]
    json: bool,
}

/// Everything a scan knows. The daemon, the tray, and the MCP server all
/// consume this same shape.
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub taken_at: u64,
    pub scanner_pid: u32,
    pub system: system::SystemInfo,
    pub groups: Vec<groups::AppGroup>,
    pub sessions: Vec<agents::AgentSession>,
    pub browsers: Vec<browser::BrowserInfo>,
    #[serde(default)]
    pub ports: Vec<ports::PortInfo>,
    pub advice: Vec<rules::Advice>,
}

/// Observe everything once. `sample` is how a fresh `System` gets a CPU
/// reading; pass `None` when `sys` was refreshed on a previous tick.
pub fn take_snapshot(
    sys: &mut System,
    sample: Option<Duration>,
    thresholds: &rules::Thresholds,
) -> Snapshot {
    let table = procs::ProcTable::collect(sys, sample);
    let system = system::collect(sys);
    let mut det = agents::detect(&table, thresholds.stale_after_secs);
    for s in &mut det.sessions {
        s.state = rules::session_state(s, thresholds);
    }
    let groups = groups::group(&table, &det);
    let browsers = browser::detect(&table, &groups);
    let ports = ports::listening(&table, &det, &groups);
    for s in &mut det.sessions {
        s.ports = ports
            .iter()
            .filter(|p| s.pids.contains(&p.pid))
            .map(|p| p.port)
            .collect();
        s.ports.sort_unstable();
        s.ports.dedup();
    }
    let advice = rules::evaluate(
        &system,
        &groups,
        &det.sessions,
        &browsers,
        &ports,
        thresholds,
    );
    Snapshot {
        taken_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        scanner_pid: std::process::id(),
        system,
        groups,
        sessions: det.sessions,
        browsers,
        ports,
        advice,
    }
}

fn scan(args: &ScanArgs, cfg: &Config) -> Result<()> {
    let mut thresholds = cfg.thresholds();
    if let Some(h) = args.stale_after_hours {
        thresholds.stale_after_secs = (h * 3600.0) as u64;
    }
    let mut sys = System::new();
    let sample = Duration::from_millis(args.sample_ms.unwrap_or(1500));
    let snap = take_snapshot(&mut sys, Some(sample), &thresholds);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
    } else {
        print!("{}", report::render(&snap));
    }
    Ok(())
}

fn run_daemon(args: &DaemonArgs, cfg: &Config) -> Result<()> {
    let mut thresholds = cfg.thresholds();
    if let Some(h) = args.stale_after_hours {
        thresholds.stale_after_secs = (h * 3600.0) as u64;
    }
    if let Some(m) = args.min_quiet_minutes {
        thresholds.min_quiet_secs = m * 60;
    }
    if let Some(q) = args.quiet_cpu {
        thresholds.quiet_cpu = q;
    }
    let interval = args.interval.unwrap_or(cfg.interval_secs).max(5);
    let window = args.window.unwrap_or(cfg.window_secs).max(interval);
    let remind = args.remind_every_hours.unwrap_or(cfg.remind_every_hours);
    daemon::run(daemon::DaemonConfig {
        interval: Duration::from_secs(interval),
        window: Duration::from_secs(window),
        retention_days: args.retention_days.unwrap_or(cfg.retention_days).max(1),
        thresholds,
        once: args.once,
        notify: cfg.notify && !args.no_notify,
        remind_every: Duration::from_secs((remind * 3600.0).max(60.0) as u64),
    })
}

fn status(args: &StatusArgs) -> Result<()> {
    match daemon::latest()? {
        None => {
            println!(
                "no daemon snapshot yet · run `autotrim daemon` (or `autotrim scan` for a one-shot)"
            );
        }
        Some((snap, age)) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            } else {
                println!("latest daemon snapshot · {} ago", fmt::dur(age));
                print!("{}", report::render(&snap));
            }
        }
    }
    Ok(())
}

fn show_config(cfg: &Config, from: Option<&std::path::Path>) -> Result<()> {
    match from {
        Some(p) => println!("# from {}", p.display()),
        None => println!(
            "# defaults (no file at {}; `autotrim config init` writes one)",
            Config::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "?".to_string())
        ),
    }
    print!("{}", toml::to_string_pretty(cfg)?);
    Ok(())
}

fn init_config(force: bool) -> Result<()> {
    let path =
        Config::path().ok_or_else(|| anyhow::anyhow!("no data directory on this platform"))?;
    if path.exists() && !force {
        println!(
            "{} already exists; pass --force to overwrite",
            path.display()
        );
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, Config::template())?;
    println!("wrote {}", path.display());
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (cfg, cfg_path) = Config::load()?;
    match cli.cmd.unwrap_or(Cmd::Scan(ScanArgs::default())) {
        Cmd::Scan(args) => scan(&args, &cfg),
        Cmd::Daemon(args) => run_daemon(&args, &cfg),
        Cmd::Status(args) => status(&args),
        Cmd::Watch(args) => watch::run(
            Duration::from_secs(args.interval.max(1)),
            args.log_lines,
            cfg.thresholds(),
        ),
        Cmd::Log(args) => watch::print_log(args.lines, args.follow),
        Cmd::Config { action } => match action {
            None => show_config(&cfg, cfg_path.as_deref()),
            Some(ConfigCmd::Init { force }) => init_config(force),
        },
        Cmd::Service { action } => service::run(match action {
            ServiceCmd::Install => service::Action::Install,
            ServiceCmd::Uninstall => service::Action::Uninstall,
            ServiceCmd::Restart => service::Action::Restart,
            ServiceCmd::Status => service::Action::Status,
        }),
    }
}
