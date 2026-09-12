//! The reclaim verbs. Narrow on purpose: close an agent session, stop an
//! unmanaged local server. Every action is logged with how to undo it.

use crate::agents::{AgentKind, AgentSession};
use crate::fmt::stamp_utc;
use crate::paths;
use crate::ports::PortInfo;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActionRecord {
    pub ts: u64,
    /// manual, auto, or dry-run
    pub mode: String,
    /// close_session or stop_server
    pub action: String,
    pub pid: u32,
    pub target: String,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub transcript: Option<String>,
    /// How to get it back.
    pub resume: Option<String>,
    /// Resident bytes the target held when acted on.
    pub rss: u64,
    pub result: String,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The command that brings a closed session back, when the agent has one.
pub fn resume_command(s: &AgentSession) -> Option<String> {
    let cd = s
        .cwd
        .as_deref()
        .map(|c| format!("cd {} && ", shell_quote(c)))
        .unwrap_or_default();
    match s.kind {
        AgentKind::ClaudeCode => {
            let id = s.session_id.as_deref()?;
            let hint = if s.host == "Claude app" {
                "   # or reopen it from the Claude app sidebar"
            } else {
                ""
            };
            Some(format!("{cd}claude --resume {id}{hint}"))
        }
        AgentKind::Codex => {
            // rollout-<timestamp>-<uuid>.jsonl: the uuid is the thread id.
            let file = s.transcript.as_deref()?;
            let stem = Path::new(file).file_stem()?.to_str()?;
            let id = stem.get(stem.len().checked_sub(36)?..)?;
            Some(format!("{cd}codex resume {id}"))
        }
        _ => None,
    }
}

/// Terminate a process tree politely, then firmly. Returns what happened.
pub fn terminate_tree(pids: &[u32], wait: Duration) -> String {
    let mut sys = System::new();
    let targets: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
    let Some(root) = targets.first().copied() else {
        return "nothing to do".to_string();
    };
    let Some(p) = sys.process(root) else {
        return "already gone".to_string();
    };
    if p.kill_with(Signal::Term).is_none() {
        // Platform without SIGTERM: fall through to a hard kill.
        p.kill();
    }
    let started = Instant::now();
    let mut outcome = "terminated";
    loop {
        std::thread::sleep(Duration::from_millis(200));
        sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
        if sys.process(root).is_none() {
            break;
        }
        if started.elapsed() >= wait {
            if let Some(p) = sys.process(root) {
                p.kill();
            }
            outcome = "killed after timeout";
            std::thread::sleep(Duration::from_millis(300));
            break;
        }
    }
    // Anything left in the tree goes the same way.
    sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
    for t in targets.iter().skip(1) {
        if let Some(p) = sys.process(*t)
            && p.kill_with(Signal::Term).is_none()
        {
            p.kill();
        }
    }
    outcome.to_string()
}

pub fn close_session(s: &AgentSession, mode: &str, dry_run: bool) -> ActionRecord {
    let resume = resume_command(s);
    let result = if dry_run {
        "dry run, nothing done".to_string()
    } else {
        terminate_tree(&s.pids, Duration::from_secs(10))
    };
    ActionRecord {
        ts: now_epoch(),
        mode: if dry_run {
            "dry-run".to_string()
        } else {
            mode.to_string()
        },
        action: "close_session".to_string(),
        pid: s.pid,
        target: format!(
            "{} · {} · {}",
            s.kind.label(),
            s.host,
            s.session_name
                .as_deref()
                .or(s.project.as_deref())
                .unwrap_or("?")
        ),
        project: s.project.clone(),
        session_id: s.session_id.clone(),
        transcript: s.transcript.clone(),
        resume,
        rss: s.rss,
        result,
    }
}

pub fn stop_server(p: &PortInfo, mode: &str, dry_run: bool) -> ActionRecord {
    let result = if dry_run {
        "dry run, nothing done".to_string()
    } else {
        terminate_tree(&[p.pid], Duration::from_secs(10))
    };
    ActionRecord {
        ts: now_epoch(),
        mode: if dry_run {
            "dry-run".to_string()
        } else {
            mode.to_string()
        },
        action: "stop_server".to_string(),
        pid: p.pid,
        target: format!("{} · {}:{} ({})", p.process, p.addr, p.port, p.protocol),
        project: None,
        session_id: None,
        transcript: None,
        resume: p
            .exe
            .as_ref()
            .map(|e| format!("{e}   # restart it by hand, with whatever arguments it had")),
        rss: p.owner_rss,
        result,
    }
}

pub fn log(rec: &ActionRecord) -> Result<()> {
    let dir = paths::data_dir().context("no data directory")?;
    fs::create_dir_all(&dir)?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("actions.jsonl"))?;
    serde_json::to_writer(&mut f, rec)?;
    f.write_all(b"\n")?;
    Ok(())
}

pub fn read_log(last: usize) -> Result<Vec<ActionRecord>> {
    let dir = paths::data_dir().context("no data directory")?;
    let path = dir.join("actions.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path)?;
    let all: Vec<ActionRecord> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let start = all.len().saturating_sub(last);
    Ok(all[start..].to_vec())
}

pub fn describe(rec: &ActionRecord) -> String {
    let mut s = format!(
        "{} [{}] {} · {} · {}",
        stamp_utc(rec.ts),
        rec.mode,
        rec.action.replace('_', " "),
        rec.target,
        rec.result
    );
    if let Some(r) = &rec.resume {
        s.push_str("\n    resume: ");
        s.push_str(r);
    }
    s
}
