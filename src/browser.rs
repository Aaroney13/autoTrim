//! Chromium-family browsers: how many renderers, how many profiles, and
//! which tabs are open.
//!
//! Tabs come from the browser's own session files (see `snss`), found
//! through the files the browser process holds open, so only profiles that
//! are actually running are counted and the browser is never asked
//! anything. Chrome does not publish per-tab memory to anyone outside
//! itself; the per-tab figure here is renderer memory divided by open tabs
//! and is labelled an estimate wherever it is shown.

use crate::automation;
use crate::groups::AppGroup;
use crate::openfiles::open_files;
use crate::procs::ProcTable;
use crate::snss;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A browser by the name `groups::app_name` reports, and where it keeps
/// its user data on each platform relative to that platform's app-data
/// root. Empty means unknown there. Only the current platform reads its
/// field.
#[allow(dead_code)]
struct Browser {
    name: &'static str,
    macos: &'static str,
    linux: &'static str,
    windows: &'static str,
}

const BROWSERS: &[Browser] = &[
    Browser {
        name: "Google Chrome",
        macos: "Google/Chrome",
        linux: "google-chrome",
        windows: "Google/Chrome/User Data",
    },
    Browser {
        name: "Chromium",
        macos: "Chromium",
        linux: "chromium",
        windows: "Chromium/User Data",
    },
    Browser {
        name: "Brave Browser",
        macos: "BraveSoftware/Brave-Browser",
        linux: "BraveSoftware/Brave-Browser",
        windows: "BraveSoftware/Brave-Browser/User Data",
    },
    Browser {
        name: "Microsoft Edge",
        macos: "Microsoft Edge",
        linux: "microsoft-edge",
        windows: "Microsoft/Edge/User Data",
    },
    Browser {
        name: "Arc",
        macos: "Arc/User Data",
        linux: "",
        windows: "",
    },
    Browser {
        name: "Vivaldi",
        macos: "Vivaldi",
        linux: "vivaldi",
        windows: "Vivaldi/User Data",
    },
];

/// Browsers whose AppleScript tab `id` is the session id from the session
/// file, so a tab we read can be closed by that number. Arc has its own
/// model of spaces and is left alone.
const CLOSE_BY_ID: &[&str] = &[
    "Google Chrome",
    "Chromium",
    "Brave Browser",
    "Microsoft Edge",
    "Vivaldi",
];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BrowserInfo {
    pub name: String,
    pub rss: u64,
    pub procs: usize,
    /// Renderer processes, excluding extension processes. Not the same as
    /// tabs: cross-site frames, prerenders and the spare renderer count too.
    pub renderers: usize,
    /// Renderers at least 40 MB, which is roughly "a real tab". The tab
    /// count to fall back on where the session files cannot be read.
    #[serde(default)]
    pub tab_sized_renderers: usize,
    #[serde(default)]
    pub small_renderers: usize,
    pub extension_renderers: usize,
    pub gpu: usize,
    pub utility: usize,
    /// Number of user profiles on disk. None where not implemented.
    pub profiles: Option<usize>,
    /// Resident bytes summed over the tab renderers.
    #[serde(default)]
    pub renderer_rss: u64,
    /// Every open tab in every running profile, window order.
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
    /// Windows with at least one tab, over the running profiles.
    #[serde(default)]
    pub windows: usize,
    /// The profiles with windows open right now.
    #[serde(default)]
    pub open_profiles: Vec<ProfileInfo>,
    /// Tabs rolled up by site, worst first.
    #[serde(default)]
    pub sites: Vec<SiteInfo>,
    /// Tabs not looked at for longer than the threshold. The active tab of
    /// each window is never counted.
    #[serde(default)]
    pub stale_tabs: usize,
    /// Renderer memory divided by open tabs. An estimate, not a measurement.
    #[serde(default)]
    pub per_tab_estimate: Option<u64>,
    /// Whether `actions::close_tabs_by_id` can act on this browser here.
    #[serde(default)]
    pub can_close_tabs: bool,
    /// Why there are no tabs, when there are none.
    #[serde(default)]
    pub tabs_note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TabInfo {
    /// The browser's session id for the tab. Stable for the tab's life.
    pub id: i32,
    pub window_id: i32,
    /// Profile directory name, e.g. "Default" or "Profile 2".
    pub profile: String,
    /// Position in its window.
    pub index: i32,
    pub url: String,
    /// The site, for grouping: the host without a leading "www.".
    pub site: String,
    pub title: String,
    pub pinned: bool,
    /// The selected tab in its window.
    pub active: bool,
    /// Epoch seconds when the tab was last the selected one.
    pub last_active: Option<u64>,
    /// Seconds since then. None when the browser never recorded a switch
    /// to this tab, which usually means it was opened in the background and
    /// never looked at.
    pub idle_secs: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProfileInfo {
    pub dir: String,
    /// The profile's display name from the browser's `Local State`.
    pub name: String,
    pub email: Option<String>,
    /// `name`, with the email appended when two open profiles share a name.
    pub label: String,
    pub tabs: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SiteInfo {
    pub site: String,
    pub tabs: usize,
    pub stale_tabs: usize,
    /// Longest idle among the site's tabs.
    pub oldest_idle_secs: Option<u64>,
    /// `tabs` times the per-tab estimate.
    pub est_rss: u64,
}

struct Cached<T> {
    len: u64,
    mtime: u64,
    value: T,
}

type ProfileNames = HashMap<String, (String, Option<String>)>;

/// Parsed session files and profile names, keyed by path and refreshed
/// only when the file changes. A daemon tick that finds nothing new costs a
/// stat per file.
static SESSIONS: LazyLock<Mutex<HashMap<PathBuf, Cached<Vec<snss::Tab>>>>> =
    LazyLock::new(Default::default);
static NAMES: LazyLock<Mutex<HashMap<PathBuf, Cached<ProfileNames>>>> =
    LazyLock::new(Default::default);

fn stamp(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((m.len(), mtime))
}

fn cached<T: Clone>(
    cache: &Mutex<HashMap<PathBuf, Cached<T>>>,
    path: &Path,
    load: impl FnOnce(&Path) -> Option<T>,
) -> Option<T> {
    let (len, mtime) = stamp(path)?;
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = map.get(path)
        && c.len == len
        && c.mtime == mtime
    {
        return Some(c.value.clone());
    }
    let value = load(path)?;
    map.insert(
        path.to_path_buf(),
        Cached {
            len,
            mtime,
            value: value.clone(),
        },
    );
    Some(value)
}

fn session_tabs(path: &Path) -> Vec<snss::Tab> {
    cached(&SESSIONS, path, |p| {
        let bytes = std::fs::read(p).ok()?;
        Some(snss::parse(&bytes)?.live_tabs())
    })
    .unwrap_or_default()
}

/// Display name and account for each profile directory, from the browser's
/// `Local State` file next to the profiles.
fn profile_names(user_data_dir: &Path) -> ProfileNames {
    cached(&NAMES, &user_data_dir.join("Local State"), |p| {
        let raw = std::fs::read(p).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
        let cache = v.get("profile")?.get("info_cache")?.as_object()?;
        Some(
            cache
                .iter()
                .map(|(dir, info)| {
                    let name = info
                        .get("name")
                        .and_then(|n| n.as_str())
                        .filter(|n| !n.is_empty())
                        .unwrap_or(dir)
                        .to_string();
                    let email = info
                        .get("user_name")
                        .and_then(|n| n.as_str())
                        .filter(|n| !n.is_empty())
                        .map(str::to_string);
                    (dir.clone(), (name, email))
                })
                .collect(),
        )
    })
    .unwrap_or_default()
}

/// The site a URL belongs to: the host, lower-cased, without `www.`, port
/// or credentials. Non-web schemes keep their scheme so `chrome://settings`
/// and `file:` pages group sensibly.
pub fn site_of(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.split(':').next().unwrap_or(url).to_ascii_lowercase();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut host = &rest[..end];
    if let Some((_, h)) = host.rsplit_once('@') {
        host = h;
    }
    if let Some((h, port)) = host.rsplit_once(':')
        && !h.contains(']')
        && port.chars().all(|c| c.is_ascii_digit())
    {
        host = h;
    }
    let host = host.strip_prefix("www.").unwrap_or(host);
    match scheme {
        "http" | "https" => host.to_ascii_lowercase(),
        other if host.is_empty() => format!("{}:", other.to_ascii_lowercase()),
        other => format!(
            "{}://{}",
            other.to_ascii_lowercase(),
            host.to_ascii_lowercase()
        ),
    }
}

/// Session files the browser process holds open, one per running profile:
/// `<user data>/<profile>/Sessions/Session_<n>`. When Chrome is mid-rotation
/// two are open for one profile; the newest wins.
fn live_session_files(mains: &[u32]) -> BTreeMap<PathBuf, PathBuf> {
    let mut by_profile: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for pid in mains {
        for p in open_files(*pid) {
            let is_session = p
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.starts_with("Session_"));
            let in_sessions_dir = p
                .parent()
                .and_then(|d| d.file_name())
                .is_some_and(|d| d == "Sessions");
            if !(is_session && in_sessions_dir) {
                continue;
            }
            let Some(profile_dir) = p.parent().and_then(|d| d.parent()) else {
                continue;
            };
            let e = by_profile
                .entry(profile_dir.to_path_buf())
                .or_insert_with(|| p.clone());
            if p.file_name() > e.file_name() {
                *e = p;
            }
        }
    }
    by_profile
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read the open tabs of every running profile and fill the tab-related
/// fields of `b`.
fn fill_tabs(b: &mut BrowserInfo, mains: &[u32], stale_after: u64) {
    if mains.is_empty() {
        b.tabs_note = Some("no browser process found".to_string());
        return;
    }
    let files = live_session_files(mains);
    if files.is_empty() {
        b.tabs_note = Some(if cfg!(any(target_os = "macos", target_os = "linux")) {
            "the browser holds no session file open, so no window is being recorded (private windows are never written)".to_string()
        } else {
            "tab listing is not implemented on this platform yet".to_string()
        });
        return;
    }
    let now = now_epoch();
    let mut seen = HashSet::new();
    for (profile_dir, session_file) in &files {
        seen.insert(session_file.clone());
        let dir = profile_dir
            .file_name()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_else(|| "?".to_string());
        let names = profile_dir.parent().map(profile_names).unwrap_or_default();
        let (name, email) = names
            .get(&dir)
            .cloned()
            .unwrap_or_else(|| (dir.clone(), None));
        let tabs = session_tabs(session_file);
        b.open_profiles.push(ProfileInfo {
            dir: dir.clone(),
            name,
            email,
            label: String::new(),
            tabs: tabs.len(),
        });
        for t in tabs {
            let idle_secs = t.last_active.map(|la| now.saturating_sub(la));
            b.tabs.push(TabInfo {
                id: t.id,
                window_id: t.window_id,
                profile: dir.clone(),
                index: t.index,
                site: site_of(&t.url),
                url: t.url,
                title: t.title,
                pinned: t.pinned,
                active: t.active,
                last_active: t.last_active,
                idle_secs,
            });
        }
    }
    SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|p, _| seen.contains(p));

    // Two open profiles both called "Aaron" need the account to tell apart.
    let mut name_count: HashMap<&str, usize> = HashMap::new();
    for p in &b.open_profiles {
        *name_count.entry(p.name.as_str()).or_default() += 1;
    }
    let labels: Vec<String> = b
        .open_profiles
        .iter()
        .map(|p| match (&p.email, name_count.get(p.name.as_str())) {
            (Some(e), Some(n)) if *n > 1 => format!("{} ({e})", p.name),
            _ => p.name.clone(),
        })
        .collect();
    for (p, l) in b.open_profiles.iter_mut().zip(labels) {
        p.label = l;
    }

    if b.tabs.is_empty() {
        b.tabs_note = Some("no tabs recorded in the open profiles".to_string());
        return;
    }
    b.windows = b
        .tabs
        .iter()
        .map(|t| (t.profile.as_str(), t.window_id))
        .collect::<HashSet<_>>()
        .len();
    b.per_tab_estimate = (b.renderer_rss > 0).then(|| b.renderer_rss / b.tabs.len() as u64);
    let per_tab = b.per_tab_estimate.unwrap_or(0);
    let is_stale = |t: &TabInfo| !t.active && t.idle_secs.is_some_and(|i| i >= stale_after);
    b.stale_tabs = b.tabs.iter().filter(|t| is_stale(t)).count();

    let mut sites: HashMap<&str, SiteInfo> = HashMap::new();
    for t in &b.tabs {
        let s = sites.entry(t.site.as_str()).or_insert_with(|| SiteInfo {
            site: t.site.clone(),
            tabs: 0,
            stale_tabs: 0,
            oldest_idle_secs: None,
            est_rss: 0,
        });
        s.tabs += 1;
        if is_stale(t) {
            s.stale_tabs += 1;
        }
        if let Some(i) = t.idle_secs
            && !t.active
            && s.oldest_idle_secs.is_none_or(|o| i > o)
        {
            s.oldest_idle_secs = Some(i);
        }
        s.est_rss = s.tabs as u64 * per_tab;
    }
    let mut ranked: Vec<SiteInfo> = sites.into_values().collect();
    ranked.sort_by(|a, c| {
        c.stale_tabs
            .cmp(&a.stale_tabs)
            .then(c.tabs.cmp(&a.tabs))
            .then(c.oldest_idle_secs.cmp(&a.oldest_idle_secs))
            .then(a.site.cmp(&c.site))
    });
    b.sites = ranked;
}

pub fn detect(
    table: &ProcTable,
    groups: &[AppGroup],
    tab_stale_after_secs: u64,
) -> Vec<BrowserInfo> {
    let mut out = Vec::new();
    for br in BROWSERS {
        let Some(g) = groups.iter().find(|g| g.name == br.name) else {
            continue;
        };
        let mut b = BrowserInfo {
            name: br.name.to_string(),
            rss: g.rss,
            procs: g.procs,
            renderers: 0,
            tab_sized_renderers: 0,
            small_renderers: 0,
            extension_renderers: 0,
            gpu: 0,
            utility: 0,
            profiles: profiles(br),
            renderer_rss: 0,
            tabs: Vec::new(),
            windows: 0,
            open_profiles: Vec::new(),
            sites: Vec::new(),
            stale_tabs: 0,
            per_tab_estimate: None,
            can_close_tabs: automation::AVAILABLE && CLOSE_BY_ID.contains(&br.name),
            tabs_note: None,
        };
        let mut mains = Vec::new();
        for pid in &g.pids {
            let Some(p) = table.get(*pid) else { continue };
            if p.cmd_has("--type=renderer") {
                if p.cmd_has("--extension-process") {
                    b.extension_renderers += 1;
                } else {
                    b.renderers += 1;
                    b.renderer_rss += p.rss;
                    if p.rss >= 40 * 1024 * 1024 {
                        b.tab_sized_renderers += 1;
                    } else {
                        b.small_renderers += 1;
                    }
                }
            } else if p.cmd_has("--type=gpu-process") {
                b.gpu += 1;
            } else if p.cmd_has("--type=utility") {
                b.utility += 1;
            } else if !p.cmd_has("--type=") {
                mains.push(p.pid);
            }
        }
        fill_tabs(&mut b, &mains, tab_stale_after_secs);
        out.push(b);
    }
    out
}

/// Ask the browser to close one tab, matched by id and then by URL so a tab
/// that navigated since the snapshot is left alone. Returns what happened.
pub fn close_tab(browser: &str, tab_id: i32, url: &str) -> anyhow::Result<String> {
    if !CLOSE_BY_ID.contains(&browser) {
        anyhow::bail!("{browser} tabs cannot be closed by id");
    }
    automation::close_tab(browser, tab_id, url)
}

/// Profiles are the directories under the user-data root that carry a
/// `Preferences` file, minus the two Chromium makes for itself.
fn profiles(b: &Browser) -> Option<usize> {
    let base = platform::user_data(b)?;
    let entries = std::fs::read_dir(base).ok()?;
    let n = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name != "System Profile" && name != "Guest Profile"
        })
        .filter(|e| e.path().join("Preferences").is_file())
        .count();
    Some(n)
}

fn non_empty(rel: &'static str) -> Option<&'static str> {
    (!rel.is_empty()).then_some(rel)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Browser, PathBuf, non_empty};
    use crate::system::home;

    pub fn user_data(b: &Browser) -> Option<PathBuf> {
        Some(
            home()?
                .join("Library/Application Support")
                .join(non_empty(b.macos)?),
        )
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Browser, PathBuf, non_empty};
    use crate::system::home;

    pub fn user_data(b: &Browser) -> Option<PathBuf> {
        let config = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(x) => PathBuf::from(x),
            None => home()?.join(".config"),
        };
        Some(config.join(non_empty(b.linux)?))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{Browser, PathBuf, non_empty};

    pub fn user_data(b: &Browser) -> Option<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA")?;
        Some(PathBuf::from(local).join(non_empty(b.windows)?))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    use super::{Browser, PathBuf};

    pub fn user_data(_b: &Browser) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites() {
        assert_eq!(site_of("https://www.github.com/a/b?x=1#y"), "github.com");
        assert_eq!(site_of("http://user:pw@Example.COM:8080/"), "example.com");
        assert_eq!(site_of("https://localhost:3000/app"), "localhost");
        assert_eq!(
            site_of("chrome://settings/performance"),
            "chrome://settings"
        );
        assert_eq!(site_of("file:///Users/a/doc.html"), "file:");
        assert_eq!(site_of("about:blank"), "about");
    }
}
