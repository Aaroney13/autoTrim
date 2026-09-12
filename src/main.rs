//! autotrim: notice what is holding your memory, and reclaim it safely.

mod agents;
mod browser;
mod fmt;
mod groups;
mod procs;
mod report;
mod rules;
mod system;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
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

/// Everything a scan knows. The daemon, the tray, and the MCP server all
/// consume this same shape.
#[derive(Serialize)]
pub struct Snapshot {
    pub taken_at: u64,
    pub scanner_pid: u32,
    pub system: system::SystemInfo,
    pub groups: Vec<groups::AppGroup>,
    pub sessions: Vec<agents::AgentSession>,
    pub browsers: Vec<browser::BrowserInfo>,
    pub advice: Vec<rules::Advice>,
}

fn take_snapshot(sample: Duration, thresholds: &rules::Thresholds) -> Snapshot {
    let mut sys = System::new();
    let table = procs::ProcTable::collect(&mut sys, sample);
    let system = system::collect(&mut sys);
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
    let snap = take_snapshot(Duration::from_millis(args.sample_ms), &thresholds);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
    } else {
        print!("{}", report::render(&snap));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.unwrap_or(Cmd::Scan(ScanArgs::default())) {
        Cmd::Scan(args) => scan(&args),
    }
}
