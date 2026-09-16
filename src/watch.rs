//! A live terminal view: what the daemon last saw, and its recent log.
//! Refreshes in place. With no daemon running it scans on its own.

use crate::fmt::dur;
use crate::{daemon, paths, report, rules, take_snapshot};
use anyhow::Result;
use std::fs;
use std::io::Write;
use std::time::Duration;
use sysinfo::System;

pub fn run(interval: Duration, log_lines: usize, thresholds: rules::Thresholds) -> Result<()> {
    let mut sys = System::new();
    let log_path = paths::data_dir().map(|d| d.join("daemon.log"));
    loop {
        let mut out = String::new();
        let (snap, source) = match daemon::latest()? {
            // Trust the daemon while it is clearly alive.
            Some((snap, age)) if age <= 120 => (snap, format!("daemon snapshot, {} ago", dur(age))),
            Some((_, age)) => (
                take_snapshot(&mut sys, Some(Duration::from_millis(1000)), &thresholds),
                format!("live scan (daemon snapshot is {} old)", dur(age)),
            ),
            None => (
                take_snapshot(&mut sys, Some(Duration::from_millis(1000)), &thresholds),
                "live scan (no daemon data yet)".to_string(),
            ),
        };
        out.push_str(&format!(
            "autotrim watch · {source} · refresh {}s · ctrl-c to quit\n\n",
            interval.as_secs()
        ));
        out.push_str(&report::render(&snap));
        if let Some(p) = &log_path
            && let Ok(tail) = crate::storage::tail_lines(p, log_lines)
            && !tail.is_empty()
        {
            out.push_str("\nDaemon log\n");
            for l in &tail {
                out.push_str("  ");
                out.push_str(l);
                out.push('\n');
            }
        }
        let mut stdout = std::io::stdout().lock();
        // Clear screen, home cursor, then the whole frame in one write.
        write!(stdout, "\x1b[2J\x1b[H{out}")?;
        stdout.flush()?;
        std::thread::sleep(interval);
    }
}

pub fn print_log(lines: usize, follow: bool) -> Result<()> {
    let Some(path) = paths::data_dir().map(|d| d.join("daemon.log")) else {
        println!("no data directory on this platform");
        return Ok(());
    };
    if !path.exists() {
        println!("no daemon log yet at {}", path.display());
        return Ok(());
    }
    // Capture the follow offset before tailing so new writes are not skipped.
    let mut seen = fs::metadata(&path)?.len();
    let mut rotation = crate::storage::rotation_stamp(&path);
    for l in crate::storage::tail_lines(&path, lines)? {
        println!("{l}");
    }
    if !follow {
        return Ok(());
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Ok(mut f) = fs::File::open(&path) else {
            continue;
        };
        f.lock_shared()?;
        let len = f.metadata()?.len();
        let current_rotation = crate::storage::rotation_stamp(&path);
        if len < seen || current_rotation != rotation {
            seen = 0; // rotated or truncated
        }
        rotation = current_rotation;
        if len > seen {
            use std::io::{Read, Seek, SeekFrom};
            f.seek(SeekFrom::Start(seen))?;
            let mut buf = Vec::new();
            f.take((len - seen).min(crate::storage::LOG_BYTES))
                .read_to_end(&mut buf)?;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&buf)?;
            stdout.flush()?;
            seen += buf.len() as u64;
        }
    }
}
