//! Plain-text rendering of a snapshot.

use crate::Snapshot;
use crate::agents::SessionState;
use crate::fmt::{bytes, dur, fit_left, fit_right, pct};
use crate::rules::Severity;
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
    if let Some(f) = sys.free_pct {
        head.push(format!("free {f}%"));
    }
    head.push(format!("up {}", dur(sys.uptime_secs)));
    let _ = writeln!(o, "  {}", head.join(" · "));

    let _ = writeln!(o, "\nTop holders");
    for g in s.groups.iter().take(12) {
        let unit = match g.kind {
            crate::groups::GroupKind::Agent => "sessions",
            _ => "procs",
        };
        let _ = writeln!(
            o,
            "  {:>8}  {:<28} {:>4} {}",
            bytes(g.rss),
            fit_right(&g.name, 28),
            g.procs,
            unit
        );
    }

    if !s.browsers.is_empty() {
        let _ = writeln!(o, "\nBrowsers");
        for b in &s.browsers {
            let mut parts = vec![
                bytes(b.rss),
                format!("{} tab renderers", b.renderers),
                format!("{} extension", b.extension_renderers),
            ];
            if let Some(n) = b.profiles {
                parts.push(format!("{n} profiles"));
            }
            let _ = writeln!(o, "  {:<16} {}", b.name, parts.join(" · "));
        }
    }

    let _ = writeln!(o, "\nAgent sessions ({})", s.sessions.len());
    if s.sessions.is_empty() {
        let _ = writeln!(o, "  none found");
    } else {
        let _ = writeln!(
            o,
            "  {:<12} {:<12} {:<32} {:>8} {:>6} {:>8}  STATE",
            "AGENT", "HOST", "PROJECT", "AGE", "CPU", "MEM"
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
            let quiet = String::new();
            let _ = writeln!(
                o,
                "  {:<12} {:<12} {:<32} {:>8} {:>5.1}% {:>8}  {}{}{}",
                fit_right(x.kind.label(), 12),
                fit_right(&x.host, 12),
                fit_left(x.project.as_deref().unwrap_or("?"), 32),
                dur(x.age_secs),
                x.cpu_window_mean.unwrap_or(x.cpu),
                bytes(x.rss),
                state,
                quiet,
                tag
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
