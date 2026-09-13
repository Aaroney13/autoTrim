//! Open the companion app from the command line. The menu bar app is a
//! separate binary; `autotrim open` finds it and hands off, whether it was
//! installed to Applications or only built in this tree.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const APP_NAME: &str = "autoTrim";

fn tray_binary() -> &'static str {
    if cfg!(windows) {
        "autotrim-tray.exe"
    } else {
        "autotrim-tray"
    }
}

/// Every place the app might be, most specific first.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("AUTOTRIM_APP") {
        out.push(PathBuf::from(p));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    #[cfg(target_os = "macos")]
    {
        let bundle = format!("{APP_NAME}.app");
        out.push(PathBuf::from("/Applications").join(&bundle));
        if let Some(h) = crate::system::home() {
            out.push(h.join("Applications").join(&bundle));
        }
        if let Some(d) = &exe_dir {
            // cargo's layout: target/release/autotrim sits next to
            // target/release/bundle/macos/autoTrim.app once the tray is bundled.
            out.push(d.join("bundle/macos").join(&bundle));
        }
    }
    if let Some(d) = &exe_dir {
        out.push(d.join(tray_binary()));
    }
    out
}

pub fn find() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.exists())
}

/// Where the `autotrim` command-line binary might be, most specific first.
/// The menu bar app uses this to install the login service, since the
/// daemon is the command-line binary, not the app.
pub fn cli_candidates() -> Vec<PathBuf> {
    let cli = if cfg!(windows) {
        "autotrim.exe"
    } else {
        "autotrim"
    };
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("AUTOTRIM_CLI") {
        out.push(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok();
    if let Some(d) = exe.as_ref().and_then(|e| e.parent()) {
        // cargo's layout: the CLI sits next to the tray binary.
        out.push(d.join(cli));
        // An app bundle: install-app.sh puts a copy of the CLI in Resources.
        #[cfg(target_os = "macos")]
        if let Some(contents) = d.parent() {
            out.push(contents.join("Resources").join(cli));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&paths).map(|p| p.join(cli)));
    }
    if let Some(h) = crate::system::home() {
        out.push(h.join(".cargo/bin").join(cli));
    }
    out.push(PathBuf::from("/usr/local/bin").join(cli));
    out.push(PathBuf::from("/opt/homebrew/bin").join(cli));
    out
}

/// The command-line binary, when one can be found.
pub fn find_cli() -> Option<PathBuf> {
    cli_candidates().into_iter().find(|p| p.is_file())
}

/// Launch the app, or bring it to the front if it is already running.
pub fn open() -> Result<()> {
    let Some(path) = find() else {
        let looked: Vec<String> = candidates()
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect();
        bail!(
            "the autoTrim app is not built or installed. Run scripts/install-app.sh, or set AUTOTRIM_APP to its path.\nLooked in:\n{}",
            looked.join("\n")
        );
    };
    #[cfg(target_os = "macos")]
    if path.extension().is_some_and(|e| e == "app") {
        // LaunchServices keeps one instance and brings it forward if it is
        // already running.
        let status = Command::new("open")
            .arg(&path)
            .status()
            .context("running open")?;
        if !status.success() {
            bail!("open {} failed", path.display());
        }
        println!("opened {}", path.display());
        return Ok(());
    }
    // A bare binary: start it detached. The app itself refuses a second
    // instance and focuses the first, so this is safe to repeat.
    Command::new(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", path.display()))?;
    println!("started {}", path.display());
    Ok(())
}
