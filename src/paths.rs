//! Where the daemon keeps its state and history.
//!
//! Override with `AUTOTRIM_DATA_DIR` for tests or unusual setups.

use std::path::PathBuf;

pub fn data_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AUTOTRIM_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    default_dir()
}

#[cfg(target_os = "macos")]
fn default_dir() -> Option<PathBuf> {
    crate::system::home().map(|h| h.join("Library/Application Support/autotrim"))
}

#[cfg(target_os = "windows")]
fn default_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("autotrim"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(x).join("autotrim"));
    }
    crate::system::home().map(|h| h.join(".local/share/autotrim"))
}
