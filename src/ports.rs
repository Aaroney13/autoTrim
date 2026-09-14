//! Listening TCP/UDP ports and who owns them: an app group or an agent
//! session. Dev servers, MCP servers, preview servers, and the like.

use crate::agents::Detection;
use crate::groups::{AppGroup, GroupKind};
use crate::procs::ProcTable;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PortInfo {
    pub port: u16,
    pub protocol: String,
    /// Bound address, `*` when it is unspecified (all interfaces).
    pub addr: String,
    pub pid: u32,
    #[serde(default)]
    pub start_time: u64,
    pub process: String,
    /// The app group or agent session that owns the process.
    pub owner: String,
    #[serde(default)]
    pub exe: Option<String>,
    /// True when the owner is an app bundle, a system process, or an agent
    /// session: something with its own lifecycle. False for a bare local
    /// server started from a terminal or script, the kind that gets forgotten.
    #[serde(default)]
    pub owner_managed: bool,
    /// Memory the owning process holds: footprint on macOS, resident size
    /// elsewhere.
    #[serde(default)]
    pub owner_rss: u64,
    #[serde(default)]
    pub owner_cpu: f32,
    #[serde(default)]
    pub owner_age_secs: u64,
    /// How long the port has been open. The daemon tracks this across
    /// ticks; a one-shot scan uses the owner's age, which is an upper bound.
    #[serde(default)]
    pub open_for_secs: u64,
    /// The owner is a language runtime or dev server: the kind of thing
    /// started from a terminal and forgotten. Auto mode only ever stops
    /// these; a VM manager or database left running is reported, not killed.
    #[serde(default)]
    pub dev_runtime: bool,
    /// What this port is, when it can be said without guessing.
    #[serde(default)]
    pub label: Option<String>,
    /// "known" (a table of familiar ports and owners), "process" (the
    /// owner's command line), or "probe" (an HTTP request to the port).
    #[serde(default)]
    pub label_source: Option<String>,
}

const DEV_RUNTIMES: &[&str] = &[
    "node",
    "bun",
    "deno",
    "python",
    "python3",
    "ruby",
    "php",
    "java",
    "uvicorn",
    "gunicorn",
    "flask",
    "rails",
    "puma",
    "next-server",
    "vite",
    "webpack",
    "esbuild",
    "cargo",
    "dotnet",
    "php-fpm",
    "hugo",
    "jekyll",
    "mkdocs",
    "http-server",
    "serve",
    "live-server",
];

const SYSTEM_PREFIXES: &[&str] = &[
    "/System/",
    "/usr/",
    "/sbin/",
    "/bin/",
    "/Library/Apple/",
    "/private/var/",
    "/opt/homebrew/opt/",
    "/snap/snapd/",
    "C:\\Windows\\",
];

pub fn listening(table: &ProcTable, det: &Detection, groups: &[AppGroup]) -> Vec<PortInfo> {
    let Ok(all) = listeners::get_all() else {
        return Vec::new();
    };
    let mut by_pid: HashMap<u32, &AppGroup> = HashMap::new();
    for g in groups {
        for pid in &g.pids {
            by_pid.insert(*pid, g);
        }
    }
    let mut session_owner: HashMap<u32, String> = HashMap::new();
    for s in &det.sessions {
        let label = format!(
            "{} · {}",
            s.kind.label(),
            s.session_name
                .as_deref()
                .or(s.project.as_deref())
                .unwrap_or("?")
        );
        for pid in &s.pids {
            session_owner.insert(*pid, label.clone());
        }
    }

    let mut seen: BTreeSet<(u16, String, u32)> = BTreeSet::new();
    let mut out = Vec::new();
    for l in all {
        if !matches!(l.state, listeners::SocketState::Listen) {
            continue;
        }
        let protocol = match l.protocol {
            listeners::Protocol::TCP => "tcp",
            listeners::Protocol::UDP => "udp",
        }
        .to_string();
        let pid = l.process.pid;
        let port = l.socket.port();
        if !seen.insert((port, protocol.clone(), pid)) {
            continue;
        }
        let ip = l.socket.ip();
        let addr = if ip.is_unspecified() {
            "*".to_string()
        } else {
            ip.to_string()
        };
        let proc_ = table.get(pid);
        let process = proc_
            .map(|p| p.exe_name())
            .unwrap_or_else(|| l.process.name.clone());
        let exe = proc_
            .and_then(|p| p.exe.as_ref())
            .map(|e| e.to_string_lossy().into_owned());
        let group = by_pid.get(&pid).copied();
        let in_session = session_owner.contains_key(&pid);
        // A pid in an agent group that is not in any session tree is the
        // folded-in app's: name the app, not "Claude Code sessions".
        let owner = session_owner
            .get(&pid)
            .cloned()
            .or_else(|| group.map(|g| g.app.clone().unwrap_or_else(|| g.name.clone())))
            .unwrap_or_else(|| process.clone());
        let system = exe
            .as_deref()
            .map(|e| SYSTEM_PREFIXES.iter().any(|p| e.starts_with(p)))
            .unwrap_or(true);
        let owner_managed = in_session
            || system
            || group
                .map(|g| g.kind == GroupKind::App || g.app.is_some())
                .unwrap_or(false);
        let owner_age_secs = proc_.map(|p| p.run_time).unwrap_or(0);
        let dev_runtime = {
            let n = process.to_ascii_lowercase();
            DEV_RUNTIMES
                .iter()
                .any(|r| n == *r || n.starts_with("python3."))
        };
        out.push(PortInfo {
            port,
            protocol,
            addr,
            pid,
            start_time: proc_.map(|p| p.start_time).unwrap_or(0),
            process,
            owner,
            exe,
            owner_managed,
            owner_rss: proc_.map(|p| p.rss).unwrap_or(0),
            owner_cpu: proc_.map(|p| p.cpu).unwrap_or(0.0),
            owner_age_secs,
            open_for_secs: owner_age_secs,
            dev_runtime,
            label: None,
            label_source: None,
        });
    }
    out.sort_by_key(|p| (p.port, p.protocol.clone(), p.pid));
    out
}
