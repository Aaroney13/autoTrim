//! Settings. A single `config.toml` in the data directory; every field has
//! a default, so the file may be absent or partial. Command-line flags win
//! over the file, the file wins over the defaults.

use crate::paths;
use crate::rules::Thresholds;
use crate::tab_rules::{DomainRule, serialize_rules, validate_domain_rules};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Internal file identity, not a user setting. Even edits that restore the
    /// previous values must invalidate a pending domain authorization.
    #[serde(skip)]
    pub file_revision: String,
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
    /// Close inactive tabs whose domains appear in `auto_tab_domains`.
    pub auto_close_tabs: bool,
    /// Shared inactivity threshold for listed domains.
    pub auto_tab_inactive_hours: f64,
    /// Domain allowlist for automatic Chrome tab cleanup.
    pub auto_tab_domains: Vec<DomainRule>,
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
    /// The first-run wizard has been saved successfully.
    pub onboarding_completed: bool,
    /// Categories to prioritize in the menu bar window.
    pub focus_areas: Vec<String>,

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
            file_revision: String::new(),
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
            auto_close_tabs: false,
            auto_tab_inactive_hours: 24.0,
            auto_tab_domains: Vec::new(),
            auto_grace_minutes: 10,
            auto_dry_run: false,
            auto_hosts: vec![
                "Claude app".to_string(),
                "VS Code".to_string(),
                "Cursor".to_string(),
                "terminal".to_string(),
            ],
            open_window_at_launch: true,
            onboarding_completed: false,
            focus_areas: vec!["browser".into(), "agent".into()],
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
        Self::load_at(&path)
    }

    fn load_at(path: &std::path::Path) -> Result<(Config, Option<PathBuf>)> {
        if !path.is_file() {
            return Ok((Config::default(), None));
        }
        let (text, revision) = read_config_text(path)?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate_tab_rules()
            .with_context(|| format!("validating {}", path.display()))?;
        cfg.file_revision = revision;
        Ok((cfg, Some(path.to_path_buf())))
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
auto_close_tabs = {auto_close_tabs}
auto_tab_inactive_hours = {auto_tab_inactive_hours}       # listed domains inactive this long may be closed
auto_tab_domains = []                  # exact domains by default; entries may opt into subdomains
auto_grace_minutes = {auto_grace_minutes}           # warning first, then this long before acting
auto_dry_run = {auto_dry_run}
auto_hosts = ["Claude app", "VS Code", "Cursor", "terminal"]   # sessions under other hosts are never auto-closed;
                                                              # an app's own agent engine never is

# Menu bar app
open_window_at_launch = {open_window_at_launch}   # false: start with only the menu bar item

# First-run setup and sidebar priorities
onboarding_completed = false
focus_areas = ["browser", "agent"]

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
            auto_close_tabs = d.auto_close_tabs,
            auto_tab_inactive_hours = d.auto_tab_inactive_hours,
            auto_grace_minutes = d.auto_grace_minutes,
            auto_dry_run = d.auto_dry_run,
            open_window_at_launch = d.open_window_at_launch,
        )
    }

    /// Whether auto mode does anything at all.
    pub fn auto_on(&self) -> bool {
        self.auto_close_sessions || self.auto_stop_servers || self.auto_close_tabs
    }

    pub fn validate_tab_rules(&mut self) -> Result<()> {
        self.auto_tab_domains =
            validate_domain_rules(&self.auto_tab_domains, self.auto_tab_inactive_hours)?;
        Ok(())
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
        set_values_at(&path, pairs)?;
        Ok(path)
    }

    /// Save edits only while the caller's configuration snapshot is current.
    pub fn set_values_if_unchanged(
        pairs: &[(&str, String)],
        expected_revision: &str,
    ) -> Result<PathBuf> {
        let path = Self::path().context("no data directory on this platform")?;
        set_values_checked_at(&path, pairs, Some(expected_revision))?;
        Ok(path)
    }
}

fn read_config_text(path: &std::path::Path) -> Result<(String, String)> {
    // Read and identify the same open file, including across atomic saves.
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let before = file.metadata()?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .with_context(|| format!("reading {}", path.display()))?;
    let after = file.metadata()?;
    let revision = file_revision(&after)?;
    anyhow::ensure!(
        file_revision(&before)? == revision,
        "settings changed while being read; retry"
    );
    Ok((text, revision))
}

pub(crate) fn file_revision(meta: &std::fs::Metadata) -> Result<String> {
    let revision = format!(
        "{:?}:{:?}:{}",
        meta.modified()?,
        meta.created().ok(),
        meta.len()
    );
    #[cfg(unix)]
    let revision = {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{revision}:{}:{}:{}:{}",
            meta.dev(),
            meta.ino(),
            meta.ctime(),
            meta.ctime_nsec()
        )
    };
    Ok(revision)
}

fn set_values_at(path: &std::path::Path, pairs: &[(&str, String)]) -> Result<()> {
    set_values_checked_at(path, pairs, None)
}

fn set_values_checked_at(
    path: &std::path::Path,
    pairs: &[(&str, String)],
    expected_revision: Option<&str>,
) -> Result<()> {
    let (text, revision) = if path.is_file() {
        read_config_text(path)?
    } else {
        (Config::template(), String::new())
    };
    if let Some(expected) = expected_revision {
        anyhow::ensure!(
            expected == revision,
            "settings changed; review the current rules and try again"
        );
    }
    let mut edited = set_keys(&text, pairs)?;
    let mut cfg =
        toml::from_str::<Config>(&edited).context("the edited settings would not parse")?;
    cfg.validate_tab_rules()
        .context("the edited auto tab settings are invalid")?;
    if pairs.iter().any(|(key, _)| *key == "auto_tab_domains") {
        let normalized = serialize_rules(&cfg.auto_tab_domains)?;
        edited = set_keys(&edited, &[("auto_tab_domains", normalized)])?;
    }
    if expected_revision.is_some() {
        let current = match std::fs::metadata(path) {
            Ok(meta) => file_revision(&meta)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            current == revision,
            "settings changed; review the current rules and try again"
        );
    }
    crate::storage::write_atomic(path, edited.as_bytes())?;
    Ok(())
}

/// The pure text edit behind `set_values`: replace assigned values while
/// preserving comments, including a multiline domain-rule array.
fn set_keys(text: &str, pairs: &[(&str, String)]) -> Result<String> {
    // Parser spans identify root values independently of quoted keys, nested
    // tables, and multiline arrays. Replacing only values preserves comments.
    let parsed: std::collections::BTreeMap<String, toml::Spanned<toml::Value>> =
        toml::from_str(text).context("parsing settings for edit")?;
    if let Some(rules) = parsed.get("auto_tab_domains") {
        let raw = text[rules.span()].trim_start();
        anyhow::ensure!(
            raw.starts_with('[') && !raw.starts_with("[["),
            "table-form auto_tab_domains is unsupported; use auto_tab_domains = [...] instead"
        );
    }
    let mut edits = Vec::new();
    let mut missing = Vec::new();
    // `config set` accepts repeated keys; retain the last requested value and
    // apply each source span only once, even if replacement lengths differ.
    let requested: std::collections::BTreeMap<_, _> =
        pairs.iter().map(|(key, value)| (*key, value)).collect();
    for (key, value) in requested {
        if let Some(old) = parsed.get(key) {
            edits.push((old.span(), value.clone()));
        } else {
            missing.push(format!("{key} = {value}"));
        }
    }
    if !missing.is_empty() {
        // Ignore bracket-looking lines inside a root value, such as an array
        // or multiline string, when finding the first table header.
        let values: Vec<_> = parsed
            .values()
            .filter(|v| !v.get_ref().is_table())
            .map(|v| v.span())
            .collect();
        let mut offset = 0;
        let mut attached_comments = None;
        let mut insert_at = text.len();
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') && !values.iter().any(|span| span.contains(&offset)) {
                insert_at = attached_comments.unwrap_or(offset);
                break;
            }
            if trimmed.trim().is_empty() || trimmed.starts_with('#') {
                attached_comments.get_or_insert(offset);
            } else {
                attached_comments = None;
            }
            offset += line.len();
        }
        let mut block = String::new();
        if insert_at > 0 && !text[..insert_at].ends_with("\n\n") {
            block.push('\n');
        }
        block.push_str("# Added by autotrim\n");
        block.push_str(&missing.join("\n"));
        block.push('\n');
        if insert_at < text.len() && !text[insert_at..].starts_with('\n') {
            block.push('\n');
        }
        edits.push((insert_at..insert_at, block));
    }
    edits.sort_by_key(|a| std::cmp::Reverse(a.0.start));
    let mut out = text.to_string();
    for (range, value) in edits {
        out.replace_range(range, &value);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
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
        )
        .unwrap();
        assert_eq!(before.lines().count(), after.lines().count());
        assert!(after.contains("auto_close_sessions = true\n"));
        let old_line = before
            .lines()
            .find(|l| l.starts_with("auto_grace_minutes ="))
            .unwrap();
        assert!(
            after
                .lines()
                .any(|line| line == old_line.replacen(" = 10", " = 5", 1))
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
        )
        .unwrap();
        assert!(after.starts_with("stale_after_hours = 2   # short\n"));
        assert!(after.ends_with("# Added by autotrim\nauto_dry_run = true\n"));
        let parsed: Config = toml::from_str(&after).unwrap();
        assert!(parsed.auto_dry_run);
        assert_eq!(parsed.stale_after_hours, 2.0);
    }

    #[test]
    fn repeated_key_updates_use_the_last_value_without_invalidating_spans() {
        let after = set_keys(
            "notify = false",
            &[("notify", "true".into()), ("notify", "false".into())],
        )
        .unwrap();
        assert!(!toml::from_str::<Config>(&after).unwrap().notify);
        let after = set_keys(
            "",
            &[
                ("auto_tab_inactive_hours", "100".into()),
                ("auto_tab_inactive_hours", "24".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            toml::from_str::<Config>(&after)
                .unwrap()
                .auto_tab_inactive_hours,
            24.0
        );
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
        assert!(!parsed.auto_close_tabs);
        assert_eq!(parsed.auto_tab_inactive_hours, 24.0);
        assert!(parsed.auto_tab_domains.is_empty());
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

    #[test]
    fn partial_file_gets_safe_auto_tab_defaults() {
        let mut parsed: Config = toml::from_str("notify = false\n").unwrap();
        assert!(!parsed.auto_close_tabs);
        assert_eq!(parsed.auto_tab_inactive_hours, 24.0);
        assert!(parsed.auto_tab_domains.is_empty());
        assert!(!parsed.auto_on());
        parsed.auto_close_tabs = true;
        assert!(parsed.auto_on());
    }

    #[test]
    fn ten_minute_tab_timer_and_existing_whole_hours_load_correctly() {
        use crate::daemon::{DaemonConfig, Overrides};
        let mut cfg: Config =
            toml::from_str("auto_tab_inactive_hours = 0.16666666666666666").unwrap();
        cfg.validate_tab_rules().unwrap();
        assert_eq!(
            DaemonConfig::from_config(&cfg, &Overrides::default())
                .auto
                .tab_inactive_secs,
            600
        );
        let mut legacy: Config = toml::from_str("auto_tab_inactive_hours = 24").unwrap();
        legacy.validate_tab_rules().unwrap();
        assert_eq!(
            DaemonConfig::from_config(&legacy, &Overrides::default())
                .auto
                .tab_inactive_secs,
            86_400
        );
    }

    #[test]
    fn validate_tab_rules_normalizes_domains_and_enforces_hours() {
        let mut cfg = Config {
            auto_tab_inactive_hours: 48.0,
            auto_tab_domains: vec![crate::tab_rules::DomainRule {
                domain: " Example.COM. ".to_string(),
                include_subdomains: true,
            }],
            ..Config::default()
        };
        cfg.validate_tab_rules().unwrap();
        assert_eq!(cfg.auto_tab_domains[0].domain, "example.com");

        cfg.auto_tab_inactive_hours = 0.0;
        assert!(cfg.validate_tab_rules().is_err());
    }

    #[test]
    fn set_keys_replaces_multiline_rules_without_touching_other_content() {
        let before = r#"# keep this
notify = true
auto_tab_domains = [
  { domain = "old.example", include_subdomains = false },
] # old rules
ignore_apps = ["name#tag"] # quoted hash
"#;
        let after = set_keys(
            before,
            &[(
                "auto_tab_domains",
                r#"[{ domain = "new.example", include_subdomains = true }]"#.to_string(),
            )],
        )
        .unwrap();
        assert!(after.contains("# keep this\nnotify = true\n"));
        assert!(after.contains(
            r#"auto_tab_domains = [{ domain = "new.example", include_subdomains = true }] # old rules"#
        ));
        assert!(after.contains(r#"ignore_apps = ["name#tag"] # quoted hash"#));
        assert!(!after.contains("old.example"));
    }

    #[test]
    fn set_keys_inserts_missing_root_key_before_first_table() {
        let before = "notify = true\n\n[future]\nvalue = 1\n";
        let after = set_keys(before, &[("auto_tab_inactive_hours", "12".to_string())]).unwrap();
        let inserted = after.find("auto_tab_inactive_hours = 12").unwrap();
        let table = after.find("[future]").unwrap();
        assert!(inserted < table);
    }

    #[test]
    fn set_keys_rejects_array_of_tables_rules() {
        let before = "notify = true\n[[auto_tab_domains]]\ndomain = \"example.com\"\n";
        assert!(set_keys(before, &[("notify", "false".to_string())]).is_err());
    }

    #[test]
    fn root_edits_preserve_nested_keys_and_table_comments() {
        let before = "notify = true\n\n# Future settings\n[future]\nauto_close_tabs = false\nauto_tab_domains = []\n";
        let after = set_keys(
            before,
            &[
                ("auto_close_tabs", "true".into()),
                ("auto_tab_domains", r#"[{domain = "example.com"}]"#.into()),
            ],
        )
        .unwrap();
        let cfg: Config = toml::from_str(&after).unwrap();
        assert!(cfg.auto_close_tabs);
        assert_eq!(cfg.auto_tab_domains[0].domain, "example.com");
        assert!(after.ends_with(
            "# Future settings\n[future]\nauto_close_tabs = false\nauto_tab_domains = []\n"
        ));
    }

    #[test]
    fn quoted_root_keys_and_table_form_rules_are_handled_without_data_loss() {
        let after = set_keys(
            "'auto_close_tabs' = false # keep\n",
            &[("auto_close_tabs", "true".into())],
        )
        .unwrap();
        assert!(toml::from_str::<Config>(&after).unwrap().auto_close_tabs);
        assert!(after.contains("# keep"));
        let table_form = "[[ 'auto_tab_domains' ]]\ndomain = 'example.com'\n";
        assert!(set_keys(table_form, &[("auto_tab_domains", "[]".into())]).is_err());
    }

    #[test]
    fn intervening_config_edits_never_revive_a_domain_warning() {
        use crate::{
            daemon::{DaemonConfig, Overrides},
            policy::{PolicyState, decide},
            test_support::chrome_snapshot,
        };
        let dir =
            std::env::temp_dir().join(format!("autotrim-config-revision-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let initial = "auto_close_tabs = true\nauto_tab_domains = [{domain = 'example.com'}]\n";
        let mut snap = chrome_snapshot();
        snap.taken_at = 100_000;
        snap.browsers[0].tabs[0].url = "https://example.com/page".into();
        snap.browsers[0].tabs[0].last_active = Some(1);
        for (key, revoked, restored) in [
            ("auto_close_tabs", "false", "true"),
            ("auto_dry_run", "true", "false"),
            ("auto_tab_domains", "[]", "[{domain = 'example.com'}]"),
        ] {
            crate::storage::write_atomic(&path, initial.as_bytes()).unwrap();
            let (file, _) = Config::load_at(&path).unwrap();
            let cfg = DaemonConfig::from_config(&file, &Overrides::default());
            let mut state = PolicyState::default();
            assert_eq!(decide(&mut state, &snap, &cfg, 100_000).pending.len(), 1);
            set_values_at(&path, &[(key, revoked.into())]).unwrap();
            set_values_at(&path, &[(key, restored.into())]).unwrap();
            // Also model a daemon restart, retaining only its persisted warnings.
            state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            let (file, _) = Config::load_at(&path).unwrap();
            let cfg = DaemonConfig::from_config(&file, &Overrides::default());
            let fresh = decide(&mut state, &snap, &cfg, 100_600);
            assert!(
                fresh.eligible.is_empty(),
                "revived warning after editing {key}"
            );
            assert_eq!(fresh.pending[0].due_at, 101_200);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_domain_save_cannot_restore_rules_removed_since_review() {
        let dir = std::env::temp_dir().join(format!("autotrim-stale-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        set_values_at(
            &path,
            &[("auto_tab_domains", "[{domain = 'removed.example'}]".into())],
        )
        .unwrap();
        let (reviewed, _) = Config::load_at(&path).unwrap();

        // An external edit lands before the UI's next settings poll.
        set_values_at(&path, &[("auto_tab_domains", "[]".into())]).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let stale = set_values_checked_at(
            &path,
            &[(
                "auto_tab_domains",
                "[{domain = 'removed.example'}, {domain = 'new.example'}]".into(),
            )],
            Some(&reviewed.file_revision),
        );
        assert!(stale.unwrap_err().to_string().contains("settings changed"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let (current, _) = Config::load_at(&path).unwrap();
        set_values_checked_at(
            &path,
            &[("auto_tab_domains", "[{domain = 'new.example'}]".into())],
            Some(&current.file_revision),
        )
        .unwrap();
        let (saved, _) = Config::load_at(&path).unwrap();
        assert_eq!(saved.auto_tab_domains.len(), 1);
        assert_eq!(saved.auto_tab_domains[0].domain, "new.example");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scratch_write_is_atomic_on_invalid_tab_settings_and_normalizes_valid_rules() {
        let dir = std::env::temp_dir().join(format!(
            "autotrim-config-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "notify = true\n").unwrap();

        let bad = set_values_at(&path, &[("auto_tab_inactive_hours", "0".to_string())]);
        assert!(bad.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "notify = true\n");

        set_values_at(
            &path,
            &[(
                "auto_tab_domains",
                r#"[{ domain = "EXAMPLE.COM.", include_subdomains = false }]"#.to_string(),
            )],
        )
        .unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains(r#"domain = "example.com""#));
        assert!(!saved.contains("EXAMPLE.COM."));
        let _ = std::fs::remove_dir_all(dir);
    }
}
