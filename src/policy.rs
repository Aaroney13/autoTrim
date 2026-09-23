//! Pure automatic cleanup decisions. No I/O, clock reads, or actions.
use crate::{
    Snapshot,
    agents::{AgentSession, SessionState},
    browser::TabInfo,
    daemon::{AutoConfig, DaemonConfig, PendingTarget},
    fmt::dur,
    rules::Thresholds,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct PolicyState {
    #[serde(default)]
    pub pending: HashMap<String, u64>,
    #[serde(default)]
    pub dry_done: HashSet<String>,
}

pub(crate) enum Target {
    Session(AgentSession),
    Codex {
        task: crate::codex::Task,
        rule_revision: String,
    },
    Server(crate::ports::PortInfo),
    Tab {
        browser: String,
        tab: TabInfo,
        rule_revision: Option<String>,
    },
}
impl Target {
    pub fn key(&self) -> String {
        match self {
            Self::Session(s) => format!("s:{}:{}", s.pid, s.start_time),
            Self::Codex {
                task,
                rule_revision,
            } => format!(
                "c:{}",
                serde_json::to_string(&(
                    task.backend_pid,
                    task.backend_created_at,
                    &task.codex_home,
                    &task.id,
                    &task.revision,
                    rule_revision,
                ))
                .expect("task identity is serializable")
            ),
            Self::Server(p) => format!("p:{}:{}:{}", p.pid, p.start_time, p.port),
            Self::Tab {
                browser,
                tab,
                rule_revision,
            } => format!(
                "t:{}",
                serde_json::to_string(&(
                    browser,
                    &tab.profile,
                    tab.window_id,
                    tab.id,
                    &tab.url,
                    tab.last_active,
                    rule_revision,
                ))
                .expect("tab identity is serializable")
            ),
        }
    }
}
pub(crate) struct Decisions {
    pub warned: Vec<String>,
    pub pending: Vec<PendingTarget>,
    pub eligible: Vec<Target>,
    pub cancelled: Vec<String>,
}
/// Sessions auto mode may close this tick. Stricter than the advice: it
/// needs transcript evidence of idleness, a warm quiet window agreeing,
/// an allowed host, never an app's own engine (the app would restart it),
/// and it spares the most recently active session in each project so a
/// person always keeps their place.
pub(crate) fn auto_session_candidates<'a>(
    snap: &'a Snapshot,
    auto: &AutoConfig,
    t: &Thresholds,
) -> Vec<&'a AgentSession> {
    let mut newest: HashMap<&str, u64> = HashMap::new();
    for s in &snap.sessions {
        if let (Some(p), Some(la)) = (s.project.as_deref(), s.last_activity) {
            let e = newest.entry(p).or_insert(0);
            if la > *e {
                *e = la;
            }
        }
    }
    snap.sessions
        .iter()
        .filter(|s| s.state == SessionState::Stale && !s.is_self)
        // An app's agent engine (Codex's server, Copilot's) is restarted by
        // its app, so closing it frees nothing for long. Only `close --force`
        // will.
        .filter(|s| !s.engine)
        .filter(|s| s.last_activity.is_some() && s.cpu_window_mean.is_some())
        .filter(|s| s.idle_secs.is_some_and(|i| i >= t.stale_after_secs))
        .filter(|s| s.quiet_for_secs.is_some_and(|q| q >= t.min_quiet_secs))
        .filter(|s| auto.hosts.iter().any(|h| h == &s.host))
        .filter(|s| {
            let path = s.cwd.as_deref().or(s.project.as_deref()).unwrap_or("");
            !t.ignore_projects
                .iter()
                .any(|p| !p.is_empty() && path.contains(p.as_str()))
        })
        .filter(|s| match (s.project.as_deref(), s.last_activity) {
            (Some(p), Some(la)) => newest.get(p).is_none_or(|n| la < *n),
            _ => true,
        })
        .collect()
}

/// Only verified, unchanged idle task identities may enter the warning period.
/// Keep the newest loaded task in each project (including ties); missing backend
/// evidence prevents us from knowing which task is newest, so fail closed.
pub(crate) fn auto_codex_candidates<'a>(
    snap: &'a Snapshot,
    cfg: &DaemonConfig,
) -> Vec<&'a crate::codex::Task> {
    if !cfg.auto.archive_codex || snap.codex_backends.iter().any(|b| b.error.is_some()) {
        return Vec::new();
    }
    let mut newest: HashMap<(&str, &str), u64> = HashMap::new();
    for task in snap.codex_backends.iter().flat_map(|b| &b.tasks) {
        let latest = newest.entry((&task.codex_home, &task.cwd)).or_default();
        *latest = (*latest).max(task.updated_at);
    }
    let mut candidates: Vec<_> = snap
        .codex_backends
        .iter()
        .flat_map(|b| &b.tasks)
        .filter(|t| {
            t.state == "idle" && t.protection.is_none() && !t.cwd.is_empty() && t.updated_at > 0
        })
        .filter(|t| snap.taken_at.saturating_sub(t.updated_at) >= cfg.thresholds.stale_after_secs)
        .filter(|t| {
            newest
                .get(&(t.codex_home.as_str(), t.cwd.as_str()))
                .is_some_and(|new| t.updated_at < *new)
        })
        .filter(|t| {
            !cfg.thresholds
                .ignore_projects
                .iter()
                .any(|p| !p.is_empty() && t.cwd.contains(p))
        })
        .filter(|_| {
            !cfg.thresholds.ignore_apps.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "Codex" | "ChatGPT" | "Codex app" | "ChatGPT app"
                )
            })
        })
        .collect();
    candidates.sort_by_key(|t| (t.updated_at, &t.id));
    candidates
}

/// Servers auto mode may stop: the old-servers rule's targets, narrowed to
/// known dev runtimes. Anything else old and unmanaged is only reported.
pub(crate) fn auto_server_candidates<'a>(
    snap: &'a Snapshot,
    t: &Thresholds,
) -> Vec<&'a crate::ports::PortInfo> {
    let mut seen = BTreeSet::new();
    snap.ports
        .iter()
        .filter(|p| !p.owner_managed && p.dev_runtime)
        .filter(|p| !t.ignore_ports.contains(&p.port))
        .filter(|p| p.open_for_secs >= t.port_stale_after_secs)
        .filter(|p| p.owner_cpu < t.quiet_cpu)
        .filter(|p| seen.insert(p.pid))
        .collect()
}

/// One pass of auto mode: warn about new candidates, act on ones whose
/// grace has run out, forget ones that went away or woke up. Returns what
/// the snapshot should say about it. With auto mode off the pending list
/// is emptied, so switching it on later starts every grace period afresh.
pub(crate) fn decide(
    state: &mut PolicyState,
    snap: &Snapshot,
    cfg: &DaemonConfig,
    now: u64,
) -> Decisions {
    let previous: Vec<_> = state.pending.keys().cloned().collect();
    let auto = &cfg.auto;
    let grace = auto.grace.as_secs();
    let mut live = std::collections::HashSet::new();
    let mut warned: Vec<String> = Vec::new();
    let mut acted: Vec<Target> = Vec::new();
    let mut pending: Vec<PendingTarget> = Vec::new();

    if !auto.dry_run {
        state.dry_done.clear();
    }
    if auto.close_sessions {
        for s in auto_session_candidates(snap, auto, &cfg.thresholds) {
            let k = format!("s:{}:{}", s.pid, s.start_time);
            live.insert(k.clone());
            if auto.dry_run && state.dry_done.contains(&k) {
                continue;
            }
            let name = s
                .session_name
                .as_deref()
                .or(s.project.as_deref())
                .unwrap_or("?");
            let detail = format!("idle {}", dur(s.idle_secs.unwrap_or(0)));
            let first = *state.pending.entry(k.clone()).or_insert_with(|| {
                warned.push(format!("{} · {} ({})", s.kind.label(), name, detail));
                now
            });
            if now.saturating_sub(first) >= grace {
                acted.push(Target::Session(s.clone()));
                if auto.dry_run {
                    state.dry_done.insert(k);
                }
            } else {
                pending.push(PendingTarget {
                    kind: "session".to_string(),
                    pid: s.pid,
                    target: format!("{} · {}", s.kind.label(), name),
                    detail,
                    rss: s.rss,
                    since: first,
                    due_at: first + grace,
                });
            }
        }
    }
    if auto.archive_codex {
        let mut archives = 0;
        for task in auto_codex_candidates(snap, cfg) {
            let target = Target::Codex {
                task: task.clone(),
                rule_revision: cfg.codex_rules_revision(),
            };
            let key = target.key();
            live.insert(key.clone());
            if auto.dry_run && state.dry_done.contains(&key) {
                continue;
            }
            let first = *state.pending.entry(key.clone()).or_insert_with(|| {
                warned.push(format!("Codex · {} (will archive)", task.name));
                now
            });
            // Always observe on a later tick, even with a zero configured grace.
            // Limit archiving to two tasks per pass; later targets stay pending.
            if now > first && now.saturating_sub(first) >= grace && archives < 2 {
                acted.push(target);
                archives += 1;
                if auto.dry_run {
                    state.dry_done.insert(key);
                }
            } else {
                pending.push(PendingTarget {
                    kind: "codex_task".into(),
                    pid: task.backend_pid,
                    target: format!("Codex · {}", task.name),
                    detail: format!(
                        "idle {} · archive task",
                        dur(now.saturating_sub(task.updated_at))
                    ),
                    rss: 0,
                    since: first,
                    due_at: first.saturating_add(grace),
                });
            }
        }
    }
    if auto.stop_servers {
        for p in auto_server_candidates(snap, &cfg.thresholds) {
            let k = format!("p:{}:{}:{}", p.pid, p.start_time, p.port);
            live.insert(k.clone());
            if auto.dry_run && state.dry_done.contains(&k) {
                continue;
            }
            let detail = format!("open {}", dur(p.open_for_secs));
            let first = *state.pending.entry(k.clone()).or_insert_with(|| {
                warned.push(format!(
                    "{} on {}:{} ({})",
                    p.process, p.addr, p.port, detail
                ));
                now
            });
            if now.saturating_sub(first) >= grace {
                acted.push(Target::Server(p.clone()));
                if auto.dry_run {
                    state.dry_done.insert(k);
                }
            } else {
                pending.push(PendingTarget {
                    kind: "server".to_string(),
                    pid: p.pid,
                    target: format!("{} on {}:{}", p.process, p.addr, p.port),
                    detail,
                    rss: p.owner_rss,
                    since: first,
                    due_at: first + grace,
                });
            }
        }
    }
    // Any auto-mode target also includes empty Chrome New Tab pages.
    // Their lack of content is enough evidence; a recorded idle time is
    // not required. A visit changes the key and restarts the warning.
    if auto.on() {
        for b in &snap.browsers {
            for tab in &b.tabs {
                let Some(reason) = auto.tab_reason(b, tab, now) else {
                    continue;
                };
                let (label, detail, rule_revision) = match reason {
                    crate::tab_rules::AutoTabReason::EmptyNewTab => (
                        "New Tab".to_string(),
                        "empty New Tab page".to_string(),
                        None,
                    ),
                    crate::tab_rules::AutoTabReason::Domain { domain } => (
                        if tab.title.is_empty() {
                            domain.clone()
                        } else {
                            tab.title.clone()
                        },
                        format!(
                            "{domain} · inactive {}",
                            dur(now.saturating_sub(tab.last_active.unwrap_or(now)))
                        ),
                        Some(auto.tab_rules_revision()),
                    ),
                };
                let target = Target::Tab {
                    browser: b.name.clone(),
                    tab: tab.clone(),
                    rule_revision,
                };
                let k = target.key();
                live.insert(k.clone());
                if auto.dry_run && state.dry_done.contains(&k) {
                    continue;
                }
                let name = format!("{} · {} · {}", b.name, label, tab.profile);
                let first = *state.pending.entry(k.clone()).or_insert_with(|| {
                    warned.push(name.clone());
                    now
                });
                if now.saturating_sub(first) >= grace {
                    acted.push(target);
                    if auto.dry_run {
                        state.dry_done.insert(k);
                    }
                } else {
                    pending.push(PendingTarget {
                        kind: "tab".into(),
                        pid: 0,
                        target: name,
                        detail,
                        rss: b.per_tab_estimate.unwrap_or(0),
                        since: first,
                        due_at: first + grace,
                    });
                }
            }
        }
    }
    state.pending.retain(|k, _| live.contains(k));
    state.dry_done.retain(|k| live.contains(k));

    let cancelled = previous.into_iter().filter(|k| !live.contains(k)).collect();
    Decisions {
        warned,
        pending,
        eligible: acted,
        cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, daemon::Overrides, test_support::*};
    fn config() -> DaemonConfig {
        let mut cfg = DaemonConfig::from_config(&Config::default(), &Overrides::default());
        cfg.auto = AutoConfig {
            close_sessions: true,
            stop_servers: true,
            grace: std::time::Duration::from_secs(60),
            hosts: vec!["terminal".into()],
            dry_run: false,
            ..AutoConfig::default()
        };
        cfg
    }
    fn eligible_snapshot() -> Snapshot {
        let mut snap = snapshot();
        let old = session();
        let mut new = old.clone();
        new.pid += 1;
        new.last_activity = Some(200);
        snap.sessions = vec![old, new];
        snap.ports = vec![port()];
        snap
    }
    #[test]
    fn session_exclusions_and_newest_ties() {
        let cfg = config();
        let changes: Vec<fn(&mut AgentSession)> = vec![
            |s| s.is_self = true,
            |s| s.engine = true,
            |s| s.idle_secs = None,
            |s| s.last_activity = None,
            |s| s.cpu_window_mean = None,
            |s| s.quiet_for_secs = None,
            |s| s.quiet_for_secs = Some(899),
            |s| s.host = "unknown".into(),
            |s| s.state = SessionState::Active,
            |s| s.idle_secs = Some(21599),
            |s| s.last_activity = Some(200),
        ];
        assert_eq!(
            auto_session_candidates(&eligible_snapshot(), &cfg.auto, &cfg.thresholds).len(),
            1
        );
        for change in changes {
            let mut snap = eligible_snapshot();
            change(&mut snap.sessions[0]);
            assert!(auto_session_candidates(&snap, &cfg.auto, &cfg.thresholds).is_empty());
        }
        let mut t = cfg.thresholds;
        t.ignore_projects = vec!["/work".into()];
        assert!(auto_session_candidates(&eligible_snapshot(), &cfg.auto, &t).is_empty());
    }
    #[test]
    fn server_exclusions_thresholds_and_dedup() {
        let cfg = config();
        for change in [
            (|p: &mut crate::ports::PortInfo| p.owner_managed = true)
                as fn(&mut crate::ports::PortInfo),
            |p| p.dev_runtime = false,
            |p| p.open_for_secs = 86399,
            |p| p.owner_cpu = 2.,
        ] {
            let mut snap = eligible_snapshot();
            change(&mut snap.ports[0]);
            assert!(auto_server_candidates(&snap, &cfg.thresholds).is_empty());
        }
        let mut snap = eligible_snapshot();
        snap.ports[0].open_for_secs = 86400;
        snap.ports[0].owner_cpu = 1.99;
        let mut second = snap.ports[0].clone();
        second.port += 1;
        snap.ports.push(second);
        assert_eq!(auto_server_candidates(&snap, &cfg.thresholds).len(), 1);
        let mut t = cfg.thresholds;
        t.ignore_ports = vec![3000, 3001];
        assert!(auto_server_candidates(&snap, &t).is_empty());
    }
    #[test]
    fn grace_cancellation_restart_and_mode_switches() {
        let mut cfg = config();
        let mut state = PolicyState::default();
        let mut snap = eligible_snapshot();
        let first = decide(&mut state, &snap, &cfg, 1000);
        assert_eq!(first.warned.len(), 2);
        assert_eq!(first.pending.len(), 2);
        assert!(first.eligible.is_empty());
        assert!(decide(&mut state, &snap, &cfg, 1059).eligible.is_empty());
        state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(decide(&mut state, &snap, &cfg, 1060).eligible.len(), 2);
        snap.sessions[0].state = SessionState::Active;
        snap.ports.clear();
        assert_eq!(decide(&mut state, &snap, &cfg, 1061).cancelled.len(), 2);
        snap = eligible_snapshot();
        assert_eq!(decide(&mut state, &snap, &cfg, 1070).warned.len(), 2);
        cfg.auto.close_sessions = false;
        cfg.auto.stop_servers = false;
        assert_eq!(decide(&mut state, &snap, &cfg, 1071).cancelled.len(), 2);
        cfg = config();
        cfg.auto.dry_run = true;
        assert_eq!(decide(&mut state, &snap, &cfg, 1080).warned.len(), 2);
        assert_eq!(decide(&mut state, &snap, &cfg, 1140).eligible.len(), 2);
        assert!(decide(&mut state, &snap, &cfg, 1200).eligible.is_empty());
        // The adapter removes completed pending entries. Leaving preview requires a fresh warning.
        state.pending.clear();
        cfg.auto.dry_run = false;
        assert_eq!(decide(&mut state, &snap, &cfg, 1201).warned.len(), 2);
        assert!(decide(&mut state, &snap, &cfg, 1260).eligible.is_empty());
        assert_eq!(decide(&mut state, &snap, &cfg, 1261).eligible.len(), 2);
    }
}

#[cfg(test)]
mod chrome_tab_tests {
    use super::*;
    use crate::{config::Config, daemon::Overrides};

    use crate::test_support::chrome_snapshot;

    fn auto_config() -> DaemonConfig {
        DaemonConfig::from_config(
            &Config {
                auto_close_sessions: true,
                notify: false,
                ..Config::default()
            },
            &Overrides::default(),
        )
    }

    #[test]
    fn auto_warns_for_empty_chrome_tabs_without_an_idle_timestamp() {
        let cfg = auto_config();
        let mut tracker = PolicyState::default();
        let status = decide(&mut tracker, &chrome_snapshot(), &cfg, 100);
        assert_eq!(status.pending.len(), 1);
        assert_eq!(status.pending[0].kind, "tab");
        assert_eq!(status.pending[0].since, 100);
        assert_eq!(status.pending[0].due_at, 700);
        assert!(status.pending[0].target.contains("Default"));
    }

    #[test]
    fn auto_spares_used_pinned_and_non_chrome_tabs() {
        let cfg = auto_config();
        for change in 0..5 {
            let mut snap = chrome_snapshot();
            let b = &mut snap.browsers[0];
            match change {
                0 => b.tabs[0].active = true,
                1 => b.tabs[0].pinned = true,
                2 => b.tabs[0].url = "https://www.google.com/search?q=rust".into(),
                3 => b.name = "Microsoft Edge".into(),
                _ => b.can_close_tabs = false,
            }
            let status = decide(&mut PolicyState::default(), &snap, &cfg, 100);
            assert!(status.pending.is_empty(), "case {change}");
        }
    }

    #[test]
    fn using_a_pending_tab_restarts_its_grace_period() {
        let cfg = auto_config();
        for change in 0..4 {
            let mut tracker = PolicyState::default();
            let mut snap = chrome_snapshot();
            decide(&mut tracker, &snap, &cfg, 100);
            match change {
                0 => snap.browsers[0].tabs[0].active = true,
                1 => snap.browsers[0].tabs[0].pinned = true,
                2 => snap.browsers[0].tabs[0].url = "https://example.com".into(),
                _ => snap.browsers[0].tabs.clear(),
            }
            assert!(decide(&mut tracker, &snap, &cfg, 200).pending.is_empty());
            let status = decide(&mut tracker, &chrome_snapshot(), &cfg, 300);
            assert_eq!(status.pending.len(), 1);
            assert_eq!(status.pending[0].due_at, 900);
        }
    }

    #[test]
    fn auto_off_cancels_tab_warnings_and_server_only_mode_includes_tabs() {
        let mut cfg = auto_config();
        cfg.auto.close_sessions = false;
        cfg.auto.stop_servers = true;
        let mut tracker = PolicyState::default();
        assert_eq!(
            decide(&mut tracker, &chrome_snapshot(), &cfg, 100)
                .pending
                .len(),
            1
        );
        cfg.auto.stop_servers = false;
        assert!(
            decide(&mut tracker, &chrome_snapshot(), &cfg, 200)
                .pending
                .is_empty()
        );
        assert!(tracker.pending.is_empty());
        cfg.auto.close_sessions = true;
        assert_eq!(
            decide(&mut tracker, &chrome_snapshot(), &cfg, 300).pending[0].due_at,
            900
        );
    }

    #[test]
    fn tab_activity_between_samples_and_profile_changes_reset_the_warning() {
        let cfg = auto_config();
        let mut tracker = PolicyState::default();
        let mut snap = chrome_snapshot();
        decide(&mut tracker, &snap, &cfg, 100);
        snap.browsers[0].tabs[0].last_active = Some(150);
        let status = decide(&mut tracker, &snap, &cfg, 200);
        assert_eq!(status.pending[0].due_at, 800);
        snap.browsers[0].tabs[0].profile = "Profile 2".into();
        let status = decide(&mut tracker, &snap, &cfg, 300);
        assert_eq!(status.pending[0].due_at, 900);
        assert_eq!(tracker.pending.len(), 1);
    }

    #[test]
    fn empty_tab_preview_waits_the_full_grace_and_reports_once() {
        let mut cfg = auto_config();
        cfg.auto.dry_run = true;
        let mut tracker = PolicyState::default();
        let snap = chrome_snapshot();
        assert_eq!(decide(&mut tracker, &snap, &cfg, 100).warned.len(), 1);
        let before = decide(&mut tracker, &snap, &cfg, 699);
        assert!(before.warned.is_empty());
        assert!(before.eligible.is_empty());
        let due = decide(&mut tracker, &snap, &cfg, 700);
        assert_eq!(due.eligible.len(), 1);
        assert!(matches!(due.eligible[0], Target::Tab { .. }));
        // The daemon removes the completed pending entry before execution.
        tracker.pending.remove(&due.eligible[0].key());
        tracker = serde_json::from_str(&serde_json::to_string(&tracker).unwrap()).unwrap();
        assert!(decide(&mut tracker, &snap, &cfg, 1300).eligible.is_empty());
        cfg.auto.dry_run = false;
        let live = decide(&mut tracker, &snap, &cfg, 1400);
        assert_eq!(live.warned.len(), 1);
        assert_eq!(live.pending[0].due_at, 2000);
        assert!(live.eligible.is_empty());
        assert!(tracker.dry_done.is_empty());
    }
}

#[cfg(test)]
mod domain_tab_tests {
    use super::*;
    use crate::{config::Config, daemon::Overrides, test_support::chrome_snapshot};

    fn fixture() -> (DaemonConfig, Snapshot) {
        let file: Config = toml::from_str(
            r#"
            auto_close_tabs = true
            auto_tab_inactive_hours = 24
            auto_tab_domains = [{ domain = "reddit.com", include_subdomains = true }]
        "#,
        )
        .unwrap();
        let cfg = DaemonConfig::from_config(&file, &Overrides::default());
        let mut snap = chrome_snapshot();
        snap.taken_at = 100_000;
        let tab = &mut snap.browsers[0].tabs[0];
        tab.url = "https://old.reddit.com/r/rust".into();
        tab.site = "old.reddit.com".into();
        tab.title = "Rust discussion".into();
        tab.last_active = Some(1);
        (cfg, snap)
    }

    #[test]
    fn domain_only_cleanup_waits_for_inactivity_and_full_warning() {
        let (cfg, snap) = fixture();
        let mut state = PolicyState::default();
        assert!(decide(&mut state, &snap, &cfg, 86_400).pending.is_empty());
        let first = decide(&mut state, &snap, &cfg, 100_000);
        assert_eq!(first.pending.len(), 1);
        assert!(first.pending[0].detail.contains("reddit.com"));
        assert!(first.eligible.is_empty());
        assert!(decide(&mut state, &snap, &cfg, 100_599).eligible.is_empty());
        assert_eq!(decide(&mut state, &snap, &cfg, 100_600).eligible.len(), 1);
    }

    #[test]
    fn domain_warning_is_cancelled_by_activity_identity_and_protection_changes() {
        for change in 0..8 {
            let (cfg, mut snap) = fixture();
            let mut state = PolicyState::default();
            assert_eq!(decide(&mut state, &snap, &cfg, 100_000).pending.len(), 1);
            let tab = &mut snap.browsers[0].tabs[0];
            match change {
                0 => tab.active = true,
                1 => tab.pinned = true,
                2 => tab.last_active = None,
                3 => tab.last_active = Some(100_100),
                4 => tab.url = "https://notreddit.com".into(),
                5 => tab.url = "https://old.reddit.com/new-page".into(),
                6 => tab.profile = "Profile 2".into(),
                _ => tab.window_id += 1,
            }
            let decision = decide(&mut state, &snap, &cfg, 100_600);
            assert!(decision.eligible.is_empty(), "case {change}");
            assert_eq!(decision.cancelled.len(), 1, "case {change}");
            if change >= 5 {
                assert_eq!(decision.pending[0].due_at, 101_200);
            }
        }
    }

    #[test]
    fn domain_preview_to_live_always_starts_a_new_warning() {
        let (mut cfg, snap) = fixture();
        cfg.auto.dry_run = true;
        let mut state = PolicyState::default();
        assert_eq!(decide(&mut state, &snap, &cfg, 100_000).pending.len(), 1);
        cfg.auto.dry_run = false;
        let live = decide(&mut state, &snap, &cfg, 100_599);
        assert!(live.eligible.is_empty());
        assert_eq!(live.pending[0].due_at, 101_199);
    }

    #[test]
    fn domain_rule_changes_and_persisted_warnings_require_fresh_grace() {
        let (mut cfg, snap) = fixture();
        let mut state = PolicyState::default();
        decide(&mut state, &snap, &cfg, 100_000);
        state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        cfg.auto.tab_inactive_secs = 3600;
        let changed = decide(&mut state, &snap, &cfg, 100_600);
        assert!(changed.eligible.is_empty());
        assert_eq!(changed.cancelled.len(), 1);
        assert_eq!(changed.pending[0].due_at, 101_200);
        cfg.auto.tab_domains.clear();
        assert!(decide(&mut state, &snap, &cfg, 101_200).eligible.is_empty());
        assert!(state.pending.is_empty());
        let (cfg, snap) = fixture();
        assert_eq!(
            decide(&mut state, &snap, &cfg, 101_201).pending[0].due_at,
            101_801
        );
    }
}

#[cfg(test)]
pub(crate) mod codex_tests {
    use super::*;
    use crate::{
        codex::{Backend, Task},
        config::Config,
        daemon::Overrides,
        test_support::snapshot,
    };

    pub(crate) fn fixture() -> (Snapshot, DaemonConfig) {
        let cfg = DaemonConfig::from_config(
            &Config {
                auto_archive_codex: true,
                stale_after_hours: 1.0,
                ..Config::default()
            },
            &Overrides::default(),
        );
        let mut snap = snapshot();
        snap.taken_at = 10000;
        let task = Task {
            backend_pid: 42,
            backend_created_at: 1,
            id: "old".into(),
            codex_home: "/codex".into(),
            revision: "old-revision".into(),
            name: "Old task".into(),
            cwd: "/project".into(),
            transcript: "/old".into(),
            updated_at: 100,
            state: "idle".into(),
            protection: None,
        };
        let newer = Task {
            id: "new".into(),
            updated_at: 200,
            ..task.clone()
        };
        snap.codex_backends.push(Backend {
            pid: 42,
            tasks: vec![task, newer],
            error: None,
        });
        (snap, cfg)
    }

    #[test]
    fn candidates_keep_newest_ties_and_reject_protected_or_missing_evidence() {
        let (mut snap, mut cfg) = fixture();
        assert_eq!(auto_codex_candidates(&snap, &cfg).len(), 1);
        let mut tie = snap.codex_backends[0].tasks[1].clone();
        tie.id = "tie".into();
        snap.codex_backends[0].tasks.push(tie);
        assert_eq!(auto_codex_candidates(&snap, &cfg)[0].id, "old");
        for reason in [
            "Pinned task",
            "Unfinished goal",
            "Queued work",
            "Linked to an automation",
            "Has child tasks",
        ] {
            snap.codex_backends[0].tasks[0].protection = Some(reason.into());
            assert!(auto_codex_candidates(&snap, &cfg).is_empty());
        }
        snap.codex_backends[0].tasks[0].protection = None;
        snap.codex_backends[0].tasks[0].state = "active".into();
        assert!(auto_codex_candidates(&snap, &cfg).is_empty());
        snap.codex_backends[0].tasks[0].state = "idle".into();
        snap.codex_backends[0].error = Some("disconnected".into());
        assert!(auto_codex_candidates(&snap, &cfg).is_empty());
        snap.codex_backends[0].error = None;
        cfg.thresholds.ignore_projects.push("/project".into());
        assert!(auto_codex_candidates(&snap, &cfg).is_empty());
        cfg.thresholds.ignore_projects.clear();
        cfg.auto.archive_codex = false;
        assert!(auto_codex_candidates(&snap, &cfg).is_empty());
    }

    #[test]
    fn inactivity_grace_activity_and_config_edits_require_new_warnings() {
        let (mut snap, mut cfg) = fixture();
        let mut state = PolicyState::default();
        snap.taken_at = 3699;
        assert!(decide(&mut state, &snap, &cfg, 3699).pending.is_empty());
        snap.taken_at = 3700;
        let first = decide(&mut state, &snap, &cfg, 3700);
        assert_eq!(first.pending[0].kind, "codex_task");
        assert_eq!(first.pending[0].due_at, 4300);
        assert!(decide(&mut state, &snap, &cfg, 4299).eligible.is_empty());
        state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(decide(&mut state, &snap, &cfg, 4300).eligible.len(), 1);
        snap.codex_backends[0].tasks[0].revision = "new activity".into();
        let changed = decide(&mut state, &snap, &cfg, 4301);
        assert_eq!(changed.cancelled.len(), 1);
        assert_eq!(changed.pending[0].due_at, 4901);
        cfg.auto.config_file_revision = "changed settings".into();
        let changed = decide(&mut state, &snap, &cfg, 4400);
        assert_eq!(changed.cancelled.len(), 1);
        assert_eq!(changed.pending[0].due_at, 5000);
        cfg.auto.archive_codex = false;
        assert_eq!(decide(&mut state, &snap, &cfg, 4500).cancelled.len(), 1);
        cfg.auto.archive_codex = true;
        assert_eq!(
            decide(&mut state, &snap, &cfg, 5000).pending[0].due_at,
            5600
        );
        snap.codex_backends[0].tasks.remove(1);
        assert!(
            decide(&mut state, &snap, &cfg, 5600).eligible.is_empty(),
            "retain sole remaining task"
        );
    }

    #[test]
    fn preview_and_archive_limit_and_later_tick() {
        let (mut snap, mut cfg) = fixture();
        let mut state = PolicyState::default();
        cfg.auto.dry_run = true;
        cfg.auto.grace = std::time::Duration::ZERO;
        let original = snap.codex_backends[0].tasks[0].clone();
        for id in ["old2", "old3"] {
            snap.codex_backends[0].tasks.push(Task {
                id: id.into(),
                ..original.clone()
            });
        }
        assert_eq!(decide(&mut state, &snap, &cfg, 10000).pending.len(), 3);
        let due = decide(&mut state, &snap, &cfg, 10001);
        assert_eq!(due.eligible.len(), 2);
        assert_eq!(due.pending.len(), 1);
        assert_eq!(decide(&mut state, &snap, &cfg, 10002).eligible.len(), 1);
        assert!(decide(&mut state, &snap, &cfg, 10003).eligible.is_empty());
        cfg.auto.dry_run = false;
        assert_eq!(
            decide(&mut state, &snap, &cfg, 10004).pending.len(),
            3,
            "leaving preview requires fresh warnings"
        );
    }
}
