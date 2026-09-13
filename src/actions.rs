//! The reclaim verbs. Narrow on purpose: close an agent session, stop an
//! unmanaged local server, close a browser tab, ask an app to quit, or
//! restart it so it comes back fresh. Every action is logged with how to
//! undo it.

use crate::agents::{AgentKind, AgentSession, SessionState};
use crate::automation;
use crate::browser::{self, BrowserInfo, TabInfo};
use crate::fmt::{self, stamp_utc};
use crate::groups::{self, AppGroup, GroupKind};
use crate::paths;
use crate::ports::PortInfo;
use crate::rules::Thresholds;
use crate::take_snapshot;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActionRecord {
    pub ts: u64,
    /// manual, auto, or dry-run
    pub mode: String,
    /// close_session, stop_server, close_tab, quit_app, or restart_app
    pub action: String,
    /// The process acted on. Zero for a tab, which is not a process.
    pub pid: u32,
    pub target: String,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub transcript: Option<String>,
    /// How to get it back.
    pub resume: Option<String>,
    /// Memory the target held when acted on: footprint on macOS, resident
    /// size elsewhere.
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

/// The command that opens an app again from its bundle, with any arguments
/// after `--args`.
fn reopen_command(bundle: &Path, args: &[&str]) -> String {
    let mut s = format!("open {}", shell_quote(&bundle.to_string_lossy()));
    if !args.is_empty() {
        s.push_str(" --args");
        for a in args {
            s.push(' ');
            s.push_str(&shell_quote(a));
        }
    }
    s
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

/// Close one browser tab. Matched by id and URL at the moment of closing,
/// so a tab that moved on since the snapshot is left alone.
pub fn close_tab(b: &BrowserInfo, t: &TabInfo, mode: &str, dry_run: bool) -> ActionRecord {
    let profile = b
        .open_profiles
        .iter()
        .find(|p| p.dir == t.profile)
        .map(|p| p.label.clone())
        .unwrap_or_else(|| t.profile.clone());
    let result = if dry_run {
        "dry run, nothing done".to_string()
    } else {
        browser::close_tab(&b.name, t.id, &t.url).unwrap_or_else(|e| format!("failed: {e}"))
    };
    let title = if t.title.trim().is_empty() {
        t.url.clone()
    } else {
        t.title.clone()
    };
    ActionRecord {
        ts: now_epoch(),
        mode: if dry_run {
            "dry-run".to_string()
        } else {
            mode.to_string()
        },
        action: "close_tab".to_string(),
        pid: 0,
        target: format!("{} · {} · {}", b.name, fmt::fit_right(&title, 70), t.site),
        project: None,
        session_id: None,
        transcript: None,
        resume: Some(format!(
            "open -a {} {}   # or ⌘⇧T in the \"{profile}\" window",
            shell_quote(&b.name),
            shell_quote(&t.url)
        )),
        rss: b.per_tab_estimate.unwrap_or(0),
        result,
    }
}

/// Close tabs by id with the checks every interface shares: a fresh
/// snapshot, only tabs the browser's own session file lists, and the URL
/// re-checked by the browser itself. One record per requested id, in the
/// same order; an id the browser no longer lists gets a record saying so,
/// which is not logged because nothing was done.
pub fn close_tabs_by_id(
    browser: &str,
    ids: &[i32],
    t: &Thresholds,
    dry_run: bool,
    mode: &str,
) -> Result<Vec<ActionRecord>> {
    let mut sys = System::new();
    let snap = take_snapshot(&mut sys, Some(Duration::from_millis(300)), t);
    let Some(b) = snap.browsers.iter().find(|b| b.name == browser) else {
        anyhow::bail!("{browser} is not running");
    };
    if !b.can_close_tabs {
        anyhow::bail!("{browser} tabs cannot be closed from here");
    }
    let mut out = Vec::new();
    let mut found = 0;
    for id in ids {
        let Some(tab) = b.tabs.iter().find(|x| x.id == *id) else {
            out.push(ActionRecord {
                ts: now_epoch(),
                mode: mode.to_string(),
                action: "close_tab".to_string(),
                pid: 0,
                target: format!("{browser} · tab {id}"),
                project: None,
                session_id: None,
                transcript: None,
                resume: None,
                rss: 0,
                result: "not open any more".to_string(),
            });
            continue;
        };
        found += 1;
        let rec = close_tab(b, tab, mode, dry_run);
        log(&rec)?;
        out.push(rec);
    }
    if found == 0 && !ids.is_empty() {
        anyhow::bail!(
            "no open tab in {browser} has id {}",
            ids.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(out)
}

/// True once none of `pids` is running, false if any still is after `wait`.
fn wait_gone(pids: &[u32], wait: Duration) -> bool {
    let mut sys = System::new();
    let targets: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    let started = Instant::now();
    loop {
        sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
        if targets.iter().all(|p| sys.process(*p).is_none()) {
            return true;
        }
        if started.elapsed() >= wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Ask an app to quit the way ⌘Q would: it may prompt to save, and it may
/// say no. `root` is the app's main process, for the log and to see whether
/// it went.
pub fn quit_app(g: &AppGroup, root: u32, mode: &str, dry_run: bool) -> ActionRecord {
    let result = if dry_run {
        "dry run, nothing done".to_string()
    } else {
        match automation::quit_app(&g.name) {
            Ok(msg) => {
                if wait_gone(&[root], Duration::from_secs(10)) {
                    "quit".to_string()
                } else {
                    format!("{msg}; still running after 10 s")
                }
            }
            Err(e) => format!("failed: {e}"),
        }
    };
    ActionRecord {
        ts: now_epoch(),
        mode: if dry_run {
            "dry-run".to_string()
        } else {
            mode.to_string()
        },
        action: "quit_app".to_string(),
        pid: root,
        target: g.name.clone(),
        project: None,
        session_id: None,
        transcript: None,
        resume: Some(format!("open -a {}", shell_quote(&g.name))),
        rss: g.rss,
        result,
    }
}

/// How long a restart waits for the app to be gone. Longer than quit's
/// wait: a browser flushes its session state on the way out, and a false
/// "still running" here costs the person a manual reopen.
const RESTART_WAIT: Duration = Duration::from_secs(30);
/// A moment for the app to release its singleton lock before it is opened
/// again.
const RESTART_SETTLE: Duration = Duration::from_secs(1);

/// Quit the way `quit_app` does, wait until every process of the app is
/// gone, then open `bundle` again with `args`. Never opens something that
/// is still running; the record says which way it went and the resume line
/// is always the command that opens it.
pub fn restart_app(
    g: &AppGroup,
    root: u32,
    bundle: &Path,
    args: &[&str],
    mode: &str,
    dry_run: bool,
) -> ActionRecord {
    let reopen = reopen_command(bundle, args);
    let (result, hint) = if dry_run {
        (
            "dry run, nothing done".to_string(),
            "not relaunched; this is what would open it",
        )
    } else {
        match automation::quit_app(&g.name) {
            Err(e) => (
                format!("failed: {e}"),
                "not relaunched; run this once it has quit",
            ),
            Ok(msg) if !wait_gone(&g.pids, RESTART_WAIT) => (
                format!(
                    "{msg}; still running after {} s; not relaunched",
                    RESTART_WAIT.as_secs()
                ),
                "not relaunched; run this once it has quit",
            ),
            Ok(_) => {
                std::thread::sleep(RESTART_SETTLE);
                match automation::relaunch_app(bundle, args) {
                    Ok(_) => (
                        "quit and relaunched".to_string(),
                        "already relaunched; run this if it did not come back",
                    ),
                    Err(e) => (
                        format!("quit, but could not open it again: {e}"),
                        "not relaunched; run this to open it",
                    ),
                }
            }
        }
    };
    ActionRecord {
        ts: now_epoch(),
        mode: if dry_run {
            "dry-run".to_string()
        } else {
            mode.to_string()
        },
        action: "restart_app".to_string(),
        pid: root,
        target: g.name.clone(),
        project: None,
        session_id: None,
        transcript: None,
        resume: Some(format!("{reopen}   # {hint}")),
        rss: g.rss,
        result,
    }
}

/// Apps that are part of the desk, not something to quit.
const NEVER_QUIT: &[&str] = &[
    "Finder",
    "autoTrim",
    "autotrim-tray",
    "Dock",
    "SystemUIServer",
    "loginwindow",
    "WindowServer",
    "Control Center",
    "ControlCenter",
    "Notification Center",
];

/// What the app verbs act on once the shared checks pass.
pub struct AppTarget {
    pub group: AppGroup,
    /// The app's main process: the one whose parent is outside the group.
    pub root: u32,
    /// The bundle the app runs from, when it runs from one.
    pub bundle: Option<PathBuf>,
}

/// The judgement the app verbs share, kept pure so it is tested:
/// `self_chain` is this process and its ancestors, `hosting` how many agent
/// sessions name the app as their host.
fn check_app_target(g: &AppGroup, self_chain: &[u32], hosting: usize, force: bool) -> Result<()> {
    if g.kind != GroupKind::App {
        anyhow::bail!(
            "{} is not an application bundle; only apps can be asked to quit",
            g.name
        );
    }
    // Quitting our own host would cut the branch we are sitting on.
    if self_chain.iter().any(|pid| g.pids.contains(pid)) {
        anyhow::bail!("{} is running this command", g.name);
    }
    if hosting > 0 && !force {
        anyhow::bail!(
            "{} hosts {hosting} agent session{}; close those first, or use force",
            g.name,
            if hosting == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// The checks every app verb makes: never the desk itself (decided before
/// sampling anything), then a fresh snapshot, only application bundles,
/// never the app running this, and never one that hosts agent sessions
/// without `force`. Also finds the app's main process and its bundle.
fn app_target(name: &str, t: &Thresholds, force: bool) -> Result<AppTarget> {
    if NEVER_QUIT.contains(&name) {
        anyhow::bail!("{name} is not something autoTrim will quit");
    }
    let mut sys = System::new();
    let snap = take_snapshot(&mut sys, Some(Duration::from_millis(300)), t);
    let Some(g) = snap.groups.iter().find(|g| g.name == name) else {
        anyhow::bail!("{name} is not running");
    };
    let mut self_chain = Vec::new();
    let mut cur = Some(Pid::from_u32(std::process::id()));
    while let Some(pid) = cur {
        self_chain.push(pid.as_u32());
        if self_chain.len() > 64 {
            break;
        }
        cur = sys.process(pid).and_then(|p| p.parent());
    }
    let hosting = snap
        .sessions
        .iter()
        .filter(|s| s.host_app.as_deref() == Some(name))
        .count();
    check_app_target(g, &self_chain, hosting, force)?;
    let in_group = |pid: Pid| g.pids.contains(&pid.as_u32());
    let root = g
        .pids
        .iter()
        .copied()
        .find(|pid| {
            sys.process(Pid::from_u32(*pid))
                .and_then(|p| p.parent())
                .is_none_or(|pp| !in_group(pp))
        })
        .unwrap_or(g.pids[0]);
    // Helpers live inside the same outer bundle as the main process, so any
    // member's executable names it.
    let bundle = std::iter::once(root)
        .chain(g.pids.iter().copied())
        .filter_map(|pid| sys.process(Pid::from_u32(pid)).and_then(|p| p.exe()))
        .find_map(groups::bundle_path);
    Ok(AppTarget {
        group: g.clone(),
        root,
        bundle,
    })
}

/// Quit an app by its group name with the checks every interface shares:
/// only application bundles, never the desk itself, never the app running
/// this, and never one that hosts agent sessions without `force`.
pub fn quit_app_by_name(
    name: &str,
    t: &Thresholds,
    dry_run: bool,
    force: bool,
    mode: &str,
) -> Result<ActionRecord> {
    let tgt = app_target(name, t, force)?;
    let rec = quit_app(&tgt.group, tgt.root, mode, dry_run);
    log(&rec)?;
    Ok(rec)
}

/// Restart an app by its group name with the checks `quit` makes, plus two
/// of its own: macOS only, and only an app that runs from a bundle, since
/// that is the only thing autoTrim knows how to open again.
pub fn restart_app_by_name(
    name: &str,
    t: &Thresholds,
    dry_run: bool,
    force: bool,
    mode: &str,
) -> Result<ActionRecord> {
    if !automation::AVAILABLE {
        anyhow::bail!("restarting an app is only implemented on macOS so far");
    }
    let tgt = app_target(name, t, force)?;
    let Some(bundle) = tgt.bundle.as_deref() else {
        anyhow::bail!(
            "{name} does not run from an application bundle, so autoTrim cannot open it again; use quit"
        );
    };
    let args = browser::relaunch_args(name);
    let rec = restart_app(&tgt.group, tgt.root, bundle, args, mode, dry_run);
    log(&rec)?;
    Ok(rec)
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

/// Close a session by pid with the safety checks every interface shares:
/// a fresh snapshot, only detected sessions, never the session running
/// this, never an active one without `force`. Logs the action.
pub fn close_by_pid(
    pid: u32,
    t: &Thresholds,
    dry_run: bool,
    force: bool,
    mode: &str,
) -> Result<ActionRecord> {
    let mut sys = System::new();
    let snap = take_snapshot(&mut sys, Some(Duration::from_millis(1000)), t);
    let Some(s) = snap.sessions.iter().find(|s| s.pid == pid) else {
        anyhow::bail!("pid {pid} is not a detected agent session");
    };
    if s.is_self {
        anyhow::bail!("pid {pid} is the session running this command");
    }
    if s.state == SessionState::Active && !force {
        anyhow::bail!(
            "pid {pid} looks active ({:.0}% CPU); use force to close it anyway",
            s.cpu
        );
    }
    let rec = close_session(s, mode, dry_run);
    log(&rec)?;
    Ok(rec)
}

/// Stop a listening process by pid: only unmanaged ones without `force`.
pub fn stop_by_pid(
    pid: u32,
    t: &Thresholds,
    dry_run: bool,
    force: bool,
    mode: &str,
) -> Result<ActionRecord> {
    let mut sys = System::new();
    let snap = take_snapshot(&mut sys, Some(Duration::from_millis(1000)), t);
    let Some(p) = snap.ports.iter().find(|p| p.pid == pid) else {
        anyhow::bail!("pid {pid} is not listening on anything");
    };
    if p.owner_managed && !force {
        anyhow::bail!(
            "pid {pid} ({}) belongs to {}, which manages its own lifecycle; use force to stop it anyway",
            p.process,
            p.owner
        );
    }
    let rec = stop_server(p, mode, dry_run);
    log(&rec)?;
    Ok(rec)
}

/// One line describing a session for a confirmation or a log.
pub fn session_line(s: &AgentSession) -> String {
    format!(
        "{} · {} · {} · {} · {}",
        s.kind.label(),
        s.host,
        s.session_name
            .as_deref()
            .or(s.project.as_deref())
            .unwrap_or("?"),
        match s.idle_secs {
            Some(i) => format!("idle {}", fmt::dur(i)),
            None => format!("age {}", fmt::dur(s.age_secs)),
        },
        fmt::bytes(s.rss)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(kind: GroupKind) -> AppGroup {
        AppGroup {
            name: "Slack".to_string(),
            kind,
            rss: 0,
            cpu: 0.0,
            procs: 1,
            pids: vec![100],
        }
    }

    #[test]
    fn app_target_checks() {
        // Only application bundles.
        assert!(check_app_target(&app(GroupKind::Other), &[7], 0, false).is_err());
        // Never the app this command is running inside.
        assert!(check_app_target(&app(GroupKind::App), &[7, 100, 1], 0, false).is_err());
        // Hosted sessions need force.
        let err = check_app_target(&app(GroupKind::App), &[7], 2, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("hosts 2 agent sessions"), "{err}");
        assert!(check_app_target(&app(GroupKind::App), &[7], 2, true).is_ok());
        assert!(check_app_target(&app(GroupKind::App), &[7], 0, false).is_ok());
    }

    #[test]
    fn reopen_commands() {
        let chrome = Path::new("/Applications/Google Chrome.app");
        assert_eq!(
            reopen_command(chrome, &[]),
            "open '/Applications/Google Chrome.app'"
        );
        assert_eq!(
            reopen_command(chrome, &["--restore-last-session"]),
            "open '/Applications/Google Chrome.app' --args --restore-last-session"
        );
        assert_eq!(
            reopen_command(Path::new("/Applications/Slack.app"), &[]),
            "open /Applications/Slack.app"
        );
    }
}
