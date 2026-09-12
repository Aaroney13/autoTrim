//! autotrim: notice what is holding your memory, and reclaim it safely.
//!
//! The daemon, the command line, the tray, and any future interface share
//! this crate. One `Snapshot` feeds all of them.

pub mod actions;
pub mod agents;
pub mod automation;
pub mod browser;
pub mod config;
pub mod daemon;
pub mod fmt;
pub mod groups;
pub mod notify;
pub mod openfiles;
pub mod paths;
pub mod ports;
pub mod procs;
pub mod report;
pub mod rules;
pub mod service;
pub mod snss;
pub mod system;
pub mod transcripts;
pub mod trends;
pub mod watch;

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::System;

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
    /// Growth and sustained-CPU readings from the daemon's rolling series.
    /// Empty for a one-shot scan.
    #[serde(default)]
    pub trends: Vec<trends::Trend>,
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
    let browsers = browser::detect(&table, &groups, thresholds.tab_stale_after_secs);
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
        &[],
        None,
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
        trends: Vec::new(),
        advice,
    }
}

/// A fresh one-shot snapshot with its own system handle. For interfaces
/// that only occasionally need to look for themselves.
pub fn scan_now(thresholds: &rules::Thresholds, sample: Duration) -> Snapshot {
    take_snapshot(&mut System::new(), Some(sample), thresholds)
}
