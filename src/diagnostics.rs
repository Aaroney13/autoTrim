//! Best-effort local diagnostics. Logging must never panic on a bad disk or pipe.

use crate::{fmt::stamp_utc, notify, paths, storage};
use anyhow::Result;
use serde::Serialize;
use std::fmt::Arguments;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NOTIFY: AtomicBool = AtomicBool::new(true);
static LAST_LOG_FAILURE: AtomicU64 = AtomicU64::new(0);
const REMINDER_SECS: u64 = 3600;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn set_notifications(enabled: bool) {
    NOTIFY.store(enabled, Ordering::Relaxed);
}

fn alert(title: &str, body: &str) {
    if NOTIFY.load(Ordering::Relaxed) {
        let _ = notify::send(title, "autoTrim diagnostics", body);
    }
}

pub fn log(message: Arguments<'_>) {
    let line = format!("{message}\n");
    let result = paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data directory"))
        .and_then(|dir| storage::append_log(&dir.join("daemon.log"), line.as_bytes()));
    if let Err(error) = result {
        let ts = now();
        let last = LAST_LOG_FAILURE.load(Ordering::Relaxed);
        if last == 0 || ts.saturating_sub(last) >= REMINDER_SECS {
            LAST_LOG_FAILURE.store(ts, Ordering::Relaxed);
            let _ = writeln!(
                std::io::stderr().lock(),
                "{} daemon log unavailable: {error:#}\n{line}",
                stamp_utc(ts)
            );
            alert(
                "autoTrim cannot write its log",
                "Monitoring continues. Check the data directory's permissions and available disk space.",
            );
        }
    } else if LAST_LOG_FAILURE.swap(0, Ordering::Relaxed) != 0 {
        alert(
            "autoTrim logging recovered",
            "The daemon can write its log again.",
        );
    }
}

/// Tracks a continuing failure, with immediate reporting, hourly reminders,
/// and a recovery event. No retry loop: the daemon retries on its normal tick.
#[derive(Default)]
pub(crate) struct Health {
    failed: bool,
    last_report: u64,
}

#[derive(Debug, PartialEq)]
enum Event {
    Failure,
    Recovery,
    None,
}

impl Health {
    fn event(&mut self, ok: bool, ts: u64) -> Event {
        if ok {
            let was_failed = self.failed;
            self.failed = false;
            return if was_failed {
                Event::Recovery
            } else {
                Event::None
            };
        }
        if !self.failed || ts.saturating_sub(self.last_report) >= REMINDER_SECS {
            self.failed = true;
            self.last_report = ts;
            Event::Failure
        } else {
            Event::None
        }
    }

    pub(crate) fn report(&mut self, result: &Result<()>, ts: u64) {
        match self.event(result.is_ok(), ts) {
            Event::Failure => {
                if let Err(error) = result {
                    log(format_args!(
                        "{} persistence failed; retrying next tick: {error:#}",
                        stamp_utc(ts)
                    ));
                }
                alert(
                    "autoTrim cannot save monitoring data",
                    "Monitoring continues, but saved views or history may be stale. Check disk space and data directory permissions; autoTrim will retry next tick.",
                );
            }
            Event::Recovery => {
                log(format_args!("{} persistence recovered", stamp_utc(ts)));
                alert(
                    "autoTrim storage recovered",
                    "Snapshots, history, and state are being saved again.",
                );
            }
            Event::None => (),
        }
    }
}

#[derive(Serialize)]
struct CrashReport<'a> {
    timestamp: u64,
    version: &'static str,
    pid: u32,
    kind: &'a str,
    detail: &'a str,
    backtrace: Option<String>,
}

/// Reports stay local. Do not include panic payloads (which can contain user
/// data); a source location and stack are enough to locate the failing code.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".into());
        report_crash(
            "panic",
            &location,
            Some(std::backtrace::Backtrace::force_capture().to_string()),
        );
    }));
}

pub fn fatal(error: &anyhow::Error) {
    report_crash("fatal error", &format!("{error:#}"), None);
}

fn report_crash(kind: &str, detail: &str, backtrace: Option<String>) {
    let ts = now();
    let report = CrashReport {
        timestamp: ts,
        version: env!("CARGO_PKG_VERSION"),
        pid: std::process::id(),
        kind,
        detail,
        backtrace,
    };
    let mut recent = false;
    let saved = (|| -> Result<()> {
        let dir = paths::data_dir().ok_or_else(|| anyhow::anyhow!("no data directory"))?;
        let path = dir.join("crash.json");
        recent = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age.as_secs() < REMINDER_SECS);
        storage::write_atomic(&path, &serde_json::to_vec_pretty(&report)?)
    })();
    let line = format!(
        "{} autoTrim {kind}: {detail}; local crash report {}\n",
        stamp_utc(ts),
        if saved.is_ok() {
            "saved to crash.json"
        } else {
            "could not be saved"
        }
    );
    // Rotate even when startup failed before the first tick. A nonblocking
    // append avoids deadlocking if a panic interrupted a normal log write.
    let logged = paths::data_dir().is_some_and(|dir| {
        storage::append_diagnostic(&dir.join("daemon.log"), line.as_bytes()).is_ok()
    });
    if !logged {
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }
    if !recent {
        alert(
            "autoTrim daemon stopped unexpectedly",
            "Check crash.json in the autoTrim data directory. If installed as a login service, launchd will try to restart it.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_rate_limited_and_recovery_resets_the_limit() {
        let mut health = Health::default();
        assert_eq!(health.event(true, 0), Event::None);
        assert_eq!(health.event(false, 1), Event::Failure);
        assert_eq!(health.event(false, 2), Event::None);
        assert_eq!(health.event(false, 3601), Event::Failure);
        assert_eq!(health.event(true, 3602), Event::Recovery);
        assert_eq!(health.event(true, 3603), Event::None);
        assert_eq!(health.event(false, 3604), Event::Failure);
    }

    #[test]
    fn panic_hook_child() {
        if std::env::var_os("AUTOTRIM_TEST_PANIC").is_none() {
            return;
        }
        set_notifications(false);
        install_panic_hook();
        panic!("SECRET_PROMPT_MUST_NOT_APPEAR");
    }

    #[test]
    fn panic_hook_writes_a_local_report_without_the_payload() {
        let dir = crate::storage::tests::Scratch::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostics::tests::panic_hook_child",
                "--nocapture",
            ])
            .env("AUTOTRIM_TEST_PANIC", "1")
            .env("AUTOTRIM_DATA_DIR", &dir.0)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let text = std::fs::read_to_string(dir.0.join("crash.json")).unwrap();
        let report: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(report["kind"], "panic");
        assert!(
            report["detail"]
                .as_str()
                .unwrap()
                .contains("diagnostics.rs")
        );
        assert!(!report["backtrace"].as_str().unwrap().is_empty());
        assert!(!text.contains("SECRET_PROMPT_MUST_NOT_APPEAR"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET_PROMPT_MUST_NOT_APPEAR"));
    }
}
