//! Chromium-family browsers: how many renderers, how many profiles.

use crate::groups::AppGroup;
use crate::procs::ProcTable;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BrowserInfo {
    pub name: String,
    pub rss: u64,
    pub procs: usize,
    /// Renderer processes, excluding extension processes. Not the same as
    /// tabs: cross-site frames, prerenders and the spare renderer count too.
    pub renderers: usize,
    /// Renderers at least 40 MB, which is roughly "a real tab".
    #[serde(default)]
    pub tab_sized_renderers: usize,
    #[serde(default)]
    pub small_renderers: usize,
    /// What the browser itself reports, when it can be asked (macOS).
    #[serde(default)]
    pub tabs: Option<usize>,
    #[serde(default)]
    pub windows: Option<usize>,
    pub extension_renderers: usize,
    pub gpu: usize,
    pub utility: usize,
    /// Number of user profiles on disk. None where not implemented.
    pub profiles: Option<usize>,
}

pub fn detect(table: &ProcTable, groups: &[AppGroup], count_tabs: bool) -> Vec<BrowserInfo> {
    let mut out = Vec::new();
    for b in BROWSERS {
        let Some(g) = groups.iter().find(|g| g.name == b.name) else {
            continue;
        };
        let mut info = BrowserInfo {
            name: b.name.to_string(),
            rss: g.rss,
            procs: g.procs,
            renderers: 0,
            tab_sized_renderers: 0,
            small_renderers: 0,
            tabs: None,
            windows: None,
            extension_renderers: 0,
            gpu: 0,
            utility: 0,
            profiles: profiles(b),
        };
        for pid in &g.pids {
            let Some(p) = table.get(*pid) else { continue };
            if p.cmd_has("--type=renderer") {
                if p.cmd_has("--extension-process") {
                    info.extension_renderers += 1;
                } else {
                    info.renderers += 1;
                    if p.rss >= 40 * 1024 * 1024 {
                        info.tab_sized_renderers += 1;
                    } else {
                        info.small_renderers += 1;
                    }
                }
            } else if p.cmd_has("--type=gpu-process") {
                info.gpu += 1;
            } else if p.cmd_has("--type=utility") {
                info.utility += 1;
            }
        }
        if count_tabs && let Some((tabs, windows)) = tab_count(b.name) {
            info.tabs = Some(tabs);
            info.windows = Some(windows);
        }
        out.push(info);
    }
    out
}

/// (tabs, windows) as the browser reports them over AppleScript. A browser
/// that refuses (permission denied, not scriptable) is not asked again for
/// ten minutes, so a denied prompt costs nothing afterwards.
#[cfg(target_os = "macos")]
fn tab_count(app: &str) -> Option<(usize, usize)> {
    use std::collections::HashMap;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};
    static FAILED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    {
        let mut g = FAILED.lock().ok()?;
        let m = g.get_or_insert_with(HashMap::new);
        if m.get(app).is_some_and(|t| now.saturating_sub(*t) < 600) {
            return None;
        }
    }
    let script = format!(
        "tell application \"{app}\" to return ((count of every tab of every window) as string) & \",\" & ((count of windows) as string)"
    );
    let out = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let parsed = (|| {
        let mut it = text.trim().split(',').map(|s| s.trim().parse::<usize>());
        Some((it.next()?.ok()?, it.next()?.ok()?))
    })();
    if !out.status.success() || parsed.is_none() {
        if let Ok(mut g) = FAILED.lock() {
            g.get_or_insert_with(HashMap::new)
                .insert(app.to_string(), now);
        }
        return None;
    }
    parsed
}

#[cfg(not(target_os = "macos"))]
fn tab_count(_app: &str) -> Option<(usize, usize)> {
    None
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
