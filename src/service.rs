//! Install the daemon as a login service. macOS via launchd today; other
//! platforms report that they are not wired up yet. The functions return
//! what happened so the window can show it; `run` is the command line's
//! printing face.

use anyhow::Result;
use serde::Serialize;

pub const LABEL: &str = "com.autotrim.daemon";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
    Restart,
    Status,
}

/// The login service as it stands.
#[derive(Serialize, Clone, Debug, Default)]
pub struct ServiceInfo {
    /// Whether this platform has a login service at all.
    pub supported: bool,
    pub installed: bool,
    pub running: bool,
    pub pid: Option<u32>,
    /// The daemon binary the service runs.
    pub binary: Option<String>,
    pub plist: Option<String>,
}

#[cfg(target_os = "macos")]
pub use macos::{info, install, restart, uninstall};

#[cfg(not(target_os = "macos"))]
pub use other::{info, install, restart, uninstall};

/// Do it and say what happened.
pub fn run(action: Action) -> Result<()> {
    match action {
        Action::Install => {
            let i = install(None)?;
            println!("installed {LABEL}");
            println!("  binary  {}", i.binary.unwrap_or_default());
            println!("  plist   {}", i.plist.unwrap_or_default());
            if let Some(d) = crate::paths::data_dir() {
                println!("  log     {}", d.join("daemon.log").display());
            }
            println!("It starts now and at every login. `autotrim service uninstall` removes it.");
        }
        Action::Uninstall => {
            uninstall()?;
            println!("removed {LABEL}; data directory left in place");
        }
        Action::Restart => {
            restart()?;
            println!("restarted {LABEL}");
        }
        Action::Status => {
            let i = info();
            if !i.supported {
                println!("{LABEL}: no login service on this platform yet");
            } else if !i.installed {
                println!("{LABEL}: not installed");
            } else {
                println!("{LABEL}: installed");
                println!(
                    "  state = {}",
                    if i.running { "running" } else { "not running" }
                );
                if let Some(p) = i.pid {
                    println!("  pid = {p}");
                }
                if let Some(b) = i.binary {
                    println!("  binary = {b}");
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
mod other {
    use super::ServiceInfo;
    use anyhow::{Result, bail};
    use std::path::Path;

    const MSG: &str = "service install is only implemented for macOS so far; run `autotrim daemon` under your own supervisor";

    pub fn info() -> ServiceInfo {
        ServiceInfo::default()
    }

    pub fn install(_exe: Option<&Path>) -> Result<ServiceInfo> {
        bail!(MSG)
    }

    pub fn uninstall() -> Result<()> {
        bail!(MSG)
    }

    pub fn restart() -> Result<()> {
        bail!(MSG)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{LABEL, ServiceInfo};
    use crate::paths;
    use crate::system::home;
    use anyhow::{Context, Result, bail};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn plist_path() -> Result<PathBuf> {
        Ok(home()
            .context("no home directory")?
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    fn domain() -> String {
        format!("gui/{}", unsafe { libc::getuid() })
    }

    fn target() -> String {
        format!("{}/{LABEL}", domain())
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    fn xml_unescape(s: &str) -> String {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }

    fn plist(exe: &str, log: &str, data_dir: Option<&str>) -> String {
        let env = match data_dir {
            Some(d) => format!(
                "\n  <key>EnvironmentVariables</key>\n  <dict>\n    <key>AUTOTRIM_DATA_DIR</key>\n    <string>{}</string>\n  </dict>",
                xml_escape(d)
            ),
            None => String::new(),
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>60</integer>
  <key>Umask</key>
  <integer>63</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>{env}
</dict>
</plist>
"#,
            exe = xml_escape(exe),
            log = xml_escape(log),
        )
    }

    /// The first ProgramArguments entry of a plist this module wrote.
    fn program_of(plist: &str) -> Option<String> {
        let rest = plist.split("<key>ProgramArguments</key>").nth(1)?;
        let s = rest.split("<string>").nth(1)?;
        Some(xml_unescape(s.split("</string>").next()?))
    }

    fn launchctl(args: &[&str]) -> Result<std::process::Output> {
        Command::new("launchctl")
            .args(args)
            .output()
            .context("running launchctl")
    }

    fn is_loaded() -> bool {
        launchctl(&["print", &target()])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn info() -> ServiceInfo {
        let plist_file = plist_path().ok();
        let on_disk = plist_file.as_ref().is_some_and(|p| p.exists());
        let binary = plist_file
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| program_of(&t));
        let out = launchctl(&["print", &target()]).ok();
        let loaded = out.as_ref().is_some_and(|o| o.status.success());
        let text = out
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let pick = |key: &str| {
            text.lines()
                .map(str::trim)
                .find(|l| l.starts_with(key))
                .map(|l| l[key.len()..].trim().to_string())
        };
        let pid = pick("pid = ").and_then(|p| p.parse().ok());
        let running =
            loaded && (pid.is_some() || pick("state = ").is_some_and(|s| s.starts_with("running")));
        ServiceInfo {
            supported: true,
            installed: on_disk || loaded,
            running,
            pid,
            binary,
            plist: plist_file.map(|p| p.display().to_string()),
        }
    }

    /// Write the launch agent for `exe` (this binary when None) and start
    /// it now and at every login.
    pub fn install(exe: Option<&Path>) -> Result<ServiceInfo> {
        let exe = match exe {
            Some(p) => p
                .canonicalize()
                .with_context(|| format!("locating {}", p.display()))?,
            None => std::env::current_exe()?
                .canonicalize()
                .context("locating this binary")?,
        };
        let dir = paths::data_dir().context("no data directory")?;
        crate::storage::harden_existing(&dir)?;
        let log = dir.join("daemon.log");
        crate::storage::append_log(&log, b"")?;
        let plist_file = plist_path()?;
        fs::create_dir_all(plist_file.parent().unwrap())?;
        let data_override = std::env::var("AUTOTRIM_DATA_DIR").ok();
        fs::write(
            &plist_file,
            plist(
                &exe.to_string_lossy(),
                &log.to_string_lossy(),
                data_override.as_deref(),
            ),
        )
        .with_context(|| format!("writing {}", plist_file.display()))?;
        if is_loaded() {
            let _ = launchctl(&["bootout", &target()]);
        }
        let out = launchctl(&["bootstrap", &domain(), &plist_file.to_string_lossy()])?;
        if !out.status.success() {
            bail!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(info())
    }

    /// Stop the daemon and remove the launch agent. Data is kept.
    pub fn uninstall() -> Result<()> {
        let plist_file = plist_path()?;
        if is_loaded() {
            let out = launchctl(&["bootout", &target()])?;
            if !out.status.success() {
                bail!(
                    "launchctl bootout failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
        }
        if plist_file.exists() {
            fs::remove_file(&plist_file)?;
        }
        Ok(())
    }

    /// Restart the running daemon, for example after rebuilding.
    pub fn restart() -> Result<()> {
        if !is_loaded() {
            bail!("{LABEL} is not installed; run `autotrim service install`");
        }
        let out = launchctl(&["kickstart", "-k", &target()])?;
        if !out.status.success() {
            bail!(
                "launchctl kickstart failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn program_is_read_back_from_the_plist() {
            let text = plist("/Applications/a&b/autotrim", "/tmp/d.log", None);
            assert_eq!(
                program_of(&text).as_deref(),
                Some("/Applications/a&b/autotrim")
            );
            assert_eq!(program_of("<plist/>"), None);
            assert!(text.contains("<key>ThrottleInterval</key>\n  <integer>60</integer>"));
            assert!(text.contains("<key>Umask</key>\n  <integer>63</integer>"));
        }
    }
}
