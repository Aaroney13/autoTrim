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
    /// A tab not looked at for this long is stale.
    pub tab_stale_after_hours: f64,
    /// Browser advice also fires at this many stale tabs.
    pub browser_stale_tabs: usize,
    /// Conversation advice fires at this many stale chat-UI tabs.
    pub chat_stale_tabs: usize,
    /// Under pressure, an ordinary app holding more than this is named.
    pub heavy_app_mb: u64,

    /// A local server (not an app, not an agent) listening longer than this
    /// while quiet is reported as an old server.
    pub port_stale_after_hours: f64,

    /// Minutes of history the daemon keeps per app and session for trends.
    pub trend_window_minutes: u64,
    /// Steady growth faster than this is reported as leak-like.
    pub growth_mb_per_hour: u64,
    /// And it must have grown at least this much in the window.
    pub growth_min_mb: u64,
    /// Sustained CPU (100 = one core) over `cpu_hog_minutes` is reported.
    pub cpu_hog_pct: f32,
    pub cpu_hog_minutes: u64,
    /// Swap growing by more than this in an hour is "pressure rising".
    pub pressure_rise_mb_per_hour: u64,

    /// Send a short HTTP request to unlabelled local TCP ports to identify
    /// them (Vite, Next.js, Flask, ...). Loopback only, once per port.
    pub probe_ports: bool,
    pub probe_timeout_ms: u64,

    /// Close stale agent sessions on a timer. Off by default.
    pub auto_close_sessions: bool,
    /// Stop old local servers on a timer. Off by default.
    pub auto_stop_servers: bool,
    /// Minutes between "will close" and closing. Anything that becomes
    /// active in between is spared.
    pub auto_grace_minutes: u64,
    /// Log and notify what auto mode would do, without doing it.
    pub auto_dry_run: bool,
    /// Hosts whose sessions auto mode may close. A host that respawns its
    /// sessions makes closing pointless, so this is an allowlist.
    pub auto_hosts: Vec<String>,

    /// The menu bar app opens its window when it starts. Off leaves only
    /// the menu bar item, for running in the background.
    pub open_window_at_launch: bool,

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
            tab_stale_after_hours: 24.0,
            browser_stale_tabs: 15,
            chat_stale_tabs: 2,
            heavy_app_mb: 600,
            port_stale_after_hours: 24.0,
            trend_window_minutes: 120,
            growth_mb_per_hour: 200,
            growth_min_mb: 150,
            cpu_hog_pct: 90.0,
            cpu_hog_minutes: 10,
            pressure_rise_mb_per_hour: 1024,
            probe_ports: true,
            probe_timeout_ms: 300,
            auto_close_sessions: false,
            auto_stop_servers: false,
            auto_grace_minutes: 10,
            auto_dry_run: false,
            auto_hosts: vec![
                "Claude app".to_string(),
                "VS Code".to_string(),
                "Cursor".to_string(),
                "terminal".to_string(),
            ],
            open_window_at_launch: true,
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
            tab_stale_after_secs: (self.tab_stale_after_hours * 3_600.0) as u64,
            browser_stale_tabs: self.browser_stale_tabs,
            chat_stale_tabs: self.chat_stale_tabs,
            heavy_app_bytes: self.heavy_app_mb * 1024 * 1024,
            pressure_swap_frac: self.pressure_swap_pct / 100.0,
            quiet_cpu: self.quiet_cpu,
            min_quiet_secs: self.min_quiet_minutes * 60,
            port_stale_after_secs: (self.port_stale_after_hours * 3_600.0) as u64,
            trend_window_secs: self.trend_window_minutes * 60,
            growth_bytes_per_hour: self.growth_mb_per_hour * 1024 * 1024,
            growth_min_bytes: self.growth_min_mb * 1024 * 1024,
            cpu_hog_pct: self.cpu_hog_pct,
            cpu_hog_secs: self.cpu_hog_minutes * 60,
            pressure_rise_bytes_per_hour: self.pressure_rise_mb_per_hour * 1024 * 1024,
            probe_ports: self.probe_ports,
            probe_timeout_ms: self.probe_timeout_ms,
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
tab_stale_after_hours = {tab_stale_after_hours:.1}     # a tab not looked at for this long is stale
browser_stale_tabs = {browser_stale_tabs}           # advice also fires at this many stale tabs
chat_stale_tabs = {chat_stale_tabs}              # and at this many stale conversation tabs (chatgpt.com, claude.ai, ...)
heavy_app_mb = {heavy_app_mb}

# Local servers
port_stale_after_hours = {port_stale_after_hours:.1}   # a quiet non-app listener older than this is reported

# Trends (daemon only)
trend_window_minutes = {trend_window_minutes}       # history kept per app and session
growth_mb_per_hour = {growth_mb_per_hour}         # steady growth above this looks like a leak
growth_min_mb = {growth_min_mb}              # and must have grown at least this much
cpu_hog_pct = {cpu_hog_pct:.0}                 # sustained CPU, 100 = one full core
cpu_hog_minutes = {cpu_hog_minutes}
pressure_rise_mb_per_hour = {pressure_rise_mb_per_hour}   # swap growing faster than this names the culprits

# Port labels
probe_ports = {probe_ports}              # identify local HTTP ports with one short request each
probe_timeout_ms = {probe_timeout_ms}

# Auto mode. Off by default. Try auto_dry_run = true first: it logs and
# notifies what it would have closed, and closes nothing.
auto_close_sessions = {auto_close_sessions}
auto_stop_servers = {auto_stop_servers}
auto_grace_minutes = {auto_grace_minutes}           # warning first, then this long before acting
auto_dry_run = {auto_dry_run}
auto_hosts = ["Claude app", "VS Code", "Cursor", "terminal"]   # sessions under other hosts are never auto-closed;
                                                              # an app's own agent engine never is

# Menu bar app
open_window_at_launch = {open_window_at_launch}   # false: start with only the menu bar item

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
            tab_stale_after_hours = d.tab_stale_after_hours,
            browser_stale_tabs = d.browser_stale_tabs,
            chat_stale_tabs = d.chat_stale_tabs,
            heavy_app_mb = d.heavy_app_mb,
            port_stale_after_hours = d.port_stale_after_hours,
            trend_window_minutes = d.trend_window_minutes,
            growth_mb_per_hour = d.growth_mb_per_hour,
            growth_min_mb = d.growth_min_mb,
            cpu_hog_pct = d.cpu_hog_pct,
            cpu_hog_minutes = d.cpu_hog_minutes,
            pressure_rise_mb_per_hour = d.pressure_rise_mb_per_hour,
            probe_ports = d.probe_ports,
            probe_timeout_ms = d.probe_timeout_ms,
            auto_close_sessions = d.auto_close_sessions,
            auto_stop_servers = d.auto_stop_servers,
            auto_grace_minutes = d.auto_grace_minutes,
            auto_dry_run = d.auto_dry_run,
            open_window_at_launch = d.open_window_at_launch,
        )
    }

    /// Whether auto mode does anything at all.
    pub fn auto_on(&self) -> bool {
        self.auto_close_sessions || self.auto_stop_servers
    }

    /// Whether `key` names a setting.
    pub fn has_key(key: &str) -> bool {
        toml::to_string(&Config::default())
            .map(|t| t.lines().any(|l| key_of(l) == Some(key)))
            .unwrap_or(false)
    }

    /// Change top-level keys in `config.toml`, keeping every other line as
    /// it is, comments included. The file is created from the template when
    /// there is none. The result is parsed before it is written, so a bad
    /// value never leaves behind a file the daemon cannot read. Values are
    /// TOML: `true`, `5`, `"name"`, `["a", "b"]`.
    pub fn set_values(pairs: &[(&str, String)]) -> Result<PathBuf> {
        let path = Self::path().context("no data directory on this platform")?;
        let text = if path.is_file() {
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
        } else {
            Self::template()
        };
        let edited = set_keys(&text, pairs);
        toml::from_str::<Config>(&edited).context("the edited settings would not parse")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, edited).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("renaming into {}", path.display()))?;
        Ok(path)
    }
}

/// The pure text edit behind `set_values`: replace the value on each key's
/// line, keeping its trailing comment, and append keys the file lacks.
fn set_keys(text: &str, pairs: &[(&str, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut missing = Vec::new();
    for (key, value) in pairs {
        match lines.iter().position(|l| key_of(l) == Some(key)) {
            Some(i) => {
                lines[i] = match trailing_comment(&lines[i]) {
                    Some(c) => format!("{key} = {value}   {c}"),
                    None => format!("{key} = {value}"),
                };
            }
            None => missing.push(format!("{key} = {value}")),
        }
    }
    if !missing.is_empty() {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("# Added by autotrim".to_string());
        lines.extend(missing);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The key a line assigns, when it is a top-level `key = value` line.
fn key_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let (k, _) = t.split_once('=')?;
    let k = k.trim();
    (!k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    .then_some(k)
}

/// The `# comment` at the end of a value line, ignoring a `#` inside a
/// quoted string.
fn trailing_comment(line: &str) -> Option<&str> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' => return Some(line[i..].trim_end()),
                _ => {}
            },
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_keys_replaces_value_and_keeps_comment() {
        let before = Config::template();
        let after = set_keys(
            &before,
            &[
                ("auto_close_sessions", "true".to_string()),
                ("auto_grace_minutes", "5".to_string()),
            ],
        );
        assert_eq!(before.lines().count(), after.lines().count());
        assert!(after.contains("auto_close_sessions = true\n"));
        assert!(
            after
                .contains("auto_grace_minutes = 5   # warning first, then this long before acting")
        );
        let parsed: Config = toml::from_str(&after).unwrap();
        assert!(parsed.auto_close_sessions);
        assert_eq!(parsed.auto_grace_minutes, 5);
        assert!(!parsed.auto_dry_run);
    }

    #[test]
    fn set_keys_appends_missing_keys() {
        let after = set_keys(
            "stale_after_hours = 2   # short\n",
            &[("auto_dry_run", "true".to_string())],
        );
        assert!(after.starts_with("stale_after_hours = 2   # short\n"));
        assert!(after.ends_with("# Added by autotrim\nauto_dry_run = true\n"));
        let parsed: Config = toml::from_str(&after).unwrap();
        assert!(parsed.auto_dry_run);
        assert_eq!(parsed.stale_after_hours, 2.0);
    }

    #[test]
    fn comments_inside_strings_are_not_comments() {
        assert_eq!(
            trailing_comment(r#"ignore_apps = ["a#b"]  # c"#),
            Some("# c")
        );
        assert_eq!(trailing_comment(r#"x = "a#b""#), None);
        assert_eq!(trailing_comment("x = 'it''s #1'  # d"), Some("# d"));
        assert_eq!(key_of("  auto_dry_run=false"), Some("auto_dry_run"));
        assert_eq!(key_of("# auto_dry_run = false"), None);
        assert_eq!(key_of("[table]"), None);
    }

    #[test]
    fn has_key_knows_the_settings() {
        assert!(Config::has_key("auto_close_sessions"));
        assert!(Config::has_key("open_window_at_launch"));
        assert!(!Config::has_key("auto_close_session"));
    }

    #[test]
    fn template_parses_to_defaults() {
        let parsed: Config = toml::from_str(&Config::template()).unwrap();
        assert_eq!(parsed.interval_secs, Config::default().interval_secs);
        assert_eq!(
            parsed.port_stale_after_hours,
            Config::default().port_stale_after_hours
        );
        assert!(parsed.ignore_ports.is_empty());
        assert!(!parsed.auto_close_sessions);
        assert_eq!(
            parsed.browser_stale_tabs,
            Config::default().browser_stale_tabs
        );
        assert_eq!(parsed.chat_stale_tabs, Config::default().chat_stale_tabs);
        assert_eq!(
            parsed.tab_stale_after_hours,
            Config::default().tab_stale_after_hours
        );
        assert_eq!(parsed.auto_hosts, Config::default().auto_hosts);
        assert!(parsed.open_window_at_launch);
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
