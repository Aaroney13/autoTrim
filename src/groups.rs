//! Roll processes up into the thing a person would actually quit: an app
//! bundle, an agent kind, or a bare executable.

use crate::agents::{AgentKind, Detection};
use crate::procs::{Proc, ProcTable};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    App,
    Agent,
    Other,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppGroup {
    pub name: String,
    pub kind: GroupKind,
    pub rss: u64,
    /// CPU percent summed over the group's processes.
    #[serde(default)]
    pub cpu: f32,
    pub procs: usize,
    pub pids: Vec<u32>,
}

/// Terminal emulators do not absorb their children: a dev server started
/// from a shell is its own thing, not "Terminal".
const TERMINALS: &[&str] = &[
    "Terminal",
    "iTerm2",
    "iTerm",
    "Warp",
    "Ghostty",
    "Alacritty",
    "kitty",
    "WezTerm",
    "Hyper",
];

/// Name of the outermost `.app` bundle in the executable path, if any.
pub fn bundle_name(p: &Proc) -> Option<String> {
    let exe = p.exe.as_ref()?;
    for comp in exe.components() {
        let s = comp.as_os_str().to_string_lossy();
        if let Some(stem) = s.strip_suffix(".app") {
            return Some(stem.to_string());
        }
    }
    None
}

fn exe_stem(p: &Proc) -> String {
    let n = p.exe_name();
    if n.is_empty() { p.name.clone() } else { n }
}

fn owner_of(table: &ProcTable, p: &Proc) -> (String, GroupKind) {
    if let Some(b) = bundle_name(p) {
        return (b, GroupKind::App);
    }
    for a in table.ancestors(p.pid) {
        if let Some(b) = bundle_name(a) {
            if TERMINALS.contains(&b.as_str()) {
                break;
            }
            return (b, GroupKind::App);
        }
    }
    (exe_stem(p), GroupKind::Other)
}

pub fn group(table: &ProcTable, det: &Detection) -> Vec<AppGroup> {
    let mut map: HashMap<String, AppGroup> = HashMap::new();
    for p in &table.procs {
        if det.claimed.contains(&p.pid) {
            continue;
        }
        let (name, kind) = owner_of(table, p);
        let g = map.entry(name.clone()).or_insert_with(|| AppGroup {
            name,
            kind,
            rss: 0,
            cpu: 0.0,
            procs: 0,
            pids: Vec::new(),
        });
        g.rss += p.rss;
        g.cpu += p.cpu;
        g.procs += 1;
        g.pids.push(p.pid);
    }

    let mut by_kind: HashMap<AgentKind, AppGroup> = HashMap::new();
    for s in &det.sessions {
        let g = by_kind.entry(s.kind).or_insert_with(|| AppGroup {
            name: format!("{} sessions", s.kind.label()),
            kind: GroupKind::Agent,
            rss: 0,
            cpu: 0.0,
            procs: 0,
            pids: Vec::new(),
        });
        g.rss += s.rss;
        g.cpu += s.cpu;
        g.procs += 1;
        g.pids.push(s.pid);
    }

    let mut out: Vec<AppGroup> = map.into_values().chain(by_kind.into_values()).collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.rss));
    out
}
