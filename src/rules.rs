//! Deterministic advice. Same snapshot in, same advice out.

use crate::agents::{AgentSession, SessionState};
use crate::browser::BrowserInfo;
use crate::fmt;
use crate::groups::{AppGroup, GroupKind};
use crate::system::SystemInfo;
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
    pub heavy_app_bytes: u64,
    pub pressure_swap_frac: f64,
    /// Window-mean CPU below this counts as quiet (daemon mode).
    pub quiet_cpu: f32,
    /// A session must be observed quiet at least this long before it is
    /// called stale, however old it is. Keeps a freshly started daemon from
    /// judging anything in its first minutes.
    pub min_quiet_secs: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            restart_uptime_secs: 14 * 86_400,
            restart_swap_frac: 0.75,
            stale_after_secs: 6 * 3_600,
            browser_renderers: 50,
            browser_profiles: 2,
            heavy_app_bytes: 600 * 1024 * 1024,
            pressure_swap_frac: 0.5,
            quiet_cpu: 2.0,
            min_quiet_secs: 15 * 60,
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

pub fn evaluate(
    sys: &SystemInfo,
    groups: &[AppGroup],
    sessions: &[AgentSession],
    browsers: &[BrowserInfo],
    t: &Thresholds,
) -> Vec<Advice> {
    let mut out = Vec::new();
    let under_pressure = sys.swap_frac() >= t.pressure_swap_frac;

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
        .filter(|s| s.state == SessionState::Stale && !s.is_self)
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

    // 3. Browser sprawl.
    for b in browsers {
        let many_tabs = b.renderers >= t.browser_renderers;
        let many_profiles = b.profiles.map(|n| n > t.browser_profiles).unwrap_or(false);
        if !(many_tabs || many_profiles) {
            continue;
        }
        let mut evidence = vec![format!(
            "{} across {} processes",
            fmt::bytes(b.rss),
            b.procs
        )];
        let mut actions = Vec::new();
        if many_tabs {
            evidence.push(format!("{} live tab renderers", b.renderers));
            actions.push("set Memory Saver to Maximum (chrome://settings/performance)");
        }
        if let Some(n) = b.profiles
            && many_profiles
        {
            evidence.push(format!(
                "{n} profiles, each with its own extension and utility processes"
            ));
            actions.push("consolidate to one or two profiles");
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

    out.sort_by_key(|a| a.severity);
    out
}
