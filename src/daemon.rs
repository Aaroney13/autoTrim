//! The always-on part. Samples on an interval, keeps a rolling window per
//! agent session so "quiet" means quiet for a while rather than quiet this
//! instant, persists history, and reports advice transitions.
//!
//! No model calls, no arbitrary actions. Everything here is a rule you can read.

use crate::actions;
use crate::agents::{AgentSession, SessionState};
use crate::fmt::{bytes, date_utc, dur, stamp_utc};
use crate::notify;
use crate::paths;
use crate::rules::{self, Advice, Thresholds};
use crate::trends::History;
use crate::{Snapshot, take_snapshot_with};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::System;

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
    /// Auto-mode candidates and when they were first warned about.
    #[serde(default)]
    pending: HashMap<String, u64>,
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
            let key = format!("{}:{}:{}", p.pid, p.port, p.protocol);
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

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

fn append_history(dir: &Path, now: u64, rec: &HistoryRecord) -> Result<PathBuf> {
    let path = dir.join(format!("history-{}.jsonl", date_utc(now)));
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    serde_json::to_writer(&mut f, rec)?;
    f.write_all(b"\n")?;
    Ok(path)
}

fn rotate_history(dir: &Path, now: u64, retention_days: u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = date_utc(now.saturating_sub(retention_days * 86_400));
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let date = name
            .strip_prefix("history-")
            .and_then(|n| n.strip_suffix(".jsonl"));
        if date.is_some_and(|d| d < cutoff.as_str()) {
            let _ = fs::remove_file(e.path());
        }
    }
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

/// Sessions auto mode may close this tick. Stricter than the advice: it
/// needs transcript evidence of idleness, a warm quiet window agreeing,
/// an allowed host, and it spares the most recently active session in each
/// project so a person always keeps their place.
fn auto_session_candidates<'a>(
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

/// Servers auto mode may stop: the old-servers rule's targets, narrowed to
/// known dev runtimes. Anything else old and unmanaged is only reported.
fn auto_server_candidates<'a>(
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
/// grace has run out, forget ones that went away or woke up.
fn run_auto(tracker: &mut Tracker, snap: &Snapshot, cfg: &DaemonConfig, now: u64) {
    let auto = &cfg.auto;
    if !auto.close_sessions && !auto.stop_servers {
        return;
    }
    let grace = auto.grace.as_secs();
    let mode = if auto.dry_run { "dry-run" } else { "auto" };
    let mut live = std::collections::HashSet::new();
    let mut warned: Vec<String> = Vec::new();
    let mut acted: Vec<actions::ActionRecord> = Vec::new();

    if auto.close_sessions {
        for s in auto_session_candidates(snap, auto, &cfg.thresholds) {
            let k = format!("s:{}:{}", s.pid, s.start_time);
            live.insert(k.clone());
            let first = *tracker.pending.entry(k).or_insert_with(|| {
                warned.push(format!(
                    "{} · {} (idle {})",
                    s.kind.label(),
                    s.session_name
                        .as_deref()
                        .or(s.project.as_deref())
                        .unwrap_or("?"),
                    dur(s.idle_secs.unwrap_or(0))
                ));
                now
            });
            if now.saturating_sub(first) >= grace {
                acted.push(actions::close_session(s, mode, auto.dry_run));
            }
        }
    }
    if auto.stop_servers {
        for p in auto_server_candidates(snap, &cfg.thresholds) {
            let k = format!("p:{}:{}", p.pid, p.port);
            live.insert(k.clone());
            let first = *tracker.pending.entry(k).or_insert_with(|| {
                warned.push(format!(
                    "{} on {}:{} (open {})",
                    p.process,
                    p.addr,
                    p.port,
                    dur(p.open_for_secs)
                ));
                now
            });
            if now.saturating_sub(first) >= grace {
                acted.push(actions::stop_server(p, mode, auto.dry_run));
            }
        }
    }
    tracker.pending.retain(|k, _| live.contains(k));

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
        println!("{} ~ {}: {}", stamp_utc(now), title, warned.join(" · "));
        if cfg.notify {
            let _ = notify::send(
                &title,
                "Use one to keep it. Everything closed can be resumed.",
                &body,
            );
        }
    }
    for rec in acted {
        for k in tracker.pending.keys().cloned().collect::<Vec<_>>() {
            if k.ends_with(&format!(":{}", rec.pid)) || k.contains(&format!(":{}:", rec.pid)) {
                tracker.pending.remove(&k);
            }
        }
        if let Err(e) = actions::log(&rec) {
            eprintln!("{} could not write action log: {e}", stamp_utc(now));
        }
        println!(
            "{} {}",
            stamp_utc(now),
            actions::describe(&rec)
                .trim_start_matches(|c: char| c != '[')
                .trim_start()
        );
        if cfg.notify {
            let title = format!(
                "{} {}",
                if auto.dry_run {
                    "Would have closed"
                } else {
                    "Closed"
                },
                rec.target
            );
            let body = rec.resume.clone().unwrap_or_else(|| rec.result.clone());
            let _ = notify::send(&title, &format!("freed about {}", bytes(rec.rss)), &body);
        }
    }
}

pub fn run(cfg: DaemonConfig) -> Result<()> {
    let dir = paths::data_dir().context("no data directory for this platform")?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    claim_lock(&dir)?;
    let state_path = dir.join("state.json");
    let latest_path = dir.join("latest.json");

    let mut tracker = Tracker::load(&state_path);
    if tracker.history.series.is_empty() {
        let n = warm_start(
            &mut tracker.history,
            &dir,
            now_epoch(),
            cfg.thresholds.trend_window_secs,
        );
        if n > 0 {
            eprintln!("warmed trends from {n} history records");
        }
    }
    let mut sys = System::new();
    let mut prev_ids: BTreeSet<String> = BTreeSet::new();
    let window = cfg.window.as_secs();

    eprintln!(
        "autotrim daemon · every {}s · window {}s · data in {}",
        cfg.interval.as_secs(),
        window,
        dir.display()
    );

    // The first tick needs a real CPU delta, so it samples briefly. Every
    // later tick measures CPU since the previous tick.
    let mut first = true;
    loop {
        let now = now_epoch();
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
            println!("{} + {}", stamp_utc(now), a.title);
        }
        for id in prev_ids.difference(&ids) {
            println!("{} - {} resolved", stamp_utc(now), id);
        }
        if prev_ids.is_empty() && ids.is_empty() {
            println!("{} · nothing to do", stamp_utc(now));
        }
        prev_ids = ids;

        run_auto(&mut tracker, &snap, &cfg, now);

        if cfg.notify {
            for a in tracker.due_for_notification(now, cfg.remind_every.as_secs(), &snap.advice) {
                let body = a.evidence.first().cloned().unwrap_or_default();
                match notify::send(&a.title, &a.action, &body) {
                    Ok(()) => println!("{} ! notified: {}", stamp_utc(now), a.title),
                    Err(e) => eprintln!("{} notification failed: {e}", stamp_utc(now)),
                }
            }
        }

        write_atomic(&latest_path, &serde_json::to_vec_pretty(&snap)?)?;
        append_history(&dir, now, &record(&snap))?;
        tracker.save(&state_path)?;
        rotate_history(&dir, now, cfg.retention_days);

        if cfg.once {
            eprintln!(
                "one tick · {} sessions · swap {} · wrote {}",
                snap.sessions.len(),
                bytes(snap.system.used_swap),
                latest_path.display()
            );
            break;
        }
        std::thread::sleep(cfg.interval);
    }
    let _ = fs::remove_file(dir.join("daemon.pid"));
    Ok(())
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
