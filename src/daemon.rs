//! The always-on part. Samples on an interval, keeps a rolling window per
//! agent session so "quiet" means quiet for a while rather than quiet this
//! instant, persists history, and reports advice transitions.
//!
//! No model calls, no arbitrary actions. Everything here is a rule you can read.

use crate::agents::AgentSession;
use crate::fmt::{bytes, date_utc, stamp_utc};
use crate::paths;
use crate::rules::{self, Thresholds};
use crate::{Snapshot, take_snapshot};
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
            s.cpu_window_mean = Some(mean);
            s.quiet_for_secs = t.quiet_since.map(|q| now.saturating_sub(q));
        }
        // Forget sessions that have been gone for a full window.
        self.sessions
            .retain(|_, t| now.saturating_sub(t.last_seen) <= window);
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
    top: Vec<(String, u64)>,
    advice: Vec<String>,
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
            .take(10)
            .map(|g| (g.name.clone(), g.rss))
            .collect(),
        advice: snap.advice.iter().map(|a| a.id.clone()).collect(),
    }
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

pub fn run(cfg: DaemonConfig) -> Result<()> {
    let dir = paths::data_dir().context("no data directory for this platform")?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let state_path = dir.join("state.json");
    let latest_path = dir.join("latest.json");

    let mut tracker = Tracker::load(&state_path);
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

        let mut snap = take_snapshot(&mut sys, sample, &cfg.thresholds);
        tracker.observe(now, window, cfg.thresholds.quiet_cpu, &mut snap.sessions);
        for s in &mut snap.sessions {
            s.state = rules::windowed_state(s, &cfg.thresholds);
        }
        snap.advice = rules::evaluate(
            &snap.system,
            &snap.groups,
            &snap.sessions,
            &snap.browsers,
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
