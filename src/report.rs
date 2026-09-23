//! Plain-text rendering of a snapshot.

use crate::Snapshot;
use crate::agents::SessionState;
use crate::browser::{PageKind, TabInfo};
use crate::fmt::{bytes, dur, fit_left, fit_right, pct};
use crate::rules::Severity;
use crate::trends::Trend;
use std::fmt::Write;

pub fn render(s: &Snapshot) -> String {
    let mut o = String::new();
    let sys = &s.system;

    let _ = writeln!(o, "autotrim scan · {}", sys.os);
    let mut head = vec![
        format!("RAM {}", bytes(sys.total_mem)),
        format!("used {}", bytes(sys.used_mem)),
        format!(
            "swap {} / {} ({})",
            bytes(sys.used_swap),
            bytes(sys.total_swap),
            pct(sys.used_swap, sys.total_swap)
        ),
    ];
    if let Some(c) = sys.compressed {
        head.push(format!("compressed {}", bytes(c)));
    }
    if let Some(w) = sys.wired {
        head.push(format!("wired {}", bytes(w)));
    }
    head.push(format!("up {}", dur(sys.uptime_secs)));
    let _ = writeln!(o, "  {}", head.join(" · "));
    let _ = writeln!(
        o,
        "  cpu {:.0}% · load {:.1} {:.1} {:.1}",
        sys.cpu_pct, sys.load_one, sys.load_five, sys.load_fifteen
    );

    let _ = writeln!(o, "\nTop holders");
    let _ = writeln!(
        o,
        "  Totals include helper processes. Process footprints are not physical RAM savings."
    );
    for g in s.groups.iter().take(12) {
        let (count, unit) = match g.kind {
            crate::groups::GroupKind::Agent => {
                let sessions: Vec<_> = s
                    .sessions
                    .iter()
                    .filter(|x| g.pids.contains(&x.pid))
                    .collect();
                let n = sessions.len();
                let noun = if sessions.iter().all(|x| x.engine) {
                    if n == 1 { "backend" } else { "backends" }
                } else if n == 1 {
                    "session"
                } else {
                    "sessions"
                };
                let tasks = sessions
                    .iter()
                    .flat_map(|x| &x.threads)
                    .filter(|t| !t.helper)
                    .count();
                let noun = if tasks > 0 {
                    format!(
                        "{noun} · {tasks} observed task{}",
                        if tasks == 1 { "" } else { "s" }
                    )
                } else {
                    noun.to_string()
                };
                let unit = match &g.app {
                    Some(a) => format!("{noun} + the {a} app"),
                    None => noun.to_string(),
                };
                (n, unit)
            }
            _ => (g.procs, "procs".to_string()),
        };
        let _ = writeln!(
            o,
            "  {:>8} {:>5.1}%  {:<28} {:>4} {}",
            bytes(g.rss),
            g.cpu,
            fit_right(&g.name, 28),
            count,
            unit
        );
    }

    if !s.browsers.is_empty() {
        let _ = writeln!(o, "\nBrowsers");
        for b in &s.browsers {
            let mut parts = vec![
                bytes(b.rss),
                format!(
                    "{} renderers ({} tab-sized)",
                    b.renderers, b.tab_sized_renderers
                ),
                format!("{} extension", b.extension_renderers),
            ];
            if let Some(n) = b.profiles {
                parts.push(format!("{n} profiles"));
            }
            let _ = writeln!(o, "  {:<16} {}", b.name, parts.join(" · "));
            if b.tabs.is_empty() {
                if let Some(n) = &b.tabs_note {
                    let _ = writeln!(o, "  {:<16} tabs: {n}", "");
                }
                continue;
            }
            let profiles: Vec<String> = b
                .open_profiles
                .iter()
                .map(|p| format!("{} ({})", p.label, p.tabs))
                .collect();
            let mut line = format!(
                "{} tabs in {} windows · {}",
                b.tabs.len(),
                b.windows,
                profiles.join(", ")
            );
            if b.stale_tabs > 0 {
                line.push_str(&format!(" · {} stale", b.stale_tabs));
            }
            if let Some(e) = b.per_tab_estimate {
                line.push_str(&format!(" · ≈{} per tab (renderers ÷ tabs)", bytes(e)));
            }
            let _ = writeln!(o, "  {:<16} {line}", "");
            let sites: Vec<String> = b
                .sites
                .iter()
                .take(6)
                .map(|st| {
                    if st.stale_tabs > 0 {
                        format!("{} {} ({} stale)", st.site, st.tabs, st.stale_tabs)
                    } else {
                        format!("{} {}", st.site, st.tabs)
                    }
                })
                .collect();
            let _ = writeln!(o, "  {:<16} sites: {}", "", sites.join(" · "));
            let mut pages = Vec::new();
            if b.chat_tabs > 0 {
                let on: Vec<String> = b
                    .sites
                    .iter()
                    .filter(|st| st.kind == PageKind::Chat)
                    .map(|st| format!("{} {}", st.site, st.tabs))
                    .collect();
                pages.push(format!(
                    "{} conversation{} ({} stale): {}",
                    b.chat_tabs,
                    if b.chat_tabs == 1 { "" } else { "s" },
                    b.stale_chat_tabs,
                    on.join(", ")
                ));
            }
            let local: Vec<String> = b
                .sites
                .iter()
                .filter(|st| st.kind == PageKind::Local)
                .map(|st| format!("{} {}", st.site, st.tabs))
                .collect();
            if !local.is_empty() {
                pages.push(format!("local apps: {}", local.join(", ")));
            }
            if !pages.is_empty() {
                let _ = writeln!(o, "  {:<16} pages: {}", "", pages.join(" · "));
            }
            let mut growing: Vec<&Trend> = s
                .trends
                .iter()
                .filter(|t| t.kind == "renderer" && t.growth > 0 && t.span_secs >= 10 * 60)
                .filter(|t| {
                    t.key
                        .split(':')
                        .nth(1)
                        .and_then(|p| p.parse::<u32>().ok())
                        .is_some_and(|pid| b.renderer_procs.iter().any(|r| r.pid == pid))
                })
                .collect();
            growing.sort_by_key(|t| std::cmp::Reverse(t.growth));
            for t in growing.iter().take(3) {
                let _ = writeln!(
                    o,
                    "  {:<16} growing: {} +{} over {} ({}/h) · now {} · Chrome does not say which tab",
                    "",
                    t.name.trim_start_matches(b.name.as_str()).trim(),
                    bytes(t.growth as u64),
                    dur(t.span_secs),
                    bytes(t.bytes_per_hour.max(0.0) as u64),
                    bytes(t.rss_now)
                );
            }
            let mut oldest: Vec<&TabInfo> = b
                .tabs
                .iter()
                .filter(|t| !t.active && t.idle_secs.is_some())
                .collect();
            oldest.sort_by_key(|t| std::cmp::Reverse(t.idle_secs));
            for t in oldest.iter().take(5) {
                let _ = writeln!(
                    o,
                    "  {:<16} {:>8}  {:<44} {}",
                    "",
                    dur(t.idle_secs.unwrap_or(0)),
                    fit_right(&t.title, 44),
                    t.site
                );
            }
        }
    }

    let _ = writeln!(o, "\nAgent sessions ({})", s.sessions.len());
    if s.sessions.is_empty() {
        let _ = writeln!(o, "  none found");
    } else {
        let _ = writeln!(
            o,
            "  {:<12} {:<12} {:<32} {:>8} {:>6} {:>8} {:<11} STATE",
            "AGENT", "HOST", "PROJECT", "AGE", "CPU", "MEM", "PORTS"
        );
        for x in &s.sessions {
            let tag = if x.is_self { " (this scan)" } else { "" };
            let since = match (x.idle_secs, x.quiet_for_secs) {
                (Some(i), _) => Some(format!("idle {}", dur(i))),
                (None, Some(q)) if q > 0 => Some(format!("quiet {}", dur(q))),
                _ => None,
            };
            let state = match (x.state, since) {
                (SessionState::Active, _) => "active".to_string(),
                (SessionState::Idle, Some(s)) => s,
                (SessionState::Idle, None) => "idle".to_string(),
                (SessionState::Stale, Some(s)) => format!("stale · {s}"),
                (SessionState::Stale, None) => "stale".to_string(),
            };
            let ports = if x.ports.is_empty() {
                "-".to_string()
            } else {
                x.ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let _ = writeln!(
                o,
                "  {:<12} {:<12} {:<32} {:>8} {:>5.1}% {:>8} {:<11} {}{}",
                fit_right(x.kind.label(), 12),
                fit_right(&x.host, 12),
                fit_left(x.project.as_deref().unwrap_or("?"), 32),
                dur(x.age_secs),
                x.cpu_window_mean.unwrap_or(x.cpu),
                bytes(x.rss),
                fit_right(&ports, 11),
                state,
                tag
            );
            if !x.threads.is_empty() {
                let _ = writeln!(
                    o,
                    "    Observed task transcripts (shared memory above; loaded state is unverified)"
                );
                for t in &x.threads {
                    let name = t
                        .name
                        .as_deref()
                        .or(t.first_prompt.as_deref())
                        .or(t.id.as_deref())
                        .unwrap_or("Unnamed task");
                    let activity = t
                        .last_activity
                        .map(|ts| {
                            format!("last activity {} ago", dur(s.taken_at.saturating_sub(ts)))
                        })
                        .unwrap_or_else(|| "activity unknown".to_string());
                    let _ = writeln!(
                        o,
                        "      {}{} · {} · {}",
                        if t.helper { "[helper] " } else { "" },
                        name,
                        t.cwd.as_deref().unwrap_or("project unknown"),
                        activity
                    );
                }
            }
        }
    }

    if !s.ports.is_empty() {
        let _ = writeln!(o, "\nListening ports ({})", s.ports.len());
        let _ = writeln!(
            o,
            "  {:>5} {:<5} {:<12} {:>8}  {:<40} {:<24} PROCESS",
            "PORT", "PROTO", "ADDR", "OPEN", "WHAT", "OWNER"
        );
        for p in &s.ports {
            let _ = writeln!(
                o,
                "  {:>5} {:<5} {:<12} {:>8}  {:<40} {:<24} {} ({})",
                p.port,
                p.protocol,
                fit_right(&p.addr, 12),
                dur(p.open_for_secs),
                fit_right(p.label.as_deref().unwrap_or("?"), 40),
                fit_right(&p.owner, 24),
                p.process,
                p.pid
            );
        }
    }

    let shown: Vec<&Trend> = s
        .trends
        .iter()
        .filter(|t| t.kind != "renderer" && t.span_secs >= 10 * 60)
        .take(8)
        .collect();
    if !shown.is_empty() {
        let span = shown.iter().map(|t| t.span_secs).max().unwrap_or(0);
        let _ = writeln!(o, "\nTrends (last {})", dur(span));
        let _ = writeln!(
            o,
            "  {:<36} {:>8} {:>9} {:>10} {:>7}  STEADY",
            "NAME", "NOW", "CHANGE", "RATE/H", "CPU"
        );
        for t in shown {
            let sign = if t.growth >= 0 { "+" } else { "-" };
            let _ = writeln!(
                o,
                "  {:<36} {:>8} {:>9} {:>10} {:>6.0}%  {:.0}%",
                fit_right(&t.name, 36),
                bytes(t.rss_now),
                format!("{sign}{}", bytes(t.growth.unsigned_abs())),
                format!("{sign}{}", bytes(t.bytes_per_hour.abs() as u64)),
                t.cpu_mean,
                t.rising_frac * 100.0
            );
        }
    }

    if let Some(a) = &s.auto {
        let _ = writeln!(o, "\nAuto mode · {}", a.describe());
        for p in &a.pending {
            let _ = writeln!(
                o,
                "  {} · {} · {} in {}",
                p.target,
                p.detail,
                if a.dry_run { "would close" } else { "closing" },
                dur(p.due_at.saturating_sub(s.taken_at))
            );
        }
    }

    let _ = writeln!(o, "\nAdvice");
    if s.advice.is_empty() {
        let _ = writeln!(o, "  nothing to do");
    }
    for a in &s.advice {
        let sev = match a.severity {
            Severity::High => "high",
            Severity::Medium => "med ",
            Severity::Low => "low ",
        };
        let _ = writeln!(o, "  [{sev}] {}", a.title);
        if !a.evidence.is_empty() {
            let _ = writeln!(o, "         {}", a.evidence.join("\n         "));
        }
        let _ = writeln!(o, "         -> {}", a.action);
    }
    o
}
