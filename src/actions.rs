//! The reclaim verbs. Narrow on purpose: close an agent session, stop an
//! unmanaged local server, close a browser tab, ask an app to quit, or
//! restart it so it comes back fresh. Every action is logged with how to
//! reopen or resume it when that is possible.

use crate::agents::{AgentKind, AgentSession, SessionState};
use crate::automation;
use crate::browser::{self, BrowserInfo, TabInfo};
use crate::fmt::{self, stamp_utc};
use crate::groups::{self, AppGroup, GroupKind};
use crate::paths;
use crate::ports::PortInfo;
use crate::rules::Thresholds;
use crate::system::MemorySample;
use crate::take_snapshot;
use crate::termination::{self, ProcessIdentity, TerminationOutcome};
use crate::transcripts;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessesToUpdate, System};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActionRecord {
    /// Stable identifier shared by intent and completion; absent in legacy logs.
    #[serde(default)]
    pub id: Option<String>,
    /// intent, success, failure, partial, skipped, dry_run, or legacy.
    #[serde(default = "legacy_status")]
    pub status: String,
    #[serde(default)]
    pub identities: Vec<ProcessIdentity>,
    #[serde(default)]
    pub target_identity: Option<serde_json::Value>,
    #[serde(default)]
    pub termination: Option<TerminationOutcome>,
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
    /// Whole-machine change, separate from the target's footprint/estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_observation: Option<MemoryObservation>,
    pub result: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MemoryObservation {
    pub before: MemorySample,
    pub after: MemorySample,
    /// A tab batch has one observation, attached to its last attempted action.
    pub tab_batch: bool,
    pub attempted_actions: usize,
}

/// Record a batch once, including failed/partial attempts. A skipped target or
/// preview has no measured effect. Persisted completion always precedes this
/// optional observation so a crash while sampling never hides an action result.
fn observe_actions(
    records: &mut [ActionRecord],
    before: Option<MemorySample>,
    tab_batch: bool,
    sample_after: impl FnOnce() -> Option<MemorySample>,
    mut persist: impl FnMut(&ActionRecord) -> Result<()>,
) -> Result<()> {
    let Some(before) = before else { return Ok(()) };
    let attempted: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            r.mode != "dry-run" && matches!(r.status.as_str(), "success" | "partial" | "failure")
        })
        .map(|(i, _)| i)
        .collect();
    let Some(&last) = attempted.last() else {
        return Ok(());
    };
    let Some(after) = sample_after() else {
        return Ok(());
    };
    records[last].memory_observation = Some(MemoryObservation {
        before,
        after,
        tab_batch,
        attempted_actions: attempted.len(),
    });
    persist(&records[last]).context("action completed, but memory observation could not be saved")
}

fn settled_memory() -> Option<MemorySample> {
    std::thread::sleep(Duration::from_secs(1));
    MemorySample::collect()
}

fn legacy_status() -> String {
    "legacy".into()
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
            let id = s
                .session_id
                .clone()
                .or_else(|| transcripts::rollout_uuid(Path::new(s.transcript.as_deref()?)))?;
            let hint = if s.engine && s.host != "terminal" {
                format!("   # or reopen the thread in the {}", s.host)
            } else {
                String::new()
            };
            Some(format!("{cd}codex resume {id}{hint}"))
        }
        AgentKind::Copilot => {
            let id = s.session_id.as_deref()?;
            Some(format!("{cd}copilot --resume={id}"))
        }
        AgentKind::CursorAgent => {
            let id = s.session_id.as_deref()?;
            Some(format!("{cd}agent --resume {id}"))
        }
        _ => None,
    }
}

fn close_session(s: &AgentSession, mode: &str, dry_run: bool) -> ActionRecord {
    let resume = resume_command(s);
    let result = if dry_run {
        "dry run, nothing done".to_string()
    } else {
        "intent; execution not started".into()
    };
    ActionRecord {
        id: None,
        status: "legacy".into(),
        identities: Vec::new(),
        target_identity: None,
        termination: None,
        memory_observation: None,
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

fn stop_server(p: &PortInfo, mode: &str, dry_run: bool) -> ActionRecord {
    let result = if dry_run {
        "dry run, nothing done".to_string()
    } else {
        "intent; execution not started".into()
    };
    ActionRecord {
        id: None,
        status: "legacy".into(),
        identities: Vec::new(),
        target_identity: None,
        termination: None,
        memory_observation: None,
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
fn close_tab(
    b: &BrowserInfo,
    t: &TabInfo,
    mode: &str,
    dry_run: bool,
    auto: Option<&crate::daemon::AutoConfig>,
) -> ActionRecord {
    let profile = b
        .open_profiles
        .iter()
        .find(|p| p.dir == t.profile)
        .map(|p| p.label.clone())
        .unwrap_or_else(|| t.profile.clone());
    let eligible = auto.map_or_else(
        || browser::can_auto_close_tab(b, t),
        |cfg| cfg.tab_reason(b, t, now_epoch()).is_some(),
    );
    let result = if mode == "auto" && !eligible {
        "skipped: tab is no longer eligible for automatic cleanup".to_string()
    } else if dry_run {
        "dry run, nothing done".to_string()
    } else {
        browser::close_tab(&b.name, t.id, &t.url, mode == "auto")
            .unwrap_or_else(|e| format!("failed: {e}"))
    };
    let title = if t.title.trim().is_empty() {
        t.url.clone()
    } else {
        t.title.clone()
    };
    ActionRecord {
        id: None,
        status: "legacy".into(),
        identities: Vec::new(),
        target_identity: None,
        termination: None,
        memory_observation: None,
        ts: now_epoch(),
        mode: if dry_run {
            "dry-run".to_string()
        } else {
            mode.to_string()
        },
        action: "close_tab".to_string(),
        pid: 0,
        target: format!(
            "{} · {} · {}{}",
            b.name,
            fmt::fit_right(&title, 70),
            t.site,
            auto.and_then(|cfg| cfg.tab_reason(b, t, now_epoch()))
                .and_then(|reason| match reason {
                    crate::tab_rules::AutoTabReason::Domain { domain } =>
                        Some(format!(" · auto-close domain {domain}")),
                    _ => None,
                })
                .unwrap_or_default()
        ),
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
/// Auto mode must also supply the tabs it warned about, including their
/// last-active times, so activity during the final rescan cancels a close.
pub fn close_tabs_by_id(
    browser: &str,
    ids: &[i32],
    t: &Thresholds,
    dry_run: bool,
    mode: &str,
    warned: Option<&[&TabInfo]>,
) -> Result<Vec<ActionRecord>> {
    close_tabs_checked(
        browser,
        ids,
        None,
        t,
        dry_run,
        mode,
        TabClosePolicy {
            warned,
            ..Default::default()
        },
    )
}

/// The identity of a tab shown in a persistent confirmation dialog.
#[derive(Deserialize, Clone, Debug)]
pub struct ReviewedTab {
    pub id: i32,
    pub url: String,
    pub profile: String,
}

pub fn close_reviewed_tabs(
    browser: &str,
    reviewed: &[ReviewedTab],
    t: &Thresholds,
) -> Result<Vec<ActionRecord>> {
    let ids: Vec<_> = reviewed.iter().map(|tab| tab.id).collect();
    close_tabs_checked(
        browser,
        &ids,
        Some(reviewed),
        t,
        false,
        "manual",
        TabClosePolicy::default(),
    )
}

fn reviewed_tab_error(tab: &TabInfo, expected: Option<&ReviewedTab>) -> Option<&'static str> {
    let Some(expected) = expected else {
        return Some("tab was not part of the review");
    };
    if tab.id != expected.id || tab.url != expected.url || tab.profile != expected.profile {
        return Some("tab changed since the review; review it again");
    }
    if tab.pinned || tab.active {
        return Some("tab is now pinned or active");
    }
    None
}

fn warned_tab_error(tab: &TabInfo, expected: Option<&TabInfo>) -> Option<&'static str> {
    let Some(expected) = expected else {
        return Some("tab was not warned about");
    };
    if tab.id != expected.id
        || tab.profile != expected.profile
        || tab.window_id != expected.window_id
        || tab.url != expected.url
        || tab.last_active != expected.last_active
    {
        return Some("tab changed or was used since the warning");
    }
    None
}

#[derive(Default)]
struct TabClosePolicy<'a> {
    warned: Option<&'a [&'a TabInfo]>,
    auto: Option<&'a crate::daemon::AutoConfig>,
    rule_revision: Option<&'a str>,
}

fn automatic_tab_allowed(
    b: &BrowserInfo,
    tab: &TabInfo,
    auto: &crate::daemon::AutoConfig,
    revision: Option<&str>,
    now: u64,
) -> bool {
    match auto.tab_reason(b, tab, now) {
        Some(crate::tab_rules::AutoTabReason::EmptyNewTab) => revision.is_none(),
        Some(crate::tab_rules::AutoTabReason::Domain { .. }) => {
            revision == Some(auto.tab_rules_revision().as_str())
        }
        None => false,
    }
}

fn close_tabs_checked(
    browser: &str,
    ids: &[i32],
    reviewed: Option<&[ReviewedTab]>,
    t: &Thresholds,
    dry_run: bool,
    mode: &str,
    policy: TabClosePolicy<'_>,
) -> Result<Vec<ActionRecord>> {
    let mut sys = System::new();
    let snap = take_snapshot(&mut sys, Some(Duration::from_millis(300)), t);
    let Some(b) = snap.browsers.iter().find(|b| b.name == browser) else {
        anyhow::bail!("{browser} is not running");
    };
    if !b.can_close_tabs {
        anyhow::bail!("{browser} tabs cannot be closed from here");
    }
    let before = (!dry_run).then(MemorySample::collect).flatten();
    let mut out = Vec::new();
    let mut found = 0;
    for id in ids {
        let Some(tab) = b.tabs.iter().find(|x| x.id == *id) else {
            out.push(ActionRecord {
                id: None,
                status: "legacy".into(),
                identities: Vec::new(),
                target_identity: None,
                termination: None,
                memory_observation: None,
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
        let reason = if mode == "auto" {
            warned_tab_error(
                tab,
                policy
                    .warned
                    .and_then(|tabs| tabs.iter().copied().find(|x| x.id == *id)),
            )
            .or_else(|| {
                (!policy.auto.map_or_else(
                    || browser::can_auto_close_tab(b, tab),
                    |auto| automatic_tab_allowed(b, tab, auto, policy.rule_revision, snap.taken_at),
                ))
                .then_some("tab or current rules no longer allow automatic cleanup")
            })
        } else {
            reviewed.and_then(|tabs| reviewed_tab_error(tab, tabs.iter().find(|x| x.id == *id)))
        };
        if let Some(reason) = reason {
            let mut rec = close_tab(b, tab, mode, true, policy.auto);
            rec.mode = if dry_run { "dry-run" } else { mode }.to_string();
            rec.status = "skipped".into();
            rec.result = format!("skipped: {reason}");
            log(&rec)?;
            out.push(rec);
            continue;
        }
        let mut plan = close_tab(b, tab, mode, true, policy.auto);
        plan.target_identity = Some(
            serde_json::json!({"browser": b.name, "profile": tab.profile, "id": tab.id, "url": tab.url}),
        );
        out.push(journal_with(plan, mode, dry_run, log, || {
            // A scan or a previous close may have taken time. Read current
            // settings again at the last boundary before touching the browser.
            if let Some(expected) = policy.auto {
                let mut skipped = close_tab(b, tab, mode, true, Some(expected));
                match crate::config::Config::load() {
                    Ok((file, _)) => {
                        let current =
                            crate::daemon::DaemonConfig::from_config(&file, &Default::default());
                        if current.auto.dry_run == expected.dry_run
                            && automatic_tab_allowed(
                                b,
                                tab,
                                &current.auto,
                                policy.rule_revision,
                                now_epoch(),
                            )
                        {
                            return close_tab(b, tab, mode, false, Some(&current.auto));
                        }
                        skipped.result = "skipped: current settings prohibit this cleanup".into();
                    }
                    Err(error) => {
                        skipped.result = format!("skipped: settings could not be read: {error}")
                    }
                }
                skipped.mode = mode.into();
                skipped.status = "skipped".into();
                skipped
            } else {
                close_tab(b, tab, mode, false, None)
            }
        })?);
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
    observe_actions(&mut out, before, true, settled_memory, log)?;
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
fn quit_app(g: &AppGroup, root: u32, mode: &str, dry_run: bool) -> ActionRecord {
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
        id: None,
        status: "legacy".into(),
        identities: Vec::new(),
        target_identity: None,
        termination: None,
        memory_observation: None,
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
fn restart_app(
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
        id: None,
        status: "legacy".into(),
        identities: Vec::new(),
        target_identity: None,
        termination: None,
        memory_observation: None,
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
struct AppTarget {
    identities: Vec<ProcessIdentity>,
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
    // The agent's own client is folded into its sessions group ("Claude"
    // under "Claude Code sessions"); take the app's share back out so the
    // checks and the verb see the app alone.
    let g = match snap.groups.iter().find(|g| g.name == name) {
        Some(g) => g.clone(),
        None => match snap.groups.iter().find(|g| g.app.as_deref() == Some(name)) {
            Some(g) => groups::unfold_app(g, &snap.sessions),
            None => anyhow::bail!("{name} is not running"),
        },
    };
    let g = &g;
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
        identities: termination::capture(&sys, &g.pids),
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
    let mut plan = quit_app(&tgt.group, tgt.root, mode, true);
    plan.identities = tgt.identities;
    journal(plan, mode, dry_run, || {
        quit_app(&tgt.group, tgt.root, mode, false)
    })
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
    let mut plan = restart_app(&tgt.group, tgt.root, bundle, args, mode, true);
    plan.resume = Some(reopen_command(bundle, args));
    plan.identities = tgt.identities;
    journal(plan, mode, dry_run, || {
        restart_app(&tgt.group, tgt.root, bundle, args, mode, false)
    })
}

/// Append and sync before returning. A leading newline isolates any torn prior
/// write, and an OS file lock prevents concurrent CLI/tray/daemon interleaving.
fn append_record(path: &Path, rec: &ActionRecord) -> Result<()> {
    let mut bytes = vec![b'\n'];
    serde_json::to_writer(&mut bytes, rec)?;
    bytes.push(b'\n');
    crate::storage::append_journal(path, &bytes)
}
fn log(rec: &ActionRecord) -> Result<()> {
    let dir = paths::data_dir().context("no data directory")?;
    append_record(&dir.join("actions.jsonl"), rec)
}
fn outcome_status(rec: &ActionRecord) -> &'static str {
    if let Some(out) = &rec.termination {
        return if out.success() { "success" } else { "partial" };
    }
    if rec.result.starts_with("skipped:") {
        "skipped"
    } else if rec.result == "quit"
        || rec.result == "quit and relaunched"
        || rec.result.starts_with("closed")
        || rec.result.starts_with("already gone")
        || rec.result.starts_with("not open any more")
    {
        "success"
    } else {
        "failure"
    }
}
fn journal(
    plan: ActionRecord,
    mode: &str,
    dry_run: bool,
    execute: impl FnOnce() -> ActionRecord,
) -> Result<ActionRecord> {
    let before = (!dry_run).then(MemorySample::collect).flatten();
    let mut record = journal_with(plan, mode, dry_run, log, execute)?;
    observe_actions(
        std::slice::from_mut(&mut record),
        before,
        false,
        settled_memory,
        log,
    )?;
    Ok(record)
}
fn journal_with(
    mut plan: ActionRecord,
    mode: &str,
    dry_run: bool,
    mut persist: impl FnMut(&ActionRecord) -> Result<()>,
    execute: impl FnOnce() -> ActionRecord,
) -> Result<ActionRecord> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    plan.id = Some(format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    plan.mode = if dry_run { "dry-run" } else { mode }.into();
    plan.status = if dry_run { "dry_run" } else { "intent" }.into();
    plan.result = if dry_run {
        "dry run, nothing done"
    } else {
        "incomplete: execution outcome unknown; inspect the target before retrying"
    }
    .into();
    persist(&plan).context("could not persist action intent; nothing executed")?;
    if dry_run {
        return Ok(plan);
    }
    let executed = execute();
    if executed.resume.is_some() {
        plan.resume = executed.resume;
    }
    plan.result = executed.result;
    plan.termination = executed.termination;
    plan.status = outcome_status(&plan).into();
    persist(&plan).context(
        "action executed but completion could not be persisted; intent remains incomplete",
    )?;
    Ok(plan)
}
#[cfg(test)]
fn parse_log(text: &str, last: usize) -> Vec<ActionRecord> {
    let mut all: Vec<ActionRecord> = Vec::new();
    let mut indices = std::collections::HashMap::new();
    for rec in text
        .lines()
        .filter_map(|l| serde_json::from_str::<ActionRecord>(l).ok())
    {
        if let Some(id) = &rec.id {
            if let Some(&index) = indices.get(id) {
                all[index] = rec;
                continue;
            }
            indices.insert(id.clone(), all.len());
        }
        all.push(rec);
    }
    all.drain(..all.len().saturating_sub(last));
    all
}
pub fn read_log(last: usize) -> Result<Vec<ActionRecord>> {
    let path = paths::data_dir()
        .context("no data directory")?
        .join("actions.jsonl");
    read_log_at(&path, last)
}

fn read_log_at(path: &Path, last: usize) -> Result<Vec<ActionRecord>> {
    let mut seen = std::collections::HashSet::new();
    crate::storage::tail_records(path, last, |line| {
        let rec: ActionRecord = serde_json::from_str(line).ok()?;
        if let Some(id) = &rec.id
            && !seen.insert(id.clone())
        {
            return None;
        }
        Some(rec)
    })
}

#[cfg(test)]
mod rotated_journal_tests {
    use super::*;

    #[test]
    fn recent_actions_merge_intent_and_completion_across_rotated_files() {
        let dir = crate::storage::tests::Scratch::new();
        let path = dir.0.join("actions.jsonl");
        let mut value = serde_json::json!({"id": "one", "status": "intent", "ts": 1,
            "mode": "manual", "action": "close_tab", "pid": 0, "target": "fixture",
            "rss": 1, "result": "unknown"});
        crate::storage::append_json(&dir.0.join("actions.jsonl.1"), &value).unwrap();
        value["status"] = "success".into();
        value["result"] = "closed".into();
        crate::storage::append_json(&path, &value).unwrap();
        value["id"] = "two".into();
        value["status"] = "intent".into();
        crate::storage::append_json(&path, &value).unwrap();
        crate::storage::append_log(&path, b"torn JSON").unwrap();
        let records = read_log_at(&path, 10).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, "success");
        assert_eq!(records[1].status, "intent");
        assert_eq!(read_log_at(&path, 1).unwrap()[0].id.as_deref(), Some("two"));
    }
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
    if let Some(m) = &rec.memory_observation {
        s.push_str(&format!(
            "\n    Observed whole-machine change{}: RAM {}, swap {} over {:.1}s; includes other activity, not attributed savings",
            if m.tab_batch { format!(" across {} tab attempts", m.attempted_actions) } else { String::new() },
            fmt::memory_change(m.before.used_mem, m.after.used_mem),
            fmt::memory_change(m.before.used_swap, m.after.used_swap),
            m.after.taken_at_ms.saturating_sub(m.before.taken_at_ms) as f64 / 1000.0,
        ));
    }
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
    close_by_pid_checked(pid, None, t, dry_run, force, mode)
}

pub fn close_reviewed_session(
    pid: u32,
    expected_start_time: u64,
    t: &Thresholds,
) -> Result<ActionRecord> {
    close_by_pid_checked(pid, Some(expected_start_time), t, false, false, "manual")
}

fn check_session_identity(start_time: u64, expected: Option<u64>) -> Result<()> {
    if let Some(expected) = expected
        && start_time != expected
    {
        anyhow::bail!("session process changed since the review; review it again");
    }
    Ok(())
}

fn close_by_pid_checked(
    pid: u32,
    expected_start_time: Option<u64>,
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
    check_session_identity(s.start_time, expected_start_time)?;
    if s.is_self {
        anyhow::bail!("pid {pid} is the session running this command");
    }
    if s.state == SessionState::Active && !force {
        anyhow::bail!(
            "pid {pid} looks active ({:.0}% CPU); use force to close it anyway",
            s.cpu
        );
    }
    if s.engine && !force {
        anyhow::bail!(
            "pid {pid} is the {} engine under {}, which would restart it; close threads in the app, or use force to close them all at once",
            s.kind.label(),
            s.host
        );
    }
    execute_process(close_session(s, mode, true), &sys, &s.pids, mode, dry_run)
}

/// Stop a listening process by pid: only unmanaged ones without `force`.
pub fn stop_by_pid(
    pid: u32,
    t: &Thresholds,
    dry_run: bool,
    force: bool,
    mode: &str,
) -> Result<ActionRecord> {
    stop_reviewed_pid(pid, None, t, dry_run, force, mode)
}
pub fn stop_reviewed_pid(
    pid: u32,
    expected_start_time: Option<u64>,
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
    check_session_identity(p.start_time, expected_start_time)?;
    if p.owner_managed && !force {
        anyhow::bail!(
            "pid {pid} ({}) belongs to {}, which manages its own lifecycle; use force to stop it anyway",
            p.process,
            p.owner
        );
    }
    check_self_tree(&sys, &[p.pid])?;
    execute_process(stop_server(p, mode, true), &sys, &[p.pid], mode, dry_run)
}

/// Automatic callers supply a freshly sampled snapshot carrying the daemon's
/// existing observation window. This boundary never substitutes age-only evidence.
pub(crate) fn execute_auto(
    target: &crate::policy::Target,
    snap: &crate::Snapshot,
    sys: &System,
    cfg: &crate::daemon::DaemonConfig,
) -> Result<ActionRecord> {
    use crate::policy::{Target, auto_server_candidates, auto_session_candidates};
    let eligible = auto_target_eligible(target, snap, cfg);
    let (mut plan, pids, identity) = match target {
        Target::Session(old) => {
            let current = auto_session_candidates(snap, &cfg.auto, &cfg.thresholds)
                .into_iter()
                .find(|s| s.pid == old.pid && s.start_time == old.start_time);
            let s = current.unwrap_or(old);
            (
                close_session(s, "auto", true),
                s.pids.clone(),
                ProcessIdentity {
                    pid: old.pid,
                    start_time: old.start_time,
                },
            )
        }
        Target::Server(old) => {
            let current = auto_server_candidates(snap, &cfg.thresholds)
                .into_iter()
                .find(|p| p.pid == old.pid && p.start_time == old.start_time);
            let p = current.unwrap_or(old);
            (
                stop_server(p, "auto", true),
                vec![p.pid],
                ProcessIdentity {
                    pid: old.pid,
                    start_time: old.start_time,
                },
            )
        }
        Target::Tab {
            browser,
            tab,
            rule_revision,
        } => {
            anyhow::ensure!(
                eligible,
                "tab changed, was used, or current settings prohibit cleanup"
            );
            return close_tabs_checked(
                browser,
                &[tab.id],
                None,
                &cfg.thresholds,
                cfg.auto.dry_run,
                "auto",
                TabClosePolicy {
                    warned: Some(&[tab]),
                    auto: Some(&cfg.auto),
                    rule_revision: rule_revision.as_deref(),
                },
            )?
            .into_iter()
            .next()
            .context("no tab action result");
        }
    };
    if !eligible {
        plan.identities = vec![identity];
        let result = plan.clone();
        return journal(plan, "auto", false, || {
            let mut rec = result;
            rec.result = "skipped: target disappeared, changed identity, resumed activity, or current settings prohibit cleanup".into();
            rec
        });
    }
    execute_process(plan, sys, &pids, "auto", cfg.auto.dry_run)
}
fn auto_target_eligible(
    target: &crate::policy::Target,
    snap: &crate::Snapshot,
    cfg: &crate::daemon::DaemonConfig,
) -> bool {
    use crate::policy::{Target, auto_server_candidates, auto_session_candidates};
    match target {
        Target::Session(old) => {
            cfg.auto.close_sessions
                && auto_session_candidates(snap, &cfg.auto, &cfg.thresholds)
                    .iter()
                    .any(|s| {
                        s.pid == old.pid
                            && s.start_time == old.start_time
                            && s.cpu < cfg.thresholds.quiet_cpu
                    })
        }
        Target::Server(old) => {
            cfg.auto.stop_servers
                && auto_server_candidates(snap, &cfg.thresholds)
                    .iter()
                    .any(|p| p.pid == old.pid && p.start_time == old.start_time)
        }
        Target::Tab {
            browser: name,
            tab: old,
            rule_revision,
        } => {
            cfg.auto.on()
                && snap.browsers.iter().filter(|b| &b.name == name).any(|b| {
                    b.tabs.iter().any(|tab| {
                        automatic_tab_allowed(
                            b,
                            tab,
                            &cfg.auto,
                            rule_revision.as_deref(),
                            snap.taken_at,
                        ) && warned_tab_error(tab, Some(old)).is_none()
                    })
                })
        }
    }
}

fn check_self_tree(sys: &System, pids: &[u32]) -> Result<()> {
    let mut current = Some(Pid::from_u32(std::process::id()));
    for _ in 0..128 {
        let Some(pid) = current else { break };
        anyhow::ensure!(
            !pids.contains(&pid.as_u32()),
            "target contains this command or its host"
        );
        current = sys.process(pid).and_then(|p| p.parent());
    }
    Ok(())
}
fn execute_process(
    mut plan: ActionRecord,
    sys: &System,
    pids: &[u32],
    mode: &str,
    dry_run: bool,
) -> Result<ActionRecord> {
    check_self_tree(sys, pids)?;
    plan.identities = termination::capture(sys, pids);
    anyhow::ensure!(
        plan.identities.len() == pids.len(),
        "target tree changed during validation; review it again"
    );
    let identities = plan.identities.clone();
    let result = plan.clone();
    journal(plan, mode, dry_run, || {
        let mut result = result;
        let outcome = termination::terminate(&identities, Duration::from_secs(10));
        result.result = outcome.describe();
        result.termination = Some(outcome);
        result
    })
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

    #[test]
    fn automatic_domain_actions_recheck_the_warned_rule_and_every_protection() {
        let file: crate::config::Config = toml::from_str(
            r#"
            auto_close_tabs = true
            auto_tab_domains = [{ domain = "reddit.com", include_subdomains = true }]
        "#,
        )
        .unwrap();
        let mut cfg = crate::daemon::DaemonConfig::from_config(&file, &Default::default());
        let mut snap = crate::test_support::chrome_snapshot();
        snap.taken_at = 100_000;
        snap.browsers[0].tabs[0].url = "https://old.reddit.com/r/rust".into();
        snap.browsers[0].tabs[0].last_active = Some(1);
        let target = crate::policy::Target::Tab {
            browser: "Google Chrome".into(),
            tab: snap.browsers[0].tabs[0].clone(),
            rule_revision: Some(cfg.auto.tab_rules_revision()),
        };
        assert!(auto_target_eligible(&target, &snap, &cfg));
        for change in 0..6 {
            let b = &mut snap.browsers[0];
            let old = b.tabs[0].clone();
            match change {
                0 => b.tabs[0].active = true,
                1 => b.tabs[0].pinned = true,
                2 => b.tabs[0].last_active = Some(2),
                3 => b.tabs[0].url = "https://old.reddit.com/other".into(),
                4 => b.tabs[0].profile = "Profile 2".into(),
                _ => b.tabs[0].window_id += 1,
            }
            assert!(!auto_target_eligible(&target, &snap, &cfg), "case {change}");
            snap.browsers[0].tabs[0] = old;
        }
        cfg.auto.tab_inactive_secs += 1;
        assert!(!auto_target_eligible(&target, &snap, &cfg));
        cfg.auto.tab_inactive_secs -= 1;
        cfg.auto.dry_run = true;
        assert!(!auto_target_eligible(&target, &snap, &cfg));
        cfg.auto.dry_run = false;
        cfg.auto.close_tabs = false;
        cfg.auto.close_sessions = true;
        assert!(!auto_target_eligible(&target, &snap, &cfg));
        cfg.auto.close_tabs = true;
        cfg.auto.tab_domains.clear();
        assert!(!auto_target_eligible(&target, &snap, &cfg));
    }

    #[test]
    fn automatic_tab_execution_rechecks_current_settings_and_activity() {
        let mut snap = crate::test_support::chrome_snapshot();
        let target = crate::policy::Target::Tab {
            browser: snap.browsers[0].name.clone(),
            tab: snap.browsers[0].tabs[0].clone(),
            rule_revision: None,
        };
        let mut cfg = crate::daemon::DaemonConfig::from_config(
            &crate::config::Config {
                auto_close_sessions: true,
                ..Default::default()
            },
            &crate::daemon::Overrides::default(),
        );
        assert!(auto_target_eligible(&target, &snap, &cfg));
        cfg.auto.close_sessions = false;
        assert!(!auto_target_eligible(&target, &snap, &cfg));
        cfg.auto.stop_servers = true;
        assert!(auto_target_eligible(&target, &snap, &cfg));
        snap.browsers[0].tabs[0].last_active = Some(650);
        assert!(!auto_target_eligible(&target, &snap, &cfg));
        snap.browsers[0].tabs[0].last_active = None;
        snap.browsers[0].tabs[0].pinned = true;
        assert!(!auto_target_eligible(&target, &snap, &cfg));
        snap.browsers[0].tabs[0].pinned = false;
        snap.browsers[0].tabs[0].active = true;
        assert!(!auto_target_eligible(&target, &snap, &cfg));
        snap.browsers.clear();
        assert!(!auto_target_eligible(&target, &snap, &cfg));
    }

    #[test]
    fn automatic_tab_actions_recheck_empty_page_and_protections() {
        let mut snap = crate::test_support::chrome_snapshot();
        let b = &mut snap.browsers[0];
        for url in [
            "chrome://newtab",
            "chrome://newtab/",
            "chrome://new-tab-page/",
            "chrome://new-tab-page-third-party/",
        ] {
            b.tabs[0].url = url.into();
            let rec = close_tab(b, &b.tabs[0], "auto", true, None);
            assert_eq!(rec.result, "dry run, nothing done", "{url}");
            assert_eq!(rec.mode, "dry-run");
        }
        for url in [
            "",
            "about:blank",
            "chrome://settings/",
            "chrome://newtab/other",
            "chrome://newtab/?q=work",
            "chrome://newtab.evil/",
            "https://www.google.com/",
            "https://example.com",
        ] {
            b.tabs[0].url = url.into();
            assert!(
                close_tab(b, &b.tabs[0], "auto", true, None)
                    .result
                    .starts_with("skipped:"),
                "{url}"
            );
        }
        b.tabs[0].url = "chrome://newtab/".into();
        b.tabs[0].active = true;
        assert!(
            close_tab(b, &b.tabs[0], "auto", true, None)
                .result
                .starts_with("skipped:")
        );
        b.tabs[0].active = false;
        b.tabs[0].pinned = true;
        assert!(
            close_tab(b, &b.tabs[0], "auto", true, None)
                .result
                .starts_with("skipped:")
        );
        // An explicit manual close keeps its existing behavior.
        assert_eq!(
            close_tab(b, &b.tabs[0], "manual", true, None).result,
            "dry run, nothing done"
        );
    }

    #[test]
    fn reviewed_session_rejects_reused_pid() {
        assert!(check_session_identity(100, Some(100)).is_ok());
        assert!(check_session_identity(200, Some(100)).is_err());
        assert!(check_session_identity(200, None).is_ok());
    }

    #[test]
    fn reviewed_tab_requires_same_page_profile_and_unprotected_state() {
        let mut tab = TabInfo {
            id: 7,
            window_id: 1,
            profile: "Default".into(),
            index: 0,
            url: "https://example.com/notes".into(),
            site: "example.com".into(),
            title: "Notes".into(),
            pinned: false,
            active: false,
            last_active: None,
            idle_secs: Some(90000),
            kind: Default::default(),
        };
        let expected = ReviewedTab {
            id: tab.id,
            url: tab.url.clone(),
            profile: tab.profile.clone(),
        };
        assert!(reviewed_tab_error(&tab, Some(&expected)).is_none());
        assert!(reviewed_tab_error(&tab, None).is_some());
        tab.url.push_str("/edited");
        assert!(reviewed_tab_error(&tab, Some(&expected)).is_some());
        tab.url = expected.url.clone();
        tab.profile = "Profile 2".into();
        assert!(reviewed_tab_error(&tab, Some(&expected)).is_some());
        tab.profile = expected.profile.clone();
        tab.pinned = true;
        assert!(reviewed_tab_error(&tab, Some(&expected)).is_some());
        tab.pinned = false;
        tab.active = true;
        assert!(reviewed_tab_error(&tab, Some(&expected)).is_some());
        tab.active = false;
        tab.id += 1;
        assert!(reviewed_tab_error(&tab, Some(&expected)).is_some());
    }

    #[test]
    fn automatic_close_rejects_changes_during_the_final_rescan() {
        let warned = TabInfo {
            id: 7,
            window_id: 1,
            profile: "Default".into(),
            index: 0,
            url: "chrome://newtab/".into(),
            site: "chrome://newtab".into(),
            title: "New Tab".into(),
            pinned: false,
            active: false,
            last_active: Some(100),
            idle_secs: Some(600),
            kind: Default::default(),
        };
        assert!(warned_tab_error(&warned, Some(&warned)).is_none());
        assert!(warned_tab_error(&warned, None).is_some());
        for change in 0..5 {
            let mut current = warned.clone();
            match change {
                0 => current.last_active = Some(650),
                1 => current.window_id = 2,
                2 => current.profile = "Profile 2".into(),
                3 => current.url = "chrome://new-tab-page/".into(),
                _ => current.id = 8,
            }
            assert!(
                warned_tab_error(&current, Some(&warned)).is_some(),
                "case {change}"
            );
        }
    }

    fn app(kind: GroupKind) -> AppGroup {
        AppGroup {
            name: "Slack".to_string(),
            kind,
            rss: 0,
            cpu: 0.0,
            procs: 1,
            pids: vec![100],
            app: None,
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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::test_support::*;
    use std::cell::Cell;

    fn sample(time: u64, ram: u64, swap: u64) -> MemorySample {
        MemorySample {
            taken_at_ms: time,
            used_mem: ram,
            used_swap: swap,
        }
    }

    #[test]
    fn memory_observation_records_batch_once_and_keeps_increases() {
        let mut records: Vec<_> = ["success", "failure", "skipped"]
            .iter()
            .enumerate()
            .map(|(i, status)| {
                let mut rec = close_session(&session(), "manual", true);
                rec.id = Some(format!("action-{i}"));
                rec.mode = "manual".into();
                rec.status = (*status).into();
                rec
            })
            .collect();
        let mut log_text = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        observe_actions(
            &mut records,
            Some(sample(1000, 4096, 4096)),
            true,
            || Some(sample(2500, 3072, 6144)),
            |r| {
                log_text.push('\n');
                log_text += &serde_json::to_string(r)?;
                Ok(())
            },
        )
        .unwrap();
        assert!(records[0].memory_observation.is_none());
        assert!(records[2].memory_observation.is_none());
        let observation = records[1].memory_observation.as_ref().unwrap();
        assert_eq!(observation.attempted_actions, 2);
        assert!(observation.tab_batch);
        let merged = parse_log(&log_text, 10);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[1].status, "failure");
        assert_eq!(merged[1].memory_observation.as_ref(), Some(observation));
        let text = describe(&merged[1]);
        assert!(text.contains("RAM −1 KiB, swap +2 KiB over 1.5s"), "{text}");
        assert!(text.contains("not attributed savings"));
    }

    #[test]
    fn previews_skips_and_missing_samples_do_not_claim_changes() {
        for status in ["dry_run", "skipped", "intent", "legacy"] {
            let mut rec = close_session(&session(), "manual", true);
            rec.status = status.into();
            if status != "dry_run" {
                rec.mode = "manual".into();
            }
            observe_actions(
                std::slice::from_mut(&mut rec),
                Some(sample(0, 100, 10)),
                false,
                || panic!("must not sample unexecuted targets"),
                |_| panic!("must not persist"),
            )
            .unwrap();
            assert!(rec.memory_observation.is_none());
        }
        let mut rec = close_session(&session(), "manual", true);
        rec.mode = "manual".into();
        rec.status = "success".into();
        observe_actions(
            std::slice::from_mut(&mut rec),
            None,
            false,
            || panic!("no baseline"),
            |_| panic!("must not persist"),
        )
        .unwrap();
        observe_actions(
            std::slice::from_mut(&mut rec),
            Some(sample(0, 100, 10)),
            false,
            || None,
            |_| panic!("no followup sample"),
        )
        .unwrap();
        assert!(rec.memory_observation.is_none());
    }

    #[test]
    fn observation_write_failure_keeps_completed_outcome() {
        let mut rec = close_session(&session(), "manual", true);
        rec.mode = "manual".into();
        rec.status = "partial".into();
        let error = observe_actions(
            std::slice::from_mut(&mut rec),
            Some(sample(0, 100, 10)),
            false,
            || Some(sample(1000, 100, 10)),
            |_| anyhow::bail!("disk full"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("action completed"));
        assert_eq!(rec.status, "partial");
        assert_eq!(rec.memory_observation.unwrap().attempted_actions, 1);
    }
    #[test]
    fn journal_fails_closed_and_keeps_incomplete_intent() {
        let plan = close_session(&session(), "manual", true);
        let ran = Cell::new(false);
        assert!(
            journal_with(
                plan.clone(),
                "manual",
                false,
                |_| anyhow::bail!("disk full"),
                || {
                    ran.set(true);
                    plan.clone()
                }
            )
            .is_err()
        );
        assert!(!ran.get());
        let mut records = Vec::new();
        let result = journal_with(
            plan.clone(),
            "manual",
            false,
            |r| {
                if records.is_empty() {
                    records.push(r.clone());
                    Ok(())
                } else {
                    anyhow::bail!("completion failed")
                }
            },
            || {
                ran.set(true);
                plan.clone()
            },
        );
        assert!(result.is_err());
        assert!(ran.get());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "intent");
        assert!(records[0].resume == plan.resume && records[0].transcript == plan.transcript);
        assert!(records[0].result.contains("unknown"));
        let mut writes = Vec::new();
        let done = journal_with(
            plan.clone(),
            "auto",
            false,
            |r| {
                writes.push(r.clone());
                Ok(())
            },
            || {
                let mut r = plan.clone();
                r.result = "failed: fixture".into();
                r
            },
        )
        .unwrap();
        assert_eq!(done.status, "failure");
        assert_eq!(writes[0].id, writes[1].id);
        let mut log_text = serde_json::to_string(&plan).unwrap();
        log_text.push('\n');
        for rec in writes {
            log_text += &serde_json::to_string(&rec).unwrap();
            log_text.push('\n');
        }
        let merged = parse_log(&log_text, 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].status, "failure");
        let dry = journal_with(
            plan.clone(),
            "auto",
            true,
            |_| Ok(()),
            || panic!("dry run executed"),
        )
        .unwrap();
        assert_eq!(dry.status, "dry_run");
        assert_eq!(dry.mode, "dry-run");
    }
    #[test]
    fn interruption_is_visible_and_legacy_records_still_parse() {
        let plan = close_session(&session(), "manual", true);
        let mut records = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = journal_with(
                plan,
                "manual",
                false,
                |r| {
                    records.push(r.clone());
                    Ok(())
                },
                || panic!("interrupted"),
            );
        }));
        assert!(result.is_err());
        assert_eq!(records[0].status, "intent");
        let mut value = serde_json::to_value(&records[0]).unwrap();
        for field in [
            "id",
            "status",
            "identities",
            "target_identity",
            "termination",
        ] {
            value.as_object_mut().unwrap().remove(field);
        }
        let legacy: ActionRecord = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.status, "legacy");
    }
    #[test]
    fn revalidation_rejects_changes_between_batch_targets() {
        let mut cfg = crate::daemon::DaemonConfig::from_config(
            &crate::config::Config::default(),
            &crate::daemon::Overrides::default(),
        );
        cfg.auto.close_sessions = true;
        cfg.auto.stop_servers = true;
        cfg.auto.hosts = vec!["terminal".into()];
        let mut snap = snapshot();
        let mut s = session();
        s.project = None;
        snap.sessions.push(s.clone());
        snap.ports.push(port());
        let selected = crate::policy::Target::Session(s.clone());
        assert!(auto_target_eligible(&selected, &snap, &cfg));
        for change in [
            (|s: &mut AgentSession| s.start_time += 1) as fn(&mut AgentSession),
            |s| s.cpu = 3.,
            |s| s.idle_secs = Some(0),
            |s| s.engine = true,
            |s| s.is_self = true,
            |s| s.quiet_for_secs = None,
            |s| s.cpu_window_mean = None,
        ] {
            snap.sessions[0] = s.clone();
            change(&mut snap.sessions[0]);
            snap.sessions[0].state =
                crate::rules::session_state(&snap.sessions[0], &cfg.thresholds);
            assert!(!auto_target_eligible(&selected, &snap, &cfg));
        }
        snap.sessions.clear();
        assert!(!auto_target_eligible(&selected, &snap, &cfg));
        snap.sessions.push(s);
        cfg.auto.close_sessions = false;
        assert!(!auto_target_eligible(&selected, &snap, &cfg));
        let server = crate::policy::Target::Server(snap.ports[0].clone());
        assert!(auto_target_eligible(&server, &snap, &cfg));
        cfg.auto.stop_servers = false;
        assert!(!auto_target_eligible(&server, &snap, &cfg));
        cfg.auto.stop_servers = true;
        snap.ports[0].start_time += 1;
        assert!(!auto_target_eligible(&server, &snap, &cfg));
    }
}
