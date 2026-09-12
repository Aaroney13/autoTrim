//! autotrim: notice what is holding your memory, and reclaim it safely.

mod agents;
mod browser;
mod daemon;
mod fmt;
mod groups;
mod paths;
mod procs;
mod report;
mod rules;
mod system;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
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
    /// One-shot report: memory holders, agent sessions, browsers, advice.
    Scan(ScanArgs),
    /// Sample continuously, keep history, and report advice as it changes.
    Daemon(DaemonArgs),
    /// Show the daemon's latest snapshot without sampling anything.
    Status(StatusArgs),
}

#[derive(Args, Clone)]
struct ScanArgs {
    /// Emit the snapshot as JSON instead of text.
    #[arg(long)]
    json: bool,
    /// How long to sample CPU before reporting, in milliseconds.
    #[arg(long, default_value_t = 1500)]
    sample_ms: u64,
    /// A quiet session older than this is reported as stale.
    #[arg(long, default_value_t = 6.0)]
    stale_after_hours: f64,
}

impl Default for ScanArgs {
    fn default() -> Self {
        ScanArgs {
            json: false,
            sample_ms: 1500,
            stale_after_hours: 6.0,
        }
    }
}

#[derive(Args, Clone)]
struct DaemonArgs {
    /// Seconds between samples.
    #[arg(long, default_value_t = 30)]
    interval: u64,
    /// Rolling window, in seconds, over which a session's CPU is averaged.
    #[arg(long, default_value_t = 600)]
    window: u64,
    /// Days of history to keep on disk.
    #[arg(long, default_value_t = 7)]
    retention_days: u64,
    /// A quiet session older than this is reported as stale.
    #[arg(long, default_value_t = 6.0)]
    stale_after_hours: f64,
    /// Minutes a session must be observed quiet before it can be called stale.
    #[arg(long, default_value_t = 15)]
    min_quiet_minutes: u64,
    /// Window-mean CPU percent below which a session counts as quiet.
    #[arg(long, default_value_t = 2.0)]
    quiet_cpu: f32,
    /// Take a single sample, write it, and exit.
    #[arg(long)]
    once: bool,
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
    let det = agents::detect(&table, thresholds.stale_after_secs);
    let groups = groups::group(&table, &det);
    let browsers = browser::detect(&table, &groups);
    let advice = rules::evaluate(&system, &groups, &det.sessions, &browsers, thresholds);
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
        advice,
    }
}

fn scan(args: &ScanArgs) -> Result<()> {
    let thresholds = rules::Thresholds {
        stale_after_secs: (args.stale_after_hours * 3600.0) as u64,
        ..Default::default()
    };
    let mut sys = System::new();
    let snap = take_snapshot(
        &mut sys,
        Some(Duration::from_millis(args.sample_ms)),
        &thresholds,
    );
    if args.json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
    } else {
        print!("{}", report::render(&snap));
    }
    Ok(())
}

fn run_daemon(args: &DaemonArgs) -> Result<()> {
    let thresholds = rules::Thresholds {
        stale_after_secs: (args.stale_after_hours * 3600.0) as u64,
        min_quiet_secs: args.min_quiet_minutes * 60,
        quiet_cpu: args.quiet_cpu,
        ..Default::default()
    };
    daemon::run(daemon::DaemonConfig {
        interval: Duration::from_secs(args.interval.max(5)),
        window: Duration::from_secs(args.window.max(args.interval)),
        retention_days: args.retention_days.max(1),
        thresholds,
        once: args.once,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.unwrap_or(Cmd::Scan(ScanArgs::default())) {
        Cmd::Scan(args) => scan(&args),
        Cmd::Daemon(args) => run_daemon(&args),
        Cmd::Status(args) => status(&args),
    }
}
