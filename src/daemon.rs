//! The always-on part. Samples on an interval, keeps a rolling window per
//! agent session so "quiet" means quiet for a while rather than quiet this
//! instant, persists history, and reports advice transitions.
//!
//! No model calls, no arbitrary actions. Everything here is a rule you can read.

use crate::actions;
use crate::agents::AgentSession;
use crate::config::Config;
use crate::fmt::{bytes, date_utc, dur, stamp_utc};
use crate::footprint;
use crate::notify;
use crate::paths;
use crate::rules::{self, Advice, Thresholds};
use crate::storage::write_atomic;
use crate::trends::History;
use crate::{Snapshot, take_snapshot_with};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::System;

macro_rules! daemon_log {
    ($($arg:tt)*) => { crate::diagnostics::log(format_args!($($arg)*)) };
}

pub struct DaemonConfig {
    pub interval: Duration,
    pub window: Duration,
    pub retention_days: u64,
    pub thresholds: Thresholds,
    /// Take one sample and exit. For testing and for cron-style use.
    pub once: bool,
    /// Deliver native notifications for advice.
    pub notify: bool,
    /// How long before the same advice is notified again while it persists.
    pub remind_every: Duration,
    pub auto: AutoConfig,
}

#[derive(Clone, Debug, Default)]
pub struct AutoConfig {
    pub close_sessions: bool,
    pub stop_servers: bool,
    pub grace: Duration,
    pub dry_run: bool,
    pub hosts: Vec<String>,
}

/// Command-line overrides. Everything else comes from the config file,
/// which the daemon re-reads whenever it changes, so a toggle in the
/// window or an edit by hand takes effect on the next tick.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub interval_secs: Option<u64>,
    pub window_secs: Option<u64>,
    pub retention_days: Option<u64>,
    pub stale_after_hours: Option<f64>,
    pub min_quiet_minutes: Option<u64>,
    pub quiet_cpu: Option<f32>,
    pub no_notify: bool,
    pub remind_every_hours: Option<f64>,
    pub once: bool,
}

impl DaemonConfig {
    /// The file's settings with the command line's overrides on top.
    pub fn from_config(cfg: &Config, o: &Overrides) -> DaemonConfig {
        let mut thresholds = cfg.thresholds();
        if let Some(h) = o.stale_after_hours {
            thresholds.stale_after_secs = (h * 3600.0) as u64;
        }
        if let Some(m) = o.min_quiet_minutes {
            thresholds.min_quiet_secs = m * 60;
        }
        if let Some(q) = o.quiet_cpu {
            thresholds.quiet_cpu = q;
        }
        let interval = o.interval_secs.unwrap_or(cfg.interval_secs).max(5);
        let window = o.window_secs.unwrap_or(cfg.window_secs).max(interval);
        let remind = o.remind_every_hours.unwrap_or(cfg.remind_every_hours);
        DaemonConfig {
            interval: Duration::from_secs(interval),
            window: Duration::from_secs(window),
            retention_days: o.retention_days.unwrap_or(cfg.retention_days).max(1),
            thresholds,
            once: o.once,
            notify: cfg.notify && !o.no_notify,
            remind_every: Duration::from_secs((remind * 3600.0).max(60.0) as u64),
            auto: AutoConfig {
                close_sessions: cfg.auto_close_sessions,
                stop_servers: cfg.auto_stop_servers,
                grace: Duration::from_secs(cfg.auto_grace_minutes * 60),
                dry_run: cfg.auto_dry_run,
                hosts: cfg.auto_hosts.clone(),
            },
        }
    }
}

impl AutoConfig {
    /// The same settings in the shape the snapshot carries.
    pub fn status(&self, pending: Vec<PendingTarget>) -> AutoStatus {
        AutoStatus {
            close_sessions: self.close_sessions,
            stop_servers: self.stop_servers,
            dry_run: self.dry_run,
            grace_secs: self.grace.as_secs(),
            hosts: self.hosts.clone(),
            pending,
        }
    }

    pub fn describe(&self) -> String {
        self.status(Vec::new()).describe()
    }
}

/// Auto mode as the daemon is running it, for the window and `status`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AutoStatus {
    pub close_sessions: bool,
    pub stop_servers: bool,
    pub dry_run: bool,
    pub grace_secs: u64,
    pub hosts: Vec<String>,
    /// Targets warned about and waiting out the grace period.
    pub pending: Vec<PendingTarget>,
}

impl AutoStatus {
    /// One line: what it does and how, or "off".
    pub fn describe(&self) -> String {
        if !self.close_sessions && !self.stop_servers {
            return "off".to_string();
        }
        let mut what = Vec::new();
        if self.close_sessions {
            what.push("closes stale sessions");
        }
        if self.stop_servers {
            what.push("stops old servers");
        }
        let mut s = format!(
            "{} after a {} warning",
            what.join(" and "),
            dur(self.grace_secs)
        );
        if self.dry_run {
            s.push_str(" · dry run, nothing is closed");
        }
        s
    }
}

/// One target auto mode has warned about.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PendingTarget {
    /// "session" or "server".
    pub kind: String,
    pub pid: u32,
    pub target: String,
    /// Why: "idle 9h" or "open 3d".
    pub detail: String,
    pub rss: u64,
    /// Epoch seconds when it was first warned about.
    pub since: u64,
    /// Epoch seconds when the grace period runs out.
    pub due_at: u64,
}

/// Rolling observations for one session, keyed by pid and start time so a
/// reused pid is never mistaken for the old session.
#[derive(Serialize, Deserialize, Default, Clone)]
struct SessionTrack {
    /// (epoch seconds, CPU percent for that tick)
    samples: VecDeque<(u64, f32)>,
    /// When the window mean last dropped below the quiet threshold.
    quiet_since: Option<u64>,
    last_seen: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Tracker {
    sessions: HashMap<String, SessionTrack>,
    /// Advice id to the last time it was notified. Persisted so a restart
    /// does not repeat what the user was just told.
    #[serde(default)]
    notified: HashMap<String, u64>,
    /// "pid:port:proto" to when the daemon first saw it listening. On first
    /// sight the owner's start time is used, since a port cannot predate
    /// its process, which makes restarts and first runs honest.
    #[serde(default)]
    ports: HashMap<String, u64>,
    // Flatten retains the existing persisted pending/dry_done keys.
    #[serde(flatten)]
    policy: crate::policy::PolicyState,
    /// Rolling series per app and session, for growth and CPU trends.
    #[serde(default)]
    history: History,
    /// "pid:port" to what the probe learned, so each port is asked once.
    #[serde(default)]
    port_labels: HashMap<String, String>,
}

fn key(s: &AgentSession) -> String {
    format!("{}:{}", s.pid, s.start_time)
}

impl Tracker {
    fn load(path: &Path) -> Tracker {
        fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &serde_json::to_vec(self)?)
    }

    /// Fold this tick's sessions into the window and annotate them.
    fn observe(&mut self, now: u64, window: u64, quiet_cpu: f32, sessions: &mut [AgentSession]) {
        for s in sessions.iter_mut() {
            let t = self.sessions.entry(key(s)).or_default();
            t.samples.push_back((now, s.cpu));
            while t
                .samples
                .front()
                .map(|(ts, _)| now.saturating_sub(*ts) > window)
                .unwrap_or(false)
            {
                t.samples.pop_front();
            }
            let mean =
                t.samples.iter().map(|(_, c)| *c).sum::<f32>() / t.samples.len().max(1) as f32;
            if mean < quiet_cpu {
                if t.quiet_since.is_none() {
                    t.quiet_since = Some(now);
                }
            } else {
                t.quiet_since = None;
            }
            t.last_seen = now;
            // A window with one or two samples is no better than a one-shot
            // sample; only publish the mean once it is worth trusting.
            let warm = t.samples.len() >= 3;
            s.cpu_window_mean = warm.then_some(mean);
            s.quiet_for_secs = t.quiet_since.map(|q| now.saturating_sub(q));
        }
        // Forget sessions that have been gone for a full window.
        self.sessions
            .retain(|_, t| now.saturating_sub(t.last_seen) <= window);
    }

    /// Stamp each listening port with how long it has been open.
    fn observe_ports(&mut self, now: u64, ports: &mut [crate::ports::PortInfo]) {
        let mut live = std::collections::HashSet::new();
        for p in ports.iter_mut() {
            let key = format!("{}:{}:{}:{}", p.pid, p.start_time, p.port, p.protocol);
            let first = *self
                .ports
                .entry(key.clone())
                .or_insert_with(|| now.saturating_sub(p.owner_age_secs));
            p.open_for_secs = now.saturating_sub(first);
            live.insert(key);
        }
        self.ports.retain(|k, _| live.contains(k));
    }

    /// Advice worth notifying now: never seen, or seen longer ago than the
    /// reminder interval. Records the send.
    fn due_for_notification<'a>(
        &mut self,
        now: u64,
        remind_every: u64,
        advice: &'a [Advice],
    ) -> Vec<&'a Advice> {
        let mut due = Vec::new();
        for a in advice {
            let last = self.notified.get(&a.id).copied();
            if last.is_none_or(|t| now.saturating_sub(t) >= remind_every) {
                self.notified.insert(a.id.clone(), now);
                due.push(a);
            }
        }
        // Drop records for advice that has been gone long enough to be news again.
        let live: BTreeSet<&str> = advice.iter().map(|a| a.id.as_str()).collect();
        self.notified
            .retain(|id, t| live.contains(id.as_str()) || now.saturating_sub(*t) < remind_every);
        due
    }
}

/// One compact line per tick. Small enough to keep a week of 30 s ticks in
/// a few tens of megabytes.
#[derive(Serialize)]
struct HistoryRecord {
    ts: u64,
    used_mem: u64,
    used_swap: u64,
    total_swap: u64,
    compressed: Option<u64>,
    free_pct: Option<u8>,
    uptime: u64,
    sessions: Vec<SessionRecord>,
    top: Vec<TopRecord>,
    advice: Vec<String>,
}

#[derive(Serialize)]
struct TopRecord {
    name: String,
    kind: String,
    rss: u64,
    cpu: f32,
}

#[derive(Serialize)]
struct SessionRecord {
    key: String,
    kind: String,
    project: Option<String>,
    rss: u64,
    cpu: f32,
    quiet_for: Option<u64>,
    state: String,
}

fn record(snap: &Snapshot) -> HistoryRecord {
    HistoryRecord {
        ts: snap.taken_at,
        used_mem: snap.system.used_mem,
        used_swap: snap.system.used_swap,
        total_swap: snap.system.total_swap,
        compressed: snap.system.compressed,
        free_pct: snap.system.free_pct,
        uptime: snap.system.uptime_secs,
        sessions: snap
            .sessions
            .iter()
            .map(|s| SessionRecord {
                key: key(s),
                kind: s.kind.label().to_string(),
                project: s.project.clone(),
                rss: s.rss,
                cpu: s.cpu,
                quiet_for: s.quiet_for_secs,
                state: format!("{:?}", s.state).to_lowercase(),
            })
            .collect(),
        top: snap
            .groups
            .iter()
            .take(40)
            .map(|g| TopRecord {
                name: g.name.clone(),
                kind: format!("{:?}", g.kind).to_lowercase(),
                rss: g.rss,
                cpu: g.cpu,
            })
            .collect(),
        advice: snap.advice.iter().map(|a| a.id.clone()).collect(),
    }
}

/// One daemon per data directory. The lock is a pid file; one left by a
/// crash is ignored when no live autotrim process has that pid, so a stale
/// file never blocks a restart.
fn claim_lock(dir: &Path) -> Result<()> {
    let path = dir.join("daemon.pid");
    if let Ok(text) = fs::read_to_string(&path)
        && let Ok(pid) = text.trim().parse::<u32>()
        && pid != std::process::id()
    {
        let mut sys = System::new();
        let target = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[target]), true);
        if let Some(p) = sys.process(target)
            && p.name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("autotrim")
        {
            anyhow::bail!(
                "an autotrim daemon is already running (pid {pid}) on {}; stop it with `autotrim service restart` or point AUTOTRIM_DATA_DIR elsewhere",
                dir.display()
            );
        }
    }
    write_atomic(&path, std::process::id().to_string().as_bytes())
}

fn append_history(dir: &Path, now: u64, rec: &HistoryRecord) -> Result<PathBuf> {
    let path = dir.join(format!("history-{}.jsonl", date_utc(now)));
    crate::storage::append_history(&path, rec)?;
    Ok(path)
}

/// Attempt every output even when another one fails; retry on the next tick.
fn persist_tick(dir: &Path, now: u64, snap: &Snapshot, tracker: &Tracker) -> Result<()> {
    let writes = [
        (
            "latest.json",
            serde_json::to_vec_pretty(snap)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| write_atomic(&dir.join("latest.json"), &bytes)),
        ),
        (
            "history",
            append_history(dir, now, &record(snap)).map(|_| ()),
        ),
        ("state.json", tracker.save(&dir.join("state.json"))),
    ];
    let errors: Vec<String> = writes
        .into_iter()
        .filter_map(|(name, result)| result.err().map(|e| format!("{name}: {e:#}")))
        .collect();
    if !errors.is_empty() {
        anyhow::bail!("{}", errors.join("; "));
    }
    Ok(())
}

fn rotate_history(dir: &Path, now: u64, retention_days: u64) -> Result<()> {
    let entries = fs::read_dir(dir)?;
    let cutoff = date_utc(now.saturating_sub(retention_days.saturating_mul(86_400)));
    for e in entries {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let date = name
            .strip_prefix("history-")
            .and_then(|n| n.strip_suffix(".jsonl"));
        if date.is_some_and(|d| d < cutoff.as_str()) {
            fs::remove_file(e.path())
                .with_context(|| format!("removing expired history {}", e.path().display()))?;
        }
    }
    Ok(())
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Feed this tick into the rolling series. Groups are capped to the
/// heaviest forty so a machine with hundreds of tiny helpers does not bloat
/// the state file; every session is kept.
fn observe_trends(h: &mut History, snap: &Snapshot, now: u64, t: &Thresholds) {
    let w = t.trend_window_secs;
    for g in snap.groups.iter().take(40) {
        let kind = format!("{:?}", g.kind).to_lowercase();
        h.observe(
            format!("g:{}", g.name),
            &g.name,
            &kind,
            (now, g.rss, g.cpu),
            w,
        );
    }
    for s in &snap.sessions {
        let name = format!(
            "{} · {}",
            s.kind.label(),
            s.session_name
                .as_deref()
                .or(s.project.as_deref())
                .unwrap_or("?")
        );
        h.observe(
            format!("s:{}:{}", s.pid, s.start_time),
            &name,
            "session",
            (now, s.rss, s.cpu),
            w,
        );
    }
    // Each tab renderer on its own, so a page that grows all day shows up
    // even though Chrome never says which tab it is. Small renderers are
    // frames and spares and are skipped until they grow into a page.
    for b in &snap.browsers {
        for r in b
            .renderer_procs
            .iter()
            .filter(|r| r.rss >= 40 * 1024 * 1024)
        {
            h.observe(
                format!("r:{}:{}", r.pid, r.start_time),
                &format!("{} page (pid {})", b.name, r.pid),
                "renderer",
                (now, r.rss, r.cpu),
                w,
            );
        }
    }
    h.push_system(
        now,
        snap.system.used_swap,
        snap.system.compressed.unwrap_or(0),
        snap.system.used_mem,
        w,
    );
    h.prune(now, w);
}

/// Rebuild the rolling series from the history files on disk, so a restart
/// does not forget the last two hours. Tolerates the older record shape.
fn warm_start(h: &mut History, dir: &Path, now: u64, window: u64) -> usize {
    let mut count = 0;
    let days = [date_utc(now.saturating_sub(86_400)), date_utc(now)];
    for day in days {
        let path = dir.join(format!("history-{day}.jsonl"));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(ts) = v.get("ts").and_then(|t| t.as_u64()) else {
                continue;
            };
            if now.saturating_sub(ts) > window {
                continue;
            }
            count += 1;
            if let Some(top) = v.get("top").and_then(|t| t.as_array()) {
                for e in top {
                    let (name, kind, rss, cpu) = if let Some(arr) = e.as_array() {
                        (
                            arr.first()
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            "app".to_string(),
                            arr.get(1).and_then(|x| x.as_u64()).unwrap_or(0),
                            0.0f32,
                        )
                    } else {
                        (
                            e.get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            e.get("kind")
                                .and_then(|x| x.as_str())
                                .unwrap_or("app")
                                .to_string(),
                            e.get("rss").and_then(|x| x.as_u64()).unwrap_or(0),
                            e.get("cpu").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                        )
                    };
                    if !name.is_empty() {
                        h.observe(format!("g:{name}"), &name, &kind, (ts, rss, cpu), window);
                    }
                }
            }
            if let Some(sessions) = v.get("sessions").and_then(|t| t.as_array()) {
                for e in sessions {
                    let key = e.get("key").and_then(|x| x.as_str()).unwrap_or("");
                    if key.is_empty() {
                        continue;
                    }
                    let name = format!(
                        "{} · {}",
                        e.get("kind").and_then(|x| x.as_str()).unwrap_or("?"),
                        e.get("project").and_then(|x| x.as_str()).unwrap_or("?")
                    );
                    let rss = e.get("rss").and_then(|x| x.as_u64()).unwrap_or(0);
                    let cpu = e.get("cpu").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                    h.observe(format!("s:{key}"), &name, "session", (ts, rss, cpu), window);
                }
            }
            let swap = v.get("used_swap").and_then(|x| x.as_u64()).unwrap_or(0);
            let comp = v.get("compressed").and_then(|x| x.as_u64()).unwrap_or(0);
            let used = v.get("used_mem").and_then(|x| x.as_u64()).unwrap_or(0);
            h.push_system(ts, swap, comp, used, window);
        }
    }
    count
}

/// One pass of auto mode: warn about new candidates, act on ones whose
/// grace has run out, forget ones that went away or woke up. Returns what
/// the snapshot should say about it. With auto mode off the pending list
/// is emptied, so switching it on later starts every grace period afresh.
fn run_auto(
    tracker: &mut Tracker,
    snap: &Snapshot,
    cfg: &DaemonConfig,
    now: u64,
    overrides: &Overrides,
) -> AutoStatus {
    let auto = &cfg.auto;
    let grace = auto.grace.as_secs();
    let decisions = crate::policy::decide(&mut tracker.policy, snap, cfg, now);
    let warned = decisions.warned;
    let pending = decisions.pending;
    for key in decisions.cancelled {
        daemon_log!("{} cancelled pending {key}", stamp_utc(now));
    }
    let mut acted = Vec::new();
    for target in decisions.eligible {
        tracker.policy.pending.remove(&target.key());
        // Re-read settings and sample immediately before each target, since a
        // previous action may have waited many seconds for its process tree.
        let result = (|| -> Result<actions::ActionRecord> {
            let (file, _) = Config::load()?;
            let current = DaemonConfig::from_config(&file, overrides);
            let mut sys = System::new();
            let mut fresh = take_snapshot_with(
                &mut sys,
                Some(Duration::from_millis(1000)),
                &current.thresholds,
                Some(&mut tracker.port_labels),
            );
            // Retain only previously observed quiet evidence. A fresh sample
            // cannot manufacture a warmed daemon window.
            for session in &mut fresh.sessions {
                if let Some(prior) = snap
                    .sessions
                    .iter()
                    .find(|s| s.pid == session.pid && s.start_time == session.start_time)
                {
                    session.cpu_window_mean = prior.cpu_window_mean;
                    session.quiet_for_secs = prior.quiet_for_secs;
                }
                session.state = rules::session_state(session, &current.thresholds);
            }
            for port in &mut fresh.ports {
                if let Some(prior) = snap.ports.iter().find(|p| {
                    p.pid == port.pid && p.start_time == port.start_time && p.port == port.port
                }) {
                    port.open_for_secs = prior.open_for_secs;
                }
            }
            actions::execute_auto(&target, &fresh, &sys, &current)
        })();
        match result {
            Ok(rec) => acted.push(rec),
            Err(e) => daemon_log!("{} auto target skipped: {e:#}", stamp_utc(now)),
        }
    }

    if !warned.is_empty() {
        let title = format!(
            "{} {} in {}",
            if auto.dry_run {
                "Would close"
            } else {
                "Closing"
            },
            if warned.len() == 1 {
                "1 idle target".to_string()
            } else {
                format!("{} idle targets", warned.len())
            },
            dur(grace)
        );
        let body = warned.join("\n");
        daemon_log!("{} ~ {}: {}", stamp_utc(now), title, warned.join(" · "));
        if cfg.notify {
            let _ = notify::send(
                &title,
                "Use one to keep it. Available resume instructions are saved; unsaved state may be lost.",
                &body,
            );
        }
    }
    for rec in acted {
        daemon_log!(
            "{} {}",
            stamp_utc(now),
            actions::describe(&rec)
                .trim_start_matches(|c: char| c != '[')
                .trim_start()
        );
        if cfg.notify {
            let (title, subtitle) = action_notification(&rec);
            let body = format!(
                "{}\n{}",
                rec.result,
                rec.resume
                    .as_deref()
                    .unwrap_or("No resume command is available.")
            );
            let _ = notify::send(&title, &subtitle, &body);
        }
    }
    auto.status(pending)
}

fn action_notification(rec: &actions::ActionRecord) -> (String, String) {
    let verb = match if rec.mode == "dry-run" {
        "dry_run"
    } else {
        rec.status.as_str()
    } {
        "dry_run" => "Would close",
        "success" => "Closed",
        "skipped" => "Skipped",
        _ => "Cleanup incomplete for",
    };
    let memory = if rec.rss == 0 {
        "Previous memory footprint unknown".into()
    } else {
        format!("Previously held {} (not measured savings)", bytes(rec.rss))
    };
    (format!("{verb} {}", rec.target), memory)
}

#[cfg(test)]
mod notification_tests {
    use super::*;
    #[test]
    fn never_claims_measured_savings() {
        let base = serde_json::json!({"ts": 0, "mode": "dry-run", "action": "close_session", "pid": 1,
            "target": "fixture", "rss": 1000, "result": "dry run", "status": "dry_run"});
        let mut rec: actions::ActionRecord = serde_json::from_value(base).unwrap();
        for (status, expected) in [
            ("dry_run", "Would close"),
            ("success", "Closed"),
            ("partial", "Cleanup incomplete"),
            ("failure", "Cleanup incomplete"),
            ("skipped", "Skipped"),
        ] {
            rec.status = status.into();
            rec.mode = if status == "dry_run" {
                "dry-run"
            } else {
                "auto"
            }
            .into();
            let (title, subtitle) = action_notification(&rec);
            assert!(title.starts_with(expected));
            assert!(subtitle.contains("Previously held"));
            assert!(!subtitle.contains("freed"));
        }
        rec.rss = 0;
        assert!(action_notification(&rec).1.contains("unknown"));
    }
}

/// The config file's modification time and size, or None when there is no
/// file. Cheap enough to check every tick.
fn config_stamp() -> Option<(SystemTime, u64)> {
    let m = fs::metadata(Config::path()?).ok()?;
    Some((m.modified().ok()?, m.len()))
}

struct PidGuard(PathBuf);

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The daemon loop. `file` is the config as loaded at start; the file is
/// watched and re-read when it changes, with `overrides` re-applied on
/// top each time.
pub fn run(file: &Config, overrides: Overrides) -> Result<()> {
    let dir = paths::data_dir().context("no data directory for this platform")?;
    crate::diagnostics::set_notifications(file.notify && !overrides.no_notify);
    crate::storage::private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    claim_lock(&dir)?;
    let _pid_guard = PidGuard(dir.join("daemon.pid"));
    let mut health = crate::diagnostics::Health::default();
    let mut permissions_pending = true;
    let state_path = dir.join("state.json");
    let latest_path = dir.join("latest.json");
    let mut cfg = DaemonConfig::from_config(file, &overrides);
    let mut stamp = config_stamp();

    let mut tracker = Tracker::load(&state_path);
    if tracker.history.series.is_empty() {
        let n = warm_start(
            &mut tracker.history,
            &dir,
            now_epoch(),
            cfg.thresholds.trend_window_secs,
        );
        if n > 0 {
            daemon_log!("warmed trends from {n} history records");
        }
    }
    let mut sys = System::new();
    let mut prev_ids: BTreeSet<String> = BTreeSet::new();

    daemon_log!(
        "autotrim daemon · every {}s · window {}s · auto mode {} · data in {}",
        cfg.interval.as_secs(),
        cfg.window.as_secs(),
        cfg.auto.describe(),
        dir.display()
    );

    // The first tick needs a real CPU delta, so it samples briefly. Every
    // later tick measures CPU since the previous tick.
    let mut first = true;
    // The footprint budget is a public promise; the log checks it after
    // the first tick and once an hour after that.
    let mut last_self = 0u64;
    loop {
        let now = now_epoch();
        let seen = config_stamp();
        if seen != stamp {
            stamp = seen;
            match Config::load() {
                Ok((c, _)) => {
                    cfg = DaemonConfig::from_config(&c, &overrides);
                    crate::diagnostics::set_notifications(cfg.notify);
                    daemon_log!(
                        "{} = settings reloaded · auto mode {}",
                        stamp_utc(now),
                        cfg.auto.describe()
                    );
                }
                Err(e) => daemon_log!("{} settings not reloaded: {e}", stamp_utc(now)),
            }
        }
        let window = cfg.window.as_secs();
        let sample = if first {
            Some(Duration::from_millis(1500))
        } else {
            None
        };
        first = false;

        let mut snap = take_snapshot_with(
            &mut sys,
            sample,
            &cfg.thresholds,
            Some(&mut tracker.port_labels),
        );
        let live_ports: std::collections::HashSet<String> = snap
            .ports
            .iter()
            .map(|p| format!("{}:{}", p.pid, p.port))
            .collect();
        tracker.port_labels.retain(|k, _| live_ports.contains(k));
        tracker.observe(now, window, cfg.thresholds.quiet_cpu, &mut snap.sessions);
        tracker.observe_ports(now, &mut snap.ports);
        for s in &mut snap.sessions {
            s.state = rules::session_state(s, &cfg.thresholds);
        }
        observe_trends(&mut tracker.history, &snap, now, &cfg.thresholds);
        snap.trends = tracker.history.trends(cfg.thresholds.cpu_hog_secs);
        let swap_growth = tracker.history.swap_growth(3600);
        snap.advice = rules::evaluate(
            &snap.system,
            &snap.groups,
            &snap.sessions,
            &snap.browsers,
            &snap.ports,
            &snap.trends,
            swap_growth,
            &cfg.thresholds,
        );

        let ids: BTreeSet<String> = snap.advice.iter().map(|a| a.id.clone()).collect();
        for a in snap.advice.iter().filter(|a| !prev_ids.contains(&a.id)) {
            daemon_log!("{} + {}", stamp_utc(now), a.title);
        }
        for id in prev_ids.difference(&ids) {
            daemon_log!("{} - {} resolved", stamp_utc(now), id);
        }
        if prev_ids.is_empty() && ids.is_empty() {
            daemon_log!("{} · nothing to do", stamp_utc(now));
        }
        prev_ids = ids;

        snap.auto = Some(run_auto(&mut tracker, &snap, &cfg, now, &overrides));

        if cfg.notify {
            for a in tracker.due_for_notification(now, cfg.remind_every.as_secs(), &snap.advice) {
                let body = a.evidence.first().cloned().unwrap_or_default();
                match notify::send(&a.title, &a.action, &body) {
                    Ok(()) => daemon_log!("{} ! notified: {}", stamp_utc(now), a.title),
                    Err(e) => daemon_log!("{} notification failed: {e}", stamp_utc(now)),
                }
            }
        }

        let permissions = if permissions_pending {
            let result = crate::storage::harden_existing(&dir);
            permissions_pending = result.is_err();
            result
        } else {
            Ok(())
        };
        let retention = rotate_history(&dir, now, cfg.retention_days);
        let persisted = persist_tick(&dir, now, &snap, &tracker)
            .and(permissions)
            .and(retention);
        health.report(&persisted, now);

        if cfg.once || now.saturating_sub(last_self) >= 3600 {
            if let Some(line) = self_line() {
                daemon_log!("{} {line}", stamp_utc(now));
            }
            last_self = now;
        }

        if cfg.once {
            persisted?;
            daemon_log!(
                "one tick · {} sessions · swap {} · wrote {}",
                snap.sessions.len(),
                bytes(snap.system.used_swap),
                latest_path.display()
            );
            break;
        }
        std::thread::sleep(cfg.interval);
    }
    Ok(())
}

/// The daemon's own footprint, now and at its peak, in the column a person
/// would check it against. None where the platform has no such counter.
fn self_line() -> Option<String> {
    let me = std::process::id();
    let now = footprint::footprint(me)?;
    let peak = footprint::peak_footprint(me)?;
    Some(format!(
        "self: {} footprint, {} peak",
        bytes(now),
        bytes(peak)
    ))
}

/// Read the daemon's latest snapshot without sampling anything.
pub fn latest() -> Result<Option<(Snapshot, u64)>> {
    let dir = paths::data_dir().context("no data directory for this platform")?;
    let path = dir.join("latest.json");
    if !path.exists() {
        return Ok(None);
    }
    let snap: Snapshot = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("parsing {}", path.display()))?;
    let age = now_epoch().saturating_sub(snap.taken_at);
    Ok(Some((snap, age)))
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::storage::tests::Scratch;

    #[test]
    fn each_failed_output_leaves_other_outputs_working_and_recovers() {
        let snap: Snapshot = serde_json::from_value(serde_json::json!({
            "taken_at": 50000, "scanner_pid": 1,
            "system": {"os": "fixture", "total_mem": 0, "used_mem": 0,
                "available_mem": 0, "total_swap": 0, "used_swap": 0, "uptime_secs": 0},
            "groups": [], "sessions": [], "browsers": [], "advice": []
        }))
        .unwrap();
        let outputs = ["latest.json", "history-1970-01-01.jsonl", "state.json"];
        for failed in outputs {
            let dir = Scratch::new();
            fs::create_dir(dir.0.join(failed)).unwrap();
            assert!(persist_tick(&dir.0, 1, &snap, &Tracker::default()).is_err());
            for other in outputs.into_iter().filter(|name| *name != failed) {
                let bytes = fs::read(dir.0.join(other)).unwrap();
                assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
            }
            fs::remove_dir(dir.0.join(failed)).unwrap();
            persist_tick(&dir.0, 2, &snap, &Tracker::default()).unwrap();
            assert!(dir.0.join(failed).is_file());
        }
    }
}
