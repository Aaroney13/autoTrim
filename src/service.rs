//! Install the daemon as a login service. macOS via launchd today; other
//! platforms report that they are not wired up yet.

use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
    Restart,
    Status,
}

#[cfg(target_os = "macos")]
pub fn run(action: Action) -> Result<()> {
    macos::run(action)
}

#[cfg(not(target_os = "macos"))]
pub fn run(_action: Action) -> Result<()> {
    anyhow::bail!(
        "service install is only implemented for macOS so far; run `autotrim daemon` under your own supervisor"
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Action;
    use crate::paths;
    use crate::system::home;
    use anyhow::{Context, Result, bail};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    const LABEL: &str = "com.autotrim.daemon";

    fn plist_path() -> Result<PathBuf> {
        Ok(home()
            .context("no home directory")?
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    fn domain() -> String {
        format!("gui/{}", unsafe { libc::getuid() })
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
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

    fn launchctl(args: &[&str]) -> Result<std::process::Output> {
        Command::new("launchctl")
            .args(args)
            .output()
            .context("running launchctl")
    }

    fn is_loaded() -> bool {
        launchctl(&["print", &format!("{}/{LABEL}", domain())])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn install() -> Result<()> {
        let exe = std::env::current_exe()?
            .canonicalize()
            .context("locating this binary")?;
        let dir = paths::data_dir().context("no data directory")?;
        fs::create_dir_all(&dir)?;
        let log = dir.join("daemon.log");
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
            let _ = launchctl(&["bootout", &format!("{}/{LABEL}", domain())]);
        }
        let out = launchctl(&["bootstrap", &domain(), &plist_file.to_string_lossy()])?;
        if !out.status.success() {
            bail!(
                "launchctl bootstrap failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        println!("installed {LABEL}");
        println!("  binary  {}", exe.display());
        println!("  plist   {}", plist_file.display());
        println!("  log     {}", log.display());
        println!("It starts now and at every login. `autotrim service uninstall` removes it.");
        Ok(())
    }

    fn uninstall() -> Result<()> {
        let plist_file = plist_path()?;
        if is_loaded() {
            let out = launchctl(&["bootout", &format!("{}/{LABEL}", domain())])?;
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
        println!("removed {LABEL}; data directory left in place");
        Ok(())
    }

    fn status() -> Result<()> {
        let out = launchctl(&["print", &format!("{}/{LABEL}", domain())])?;
        if !out.status.success() {
            println!("{LABEL}: not installed");
            return Ok(());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let pick = |key: &str| {
            text.lines()
                .find(|l| l.trim_start().starts_with(key))
                .map(|l| l.trim().to_string())
        };
        println!("{LABEL}: installed");
        for k in ["state = ", "pid = ", "last exit code = "] {
            if let Some(l) = pick(k) {
                println!("  {l}");
            }
        }
        Ok(())
    }

    pub fn run(action: Action) -> Result<()> {
        match action {
            Action::Install => install(),
            Action::Uninstall => uninstall(),
            Action::Restart => {
                if !is_loaded() {
                    bail!("{LABEL} is not installed; run `autotrim service install`");
                }
                let target = format!("{}/{LABEL}", domain());
                let out = launchctl(&["kickstart", "-k", &target])?;
                if !out.status.success() {
                    bail!(
                        "launchctl kickstart failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                }
                println!("restarted {LABEL}");
                Ok(())
            }
            Action::Status => status(),
        }
    }
}
