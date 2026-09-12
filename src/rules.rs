//! Deterministic advice. Same snapshot in, same advice out.

use crate::agents::{AgentSession, SessionState};
use crate::browser::{BrowserInfo, PageKind};
use crate::fmt;
use crate::groups::{AppGroup, GroupKind};
use crate::ports::PortInfo;
use crate::system::SystemInfo;
use crate::trends::Trend;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    High,
    Medium,
    Low,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Advice {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub evidence: Vec<String>,
    pub action: String,
    /// Bytes expected back if the action is taken, where that is honest to estimate.
    pub recovery: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Thresholds {
    pub restart_uptime_secs: u64,
    pub restart_swap_frac: f64,
    pub stale_after_secs: u64,
    pub browser_renderers: usize,
    pub browser_profiles: usize,
    /// A tab not looked at for this long counts as stale.
    pub tab_stale_after_secs: u64,
    /// Browser advice also fires at this many stale tabs.
    pub browser_stale_tabs: usize,
    /// Conversation advice fires at this many stale chat-UI tabs.
    pub chat_stale_tabs: usize,
    pub heavy_app_bytes: u64,
    pub pressure_swap_frac: f64,
    /// Window-mean CPU below this counts as quiet (daemon mode).
    pub quiet_cpu: f32,
    /// A session must be observed quiet at least this long before it is
    /// called stale, however old it is. Keeps a freshly started daemon from
    /// judging anything in its first minutes.
    pub min_quiet_secs: u64,
    /// A quiet, unmanaged local server open longer than this is reported.
    pub port_stale_after_secs: u64,
    pub trend_window_secs: u64,
    pub growth_bytes_per_hour: u64,
    pub growth_min_bytes: u64,
    pub cpu_hog_pct: f32,
    pub cpu_hog_secs: u64,
    pub pressure_rise_bytes_per_hour: u64,
    pub probe_ports: bool,
    pub probe_timeout_ms: u64,
    pub ignore_ports: Vec<u16>,
    pub ignore_apps: Vec<String>,
    pub ignore_projects: Vec<String>,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            restart_uptime_secs: 14 * 86_400,
            restart_swap_frac: 0.75,
            stale_after_secs: 6 * 3_600,
            browser_renderers: 50,
            browser_profiles: 2,
            tab_stale_after_secs: 24 * 3_600,
            browser_stale_tabs: 15,
            chat_stale_tabs: 2,
            heavy_app_bytes: 600 * 1024 * 1024,
            pressure_swap_frac: 0.5,
            quiet_cpu: 2.0,
            min_quiet_secs: 15 * 60,
            port_stale_after_secs: 24 * 3_600,
            trend_window_secs: 2 * 3_600,
            growth_bytes_per_hour: 200 * 1024 * 1024,
            growth_min_bytes: 150 * 1024 * 1024,
            cpu_hog_pct: 90.0,
            cpu_hog_secs: 10 * 60,
            pressure_rise_bytes_per_hour: 1024 * 1024 * 1024,
            probe_ports: true,
            probe_timeout_ms: 300,
            ignore_ports: Vec::new(),
            ignore_apps: Vec::new(),
            ignore_projects: Vec::new(),
        }
    }
}

/// Decide what a session is doing, from the best evidence available:
///
/// 1. Busy CPU right now (window mean when the daemon has one, otherwise the
///    instantaneous sample) means active, whatever the transcript says.
/// 2. A transcript with a last real message is trusted immediately: stale
///    when that message is older than the threshold.
/// 3. Without a transcript, the daemon's quiet window decides, and it needs
///    a minimum observed quiet period before calling anything stale.
/// 4. A one-shot scan with neither falls back to age alone.
pub fn session_state(s: &AgentSession, t: &Thresholds) -> SessionState {
    // A one-shot CPU sample on a busy machine jitters by a few percent, so
    // it only overrides transcript evidence when it is unmistakably busy.
    // The daemon's window mean is trusted at the normal threshold.
    let busy = match s.cpu_window_mean {
        Some(mean) => mean >= t.quiet_cpu,
        None if s.idle_secs.is_some() => s.cpu >= t.quiet_cpu.max(10.0),
        None => s.cpu >= t.quiet_cpu,
    };
    if busy {
        return SessionState::Active;
    }
    if let Some(idle) = s.idle_secs {
        return if idle >= t.stale_after_secs {
            SessionState::Stale
        } else {
            SessionState::Idle
        };
    }
    match (s.cpu_window_mean, s.quiet_for_secs) {
        // Watched long enough, and quiet long enough.
        (Some(_), Some(q)) if s.age_secs >= t.stale_after_secs && q >= t.min_quiet_secs => {
            SessionState::Stale
        }
        // Being watched, but the window is not warm or the quiet is too short.
        (_, Some(_)) => SessionState::Idle,
        // One-shot scan with nothing better than age.
        (None, None) if s.age_secs >= t.stale_after_secs => SessionState::Stale,
        _ => SessionState::Idle,
    }
}

/// Renderers are not tabs: every cross-site frame, prerender, and the spare
/// renderer gets a process too. Say both numbers when the tabs are known.
pub fn browser_renderer_line(b: &BrowserInfo) -> String {
    if b.tabs.is_empty() {
        format!(
            "{} renderer processes: {} tab-sized, {} small (cross-site frames, prerenders, a spare)",
            b.renderers, b.tab_sized_renderers, b.small_renderers
        )
    } else {
        format!(
            "{} renderer processes for {} tabs in {} windows; the other {} are cross-site frames, prerenders and a spare",
            b.renderers,
            b.tabs.len(),
            b.windows,
            b.renderers.saturating_sub(b.tabs.len())
        )
    }
}

// Every input is a distinct kind of evidence; a struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    sys: &SystemInfo,
    groups: &[AppGroup],
    sessions: &[AgentSession],
    browsers: &[BrowserInfo],
    ports: &[PortInfo],
    trends: &[Trend],
    swap_growth: Option<(i64, u64)>,
    t: &Thresholds,
) -> Vec<Advice> {
    let mut out = Vec::new();
    let under_pressure = sys.swap_frac() >= t.pressure_swap_frac;
    let ignored_project = |s: &AgentSession| {
        let path = s.cwd.as_deref().or(s.project.as_deref()).unwrap_or("");
        t.ignore_projects
            .iter()
            .any(|p| !p.is_empty() && path.contains(p.as_str()))
    };
    let ignored_app = |name: &str| t.ignore_apps.iter().any(|a| a == name);

    // 1. Long uptime with swap full: the machine needs a restart.
    if sys.uptime_secs >= t.restart_uptime_secs && sys.swap_frac() >= t.restart_swap_frac {
        let mut evidence = vec![
            format!("up {}", fmt::dur(sys.uptime_secs)),
            format!(
                "swap {} of {} ({})",
                fmt::bytes(sys.used_swap),
                fmt::bytes(sys.total_swap),
                fmt::pct(sys.used_swap, sys.total_swap)
            ),
        ];
        if let Some(c) = sys.compressed {
            evidence.push(format!("compressed {}", fmt::bytes(c)));
        }
        if let Some(w) = sys.wired {
            evidence.push(format!("wired {}", fmt::bytes(w)));
        }
        out.push(Advice {
            id: "restart".to_string(),
            severity: Severity::High,
            title: "Restart this machine".to_string(),
            evidence,
            action: "Restart, not just log out, when convenient. Swap and compressed memory are rebuilt from scratch.".to_string(),
            recovery: None,
        });
    }

    // 2. Stale agent sessions.
    let stale: Vec<&AgentSession> = sessions
        .iter()
        .filter(|s| s.state == SessionState::Stale && !s.is_self && !ignored_project(s))
        .collect();
    if !stale.is_empty() {
        let total: u64 = stale.iter().map(|s| s.rss).sum();
        let mut evidence = Vec::new();
        for s in stale.iter().take(5) {
            let since = match (s.idle_secs, s.quiet_for_secs) {
                (Some(i), _) => format!("idle {}", fmt::dur(i)),
                (None, Some(q)) => format!("quiet {}", fmt::dur(q)),
                (None, None) => format!("age {}", fmt::dur(s.age_secs)),
            };
            evidence.push(format!(
                "{} · {} · {} · {} · {}",
                s.kind.label(),
                s.host,
                s.session_name
                    .as_deref()
                    .or(s.project.as_deref())
                    .unwrap_or("?"),
                since,
                fmt::bytes(s.rss)
            ));
        }
        if stale.len() > 5 {
            evidence.push(format!("and {} more", stale.len() - 5));
        }
        let severity = if total >= 1024 * 1024 * 1024 {
            Severity::High
        } else {
            Severity::Medium
        };
        out.push(Advice {
            id: "stale_sessions".to_string(),
            severity,
            title: format!(
                "Close {} stale agent session{} holding {}",
                stale.len(),
                if stale.len() == 1 { "" } else { "s" },
                fmt::bytes(total)
            ),
            evidence,
            action: "Close them from the app that launched them. Transcripts stay on disk and the sessions can be resumed.".to_string(),
            recovery: Some(total),
        });
    }

    // 3. Browser sprawl: too many renderers, too many profiles, or too many
    //    tabs nobody has looked at in a day.
    for b in browsers.iter().filter(|b| !ignored_app(&b.name)) {
        // Real tabs when the session files could be read, otherwise the
        // renderers big enough to be one.
        let tab_like = if b.tabs.is_empty() {
            b.tab_sized_renderers
        } else {
            b.tabs.len()
        };
        let many_tabs = tab_like >= t.browser_renderers;
        let many_profiles = b.profiles.map(|n| n > t.browser_profiles).unwrap_or(false);
        let many_stale = b.stale_tabs >= t.browser_stale_tabs;
        if !(many_tabs || many_profiles || many_stale) {
            continue;
        }
        let mut evidence = vec![format!(
            "{} across {} processes",
            fmt::bytes(b.rss),
            b.procs
        )];
        let mut actions: Vec<String> = Vec::new();
        if !b.tabs.is_empty() {
            evidence.push(format!(
                "{} tabs open in {} profile{}",
                b.tabs.len(),
                b.open_profiles.len(),
                if b.open_profiles.len() == 1 { "" } else { "s" }
            ));
        }
        if many_tabs {
            evidence.push(browser_renderer_line(b));
            actions.push("set Memory Saver to Maximum (chrome://settings/performance)".to_string());
        }
        if many_stale {
            let worst: Vec<String> = b
                .sites
                .iter()
                .filter(|s| s.stale_tabs > 0)
                .take(3)
                .map(|s| format!("{} {}", s.site, s.stale_tabs))
                .collect();
            evidence.push(format!(
                "{} not looked at in over {}: {}",
                b.stale_tabs,
                fmt::dur(t.tab_stale_after_secs),
                worst.join(", ")
            ));
            let est = b
                .per_tab_estimate
                .map(|p| {
                    format!(
                        ", roughly {} at the average renderer size",
                        fmt::bytes(p * b.stale_tabs as u64)
                    )
                })
                .unwrap_or_default();
            actions.push(format!(
                "close the {} stale tabs from the browser view{est}",
                b.stale_tabs
            ));
        }
        if let Some(n) = b.profiles
            && many_profiles
        {
            evidence.push(format!(
                "{n} profiles, each with its own extension and utility processes"
            ));
            actions.push("consolidate to one or two profiles".to_string());
        }
        out.push(Advice {
            id: "browser_sprawl".to_string(),
            severity: Severity::Medium,
            title: format!("{} is holding {}", b.name, fmt::bytes(b.rss)),
            evidence,
            action: actions.join("; "),
            recovery: None,
        });
    }

    // 3b. Conversation pages left open. A chat UI keeps the whole exchange
    //     in the page, so a long conversation grows the way a leak does and
    //     a background tab never gives it back. The service keeps the
    //     conversation, so closing the tab loses nothing.
    for b in browsers.iter().filter(|b| !ignored_app(&b.name)) {
        let n = b.stale_chat_tabs;
        if n == 0 || n < t.chat_stale_tabs {
            continue;
        }
        let sites: Vec<String> = b
            .sites
            .iter()
            .filter(|s| s.kind == PageKind::Chat && s.stale_tabs > 0)
            .map(|s| {
                let oldest = s
                    .oldest_idle_secs
                    .map(|o| format!(" (oldest {})", fmt::dur(o)))
                    .unwrap_or_default();
                format!("{} {}{oldest}", s.site, s.stale_tabs)
            })
            .collect();
        let mut evidence = vec![
            format!(
                "{} of {} conversation tab{} not looked at in over {}: {}",
                n,
                b.chat_tabs,
                if b.chat_tabs == 1 { "" } else { "s" },
                fmt::dur(t.tab_stale_after_secs),
                sites.join(", ")
            ),
            "a conversation page keeps the whole exchange in the page and grows with it; in the background it never gives that back".to_string(),
        ];
        if let Some(p) = b.per_tab_estimate {
            evidence.push(format!(
                "at least {} at the average renderer size; conversation pages usually run well above it",
                fmt::bytes(p * n as u64)
            ));
        }
        out.push(Advice {
            id: "conversation_tabs".to_string(),
            severity: Severity::Medium,
            title: format!(
                "Close {} stale conversation tab{} in {}",
                n,
                if n == 1 { "" } else { "s" },
                b.name
            ),
            evidence,
            action: "Close them from the browser view. The services keep the conversation history on their side (temporary chats excepted), so each one reopens where it was.".to_string(),
            recovery: None,
        });
    }

    // 4. Under pressure, name the heaviest ordinary app. Apps that host agent
    //    sessions are skipped: quitting them is covered by the stale-session
    //    advice, and quitting the host of a live session would be destructive.
    if under_pressure {
        let hosting: HashSet<&str> = sessions
            .iter()
            .filter_map(|s| s.host_app.as_deref())
            .collect();
        if let Some(g) = groups
            .iter()
            .filter(|g| g.kind == GroupKind::App)
            .filter(|g| browsers.iter().all(|b| b.name != g.name))
            .filter(|g| !hosting.contains(g.name.as_str()))
            .filter(|g| !ignored_app(&g.name))
            .find(|g| g.rss >= t.heavy_app_bytes)
        {
            out.push(Advice {
                id: "heavy_app".to_string(),
                severity: Severity::Low,
                title: format!("{} is holding {}", g.name, fmt::bytes(g.rss)),
                evidence: vec![
                    format!("{} processes", g.procs),
                    format!("swap is {} full", fmt::pct(sys.used_swap, sys.total_swap)),
                ],
                action: "Quit it if you are not using it right now.".to_string(),
                recovery: Some(g.rss),
            });
        }
    }

    // 5. Old local servers: a quiet, unmanaged listener that has been open
    //    for a long time. One entry per owning process.
    let mut seen_pid = HashSet::new();
    let old: Vec<&PortInfo> = ports
        .iter()
        .filter(|p| !p.owner_managed)
        .filter(|p| !t.ignore_ports.contains(&p.port))
        .filter(|p| p.open_for_secs >= t.port_stale_after_secs)
        .filter(|p| p.owner_cpu < t.quiet_cpu)
        .filter(|p| seen_pid.insert(p.pid))
        .collect();
    if !old.is_empty() {
        let total: u64 = old.iter().map(|p| p.owner_rss).sum();
        let mut evidence: Vec<String> = old
            .iter()
            .take(5)
            .map(|p| {
                let all_ports: Vec<String> = ports
                    .iter()
                    .filter(|q| q.pid == p.pid)
                    .map(|q| format!("{}:{}", q.addr, q.port))
                    .collect();
                format!(
                    "{} · {} · open {} · {}",
                    p.process,
                    all_ports.join(", "),
                    fmt::dur(p.open_for_secs),
                    fmt::bytes(p.owner_rss)
                )
            })
            .collect();
        if old.len() > 5 {
            evidence.push(format!("and {} more", old.len() - 5));
        }
        out.push(Advice {
            id: "old_servers".to_string(),
            severity: if total >= 300 * 1024 * 1024 {
                Severity::Medium
            } else {
                Severity::Low
            },
            title: format!(
                "Stop {} old local server{} holding {}",
                old.len(),
                if old.len() == 1 { "" } else { "s" },
                fmt::bytes(total)
            ),
            evidence,
            action: "Stop them if nothing needs them. They were started from a terminal or script, not an app, and will not come back on their own.".to_string(),
            recovery: Some(total),
        });
    }

    // 6. Leak-like growth: steady, fast, and substantial.
    let min_span = (t.trend_window_secs / 4).max(20 * 60);
    let leak_like = |tr: &&Trend| {
        tr.span_secs >= min_span
            && tr.samples >= 8
            && tr.growth >= t.growth_min_bytes as i64
            && tr.bytes_per_hour >= t.growth_bytes_per_hour as f64
            && tr.rising_frac >= 0.6
            && tr.r2 >= 0.7
    };
    let growers: Vec<&Trend> = trends
        .iter()
        .filter(|tr| tr.kind != "renderer")
        .filter(|tr| leak_like(tr))
        .filter(|tr| !ignored_app(&tr.name))
        .collect();
    for tr in growers.iter().take(3) {
        let severity = if tr.bytes_per_hour >= 1024.0 * 1024.0 * 1024.0 {
            Severity::High
        } else {
            Severity::Medium
        };
        out.push(Advice {
            id: format!("growth:{}", tr.key),
            severity,
            title: format!(
                "{} grew {} in {} ({}/h)",
                tr.name,
                fmt::bytes(tr.growth.max(0) as u64),
                fmt::dur(tr.span_secs),
                fmt::bytes(tr.bytes_per_hour.max(0.0) as u64)
            ),
            evidence: vec![
                format!("now {} · was {}", fmt::bytes(tr.rss_now), fmt::bytes(tr.rss_start)),
                format!(
                    "rose in {:.0}% of samples · fit {:.2}",
                    tr.rising_frac * 100.0,
                    tr.r2
                ),
            ],
            action: "Steady growth that never comes back down is a leak. If you are not actively using it, quit and reopen it.".to_string(),
            recovery: Some(tr.growth.max(0) as u64),
        });
    }

    // 6b. One browser page growing steadily. Chrome does not say which tab
    //     a renderer process is, so the advice names the process and lists
    //     the long-lived pages that are open: conversations and local apps
    //     are the pages that grow with use.
    let pages: Vec<(&Trend, &BrowserInfo, u32)> = trends
        .iter()
        .filter(|tr| tr.kind == "renderer")
        .filter(|tr| leak_like(tr))
        .filter_map(|tr| {
            let pid: u32 = tr.key.split(':').nth(1)?.parse().ok()?;
            let b = browsers
                .iter()
                .find(|b| b.renderer_procs.iter().any(|r| r.pid == pid))?;
            (!ignored_app(&b.name)).then_some((tr, b, pid))
        })
        .collect();
    for (tr, b, pid) in pages.iter().take(2) {
        let mut evidence = vec![
            format!(
                "one renderer process (pid {pid}) · now {} · was {} · rose in {:.0}% of samples",
                fmt::bytes(tr.rss_now),
                fmt::bytes(tr.rss_start),
                tr.rising_frac * 100.0
            ),
            "Chrome does not say which tab a process is; pages that grow like this are the ones that stay open and keep working: conversations, mail, editors, local apps".to_string(),
        ];
        let candidates: Vec<String> = b
            .sites
            .iter()
            .filter(|s| s.kind != PageKind::Page)
            .map(|s| {
                let idle = s
                    .oldest_idle_secs
                    .map(|o| format!(", oldest {}", fmt::dur(o)))
                    .unwrap_or_default();
                let what = match s.kind {
                    PageKind::Chat => "conversation",
                    PageKind::Local => "local app",
                    PageKind::Page => "page",
                };
                format!("{} {} ({what}{idle})", s.site, s.tabs)
            })
            .collect();
        if !candidates.is_empty() {
            evidence.push(format!("open now: {}", candidates.join(", ")));
        }
        out.push(Advice {
            id: format!("page_growth:{}", tr.key),
            severity: if tr.bytes_per_hour >= 1024.0 * 1024.0 * 1024.0 {
                Severity::High
            } else {
                Severity::Medium
            },
            title: format!(
                "A {} page grew {} in {} ({}/h)",
                b.name,
                fmt::bytes(tr.growth.max(0) as u64),
                fmt::dur(tr.span_secs),
                fmt::bytes(tr.bytes_per_hour.max(0.0) as u64)
            ),
            evidence,
            action: "If it is a conversation or app page you are done with, close it from the browser view. If you still need it, reloading the tab starts the page over and gives the growth back.".to_string(),
            recovery: Some(tr.growth.max(0) as u64),
        });
    }

    // 7. Sustained CPU.
    let hogs: Vec<&Trend> = trends
        .iter()
        .filter(|tr| tr.kind != "renderer")
        .filter(|tr| tr.cpu_span_secs >= t.cpu_hog_secs * 8 / 10)
        .filter(|tr| tr.cpu_mean >= t.cpu_hog_pct && tr.cpu_min >= t.cpu_hog_pct / 2.0)
        .filter(|tr| !ignored_app(&tr.name))
        .collect();
    for tr in hogs.iter().take(3) {
        out.push(Advice {
            id: format!("cpu:{}", tr.key),
            severity: Severity::Low,
            title: format!(
                "{} has used {:.1} cores for {}",
                tr.name,
                tr.cpu_mean / 100.0,
                fmt::dur(tr.cpu_span_secs)
            ),
            evidence: vec![format!(
                "mean {:.0}% · never below {:.0}% in that time",
                tr.cpu_mean, tr.cpu_min
            )],
            action: "Check what it is doing. A build or an agent working is normal; something spinning with nothing to show is not.".to_string(),
            recovery: None,
        });
    }

    // 8. Pressure rising: swap climbing fast, with the likely causes named.
    if let Some((delta, span)) = swap_growth
        && span >= 15 * 60
    {
        let per_hour = delta as f64 * 3600.0 / span as f64;
        if per_hour >= t.pressure_rise_bytes_per_hour as f64 {
            let mut evidence = vec![format!(
                "swap +{} in {} · now {} of {}",
                fmt::bytes(delta.max(0) as u64),
                fmt::dur(span),
                fmt::bytes(sys.used_swap),
                fmt::bytes(sys.total_swap)
            )];
            for tr in trends
                .iter()
                .filter(|tr| tr.kind != "renderer" && tr.growth > 0)
                .take(3)
            {
                evidence.push(format!(
                    "{} +{} ({}/h)",
                    tr.name,
                    fmt::bytes(tr.growth as u64),
                    fmt::bytes(tr.bytes_per_hour.max(0.0) as u64)
                ));
            }
            out.push(Advice {
                id: "pressure_rising".to_string(),
                severity: Severity::High,
                title: format!(
                    "Memory pressure rising: swap +{} in {}",
                    fmt::bytes(delta.max(0) as u64),
                    fmt::dur(span)
                ),
                evidence,
                action: "The fastest-growing apps above are the likely cause. Close what you can spare before the machine starts to crawl.".to_string(),
                recovery: None,
            });
        }
    }

    out.sort_by_key(|a| a.severity);
    out
}
