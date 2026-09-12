//! Chromium-family browsers: how many renderers, how many profiles.

use crate::groups::AppGroup;
use crate::procs::ProcTable;
use serde::{Deserialize, Serialize};

/// (bundle name, user-data directory relative to the platform app-data root)
const BROWSERS: &[(&str, &str)] = &[
    ("Google Chrome", "Google/Chrome"),
    ("Chromium", "Chromium"),
    ("Brave Browser", "BraveSoftware/Brave-Browser"),
    ("Microsoft Edge", "Microsoft Edge"),
    ("Arc", "Arc/User Data"),
    ("Vivaldi", "Vivaldi"),
];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BrowserInfo {
    pub name: String,
    pub rss: u64,
    pub procs: usize,
    /// Tab renderers, excluding extension processes.
    pub renderers: usize,
    pub extension_renderers: usize,
    pub gpu: usize,
    pub utility: usize,
    /// Number of user profiles on disk. None where not implemented.
    pub profiles: Option<usize>,
}

pub fn detect(table: &ProcTable, groups: &[AppGroup]) -> Vec<BrowserInfo> {
    let mut out = Vec::new();
    for (name, data_dir) in BROWSERS {
        let Some(g) = groups.iter().find(|g| g.name == *name) else {
            continue;
        };
        let mut b = BrowserInfo {
            name: name.to_string(),
            rss: g.rss,
            procs: g.procs,
            renderers: 0,
            extension_renderers: 0,
            gpu: 0,
            utility: 0,
            profiles: platform::profiles(data_dir),
        };
        for pid in &g.pids {
            let Some(p) = table.get(*pid) else { continue };
            if p.cmd_has("--type=renderer") {
                if p.cmd_has("--extension-process") {
                    b.extension_renderers += 1;
                } else {
                    b.renderers += 1;
                }
            } else if p.cmd_has("--type=gpu-process") {
                b.gpu += 1;
            } else if p.cmd_has("--type=utility") {
                b.utility += 1;
            }
        }
        out.push(b);
    }
    out
}

#[cfg(target_os = "macos")]
mod platform {
    use crate::system::home;

    pub fn profiles(data_dir: &str) -> Option<usize> {
        let base = home()?.join("Library/Application Support").join(data_dir);
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
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn profiles(_data_dir: &str) -> Option<usize> {
        None
    }
}
