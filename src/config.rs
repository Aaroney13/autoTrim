//! Settings. A single `config.toml` in the data directory; every field has
//! a default, so the file may be absent or partial. Command-line flags win
//! over the file, the file wins over the defaults.

use crate::paths;
use crate::rules::Thresholds;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Seconds between daemon samples.
    pub interval_secs: u64,
    /// Rolling window, in seconds, for per-session CPU averaging.
    pub window_secs: u64,
    /// Days of history kept on disk.
    pub retention_days: u64,

    /// Send native notifications.
    pub notify: bool,
    /// Hours before persisting advice is notified again.
    pub remind_every_hours: f64,

    /// A quiet session idle longer than this is stale.
    pub stale_after_hours: f64,
    /// Minutes a session must be observed quiet before it can be called
    /// stale when there is no transcript evidence.
    pub min_quiet_minutes: u64,
    /// CPU percent below which a session counts as quiet.
    pub quiet_cpu: f32,

    /// Suggest a restart once uptime passes this and swap is this full.
    pub restart_uptime_days: f64,
    pub restart_swap_pct: f64,
    /// Swap fullness that counts as "under pressure" for the softer rules.
    pub pressure_swap_pct: f64,

    /// Browser advice fires at this many live tab renderers or profiles.
    pub browser_renderers: usize,
    pub browser_profiles: usize,
    /// Under pressure, an ordinary app holding more than this is named.
    pub heavy_app_mb: u64,

    /// A local server (not an app, not an agent) listening longer than this
    /// while quiet is reported as an old server.
    pub port_stale_after_hours: f64,

    /// Ports never to report.
    pub ignore_ports: Vec<u16>,
    /// App group names never to report (as shown in the report).
    pub ignore_apps: Vec<String>,
    /// Sessions whose project path contains one of these are never reported.
    pub ignore_projects: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            interval_secs: 30,
            window_secs: 600,
            retention_days: 7,
            notify: true,
            remind_every_hours: 4.0,
            stale_after_hours: 6.0,
            min_quiet_minutes: 15,
            quiet_cpu: 2.0,
            restart_uptime_days: 14.0,
            restart_swap_pct: 75.0,
            pressure_swap_pct: 50.0,
            browser_renderers: 50,
            browser_profiles: 2,
            heavy_app_mb: 600,
            port_stale_after_hours: 24.0,
            ignore_ports: Vec::new(),
            ignore_apps: Vec::new(),
            ignore_projects: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        paths::data_dir().map(|d| d.join("config.toml"))
    }

    /// The effective config and, when one was read, the file it came from.
    pub fn load() -> Result<(Config, Option<PathBuf>)> {
        let Some(path) = Self::path() else {
            return Ok((Config::default(), None));
        };
        if !path.is_file() {
            return Ok((Config::default(), None));
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok((cfg, Some(path)))
    }

    pub fn thresholds(&self) -> Thresholds {
        Thresholds {
            restart_uptime_secs: (self.restart_uptime_days * 86_400.0) as u64,
            restart_swap_frac: self.restart_swap_pct / 100.0,
            stale_after_secs: (self.stale_after_hours * 3_600.0) as u64,
            browser_renderers: self.browser_renderers,
            browser_profiles: self.browser_profiles,
            heavy_app_bytes: self.heavy_app_mb * 1024 * 1024,
            pressure_swap_frac: self.pressure_swap_pct / 100.0,
            quiet_cpu: self.quiet_cpu,
            min_quiet_secs: self.min_quiet_minutes * 60,
            port_stale_after_secs: (self.port_stale_after_hours * 3_600.0) as u64,
            ignore_ports: self.ignore_ports.clone(),
            ignore_apps: self.ignore_apps.clone(),
            ignore_projects: self.ignore_projects.clone(),
        }
    }

    /// The defaults as a commented file, for `autotrim config init`.
    pub fn template() -> String {
        let d = Config::default();
        format!(
            r#"# autoTrim settings. Every key is optional; missing keys use the defaults
# shown here. Command-line flags override this file.

# Daemon
interval_secs = {interval_secs}          # seconds between samples
window_secs = {window_secs}            # rolling window for per-session CPU
retention_days = {retention_days}           # days of history kept

# Notifications
notify = {notify}
remind_every_hours = {remind_every_hours:.1}      # re-notify persisting advice after this long

# Agent sessions
stale_after_hours = {stale_after_hours:.1}       # idle longer than this is stale
min_quiet_minutes = {min_quiet_minutes}       # observed-quiet minimum without transcript evidence
quiet_cpu = {quiet_cpu:.1}               # CPU percent below which a session is quiet

# The machine
restart_uptime_days = {restart_uptime_days:.1}
restart_swap_pct = {restart_swap_pct:.0}
pressure_swap_pct = {pressure_swap_pct:.0}

# Browsers and heavy apps
browser_renderers = {browser_renderers}
browser_profiles = {browser_profiles}
heavy_app_mb = {heavy_app_mb}

# Local servers
port_stale_after_hours = {port_stale_after_hours:.1}   # a quiet non-app listener older than this is reported

# Never report these
ignore_ports = []               # e.g. [5432, 6379]
ignore_apps = []                # e.g. ["Spotify"]
ignore_projects = []            # substrings of project paths, e.g. ["/long-running-job"]
"#,
            interval_secs = d.interval_secs,
            window_secs = d.window_secs,
            retention_days = d.retention_days,
            notify = d.notify,
            remind_every_hours = d.remind_every_hours,
            stale_after_hours = d.stale_after_hours,
            min_quiet_minutes = d.min_quiet_minutes,
            quiet_cpu = d.quiet_cpu,
            restart_uptime_days = d.restart_uptime_days,
            restart_swap_pct = d.restart_swap_pct,
            pressure_swap_pct = d.pressure_swap_pct,
            browser_renderers = d.browser_renderers,
            browser_profiles = d.browser_profiles,
            heavy_app_mb = d.heavy_app_mb,
            port_stale_after_hours = d.port_stale_after_hours,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        let parsed: Config = toml::from_str(&Config::template()).unwrap();
        assert_eq!(parsed.interval_secs, Config::default().interval_secs);
        assert_eq!(
            parsed.port_stale_after_hours,
            Config::default().port_stale_after_hours
        );
        assert!(parsed.ignore_ports.is_empty());
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let parsed: Config =
            toml::from_str("stale_after_hours = 2\nignore_ports = [5432]").unwrap();
        assert_eq!(parsed.stale_after_hours, 2.0);
        assert_eq!(parsed.ignore_ports, vec![5432]);
        assert_eq!(parsed.interval_secs, 30);
    }
}
