//! Asking another application to do something on the user's behalf: close
//! a browser tab, quit, open it again. On macOS closing and quitting are
//! Apple Events sent through `osascript`, the same thing ⌘W or ⌘Q does, so
//! the app gets to prompt or refuse, and opening again is `open`, so the
//! app belongs to LaunchServices rather than to this process. No other
//! platform has an implementation yet; the verbs say so instead of falling
//! back to a signal, because a signal is not a polite request.
//!
//! The daemon never calls anything here. Only an action does, and only
//! when a person asked for it.

use std::path::Path;

/// True where the verbs below can do anything.
pub const AVAILABLE: bool = cfg!(target_os = "macos");

/// Close one tab of `app`, matched by the browser's own tab id and then by
/// URL, so a tab that navigated since we looked is left alone. `app` must
/// come from a fixed table, never from input.
pub fn close_tab(app: &str, tab_id: i32, url: &str) -> anyhow::Result<String> {
    platform::close_tab(app, tab_id, url)
}

/// Ask `app` to quit the way ⌘Q would.
pub fn quit_app(app: &str) -> anyhow::Result<String> {
    platform::quit_app(app)
}

/// Open `bundle` again, with `args` after `--args`. Only application
/// bundles: `open` hands the launch to LaunchServices, so the app is not a
/// child of this process and outlives it.
pub fn relaunch_app(bundle: &Path, args: &[&str]) -> anyhow::Result<String> {
    platform::relaunch_app(bundle, args)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;
    use std::process::{Command, Stdio};

    fn osascript(script: &str, args: &[String]) -> anyhow::Result<String> {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .args(args)
            .output()
            .map_err(|e| anyhow::anyhow!("could not run osascript: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !out.status.success() {
            let msg = if stderr.contains("-1743") || stderr.contains("Not authorized") {
                "macOS has not allowed autoTrim to control that app (System Settings › Privacy & Security › Automation)".to_string()
            } else if stderr.is_empty() {
                format!("osascript exited with {}", out.status)
            } else {
                stderr
            };
            anyhow::bail!("{msg}");
        }
        Ok(stdout)
    }

    /// Tab ids are compared as text on both sides. Chrome hands them over
    /// as text, and AppleScript's integer stops at 2^29, so an id near two
    /// billion coerced to integer silently becomes a real and never matches.
    pub fn close_tab(app: &str, tab_id: i32, url: &str) -> anyhow::Result<String> {
        let app = app.replace('"', "");
        let script = format!(
            r#"on run argv
  set wantId to (item 1 of argv) as text
  set wantUrl to item 2 of argv
  with timeout of 15 seconds
    tell application "{app}"
      repeat with w in windows
        set ids to id of tabs of w
        repeat with i from 1 to count of ids
          if ((item i of ids) as text) is wantId then
            set t to tab i of w
            if (URL of t) is wantUrl then
              close t
              return "closed"
            else
              return "left alone: the tab moved to " & (URL of t)
            end if
          end if
        end repeat
      end repeat
    end tell
  end timeout
  return "not found: the tab is already gone"
end run"#
        );
        osascript(&script, &[tab_id.to_string(), url.to_string()])
    }

    pub fn quit_app(app: &str) -> anyhow::Result<String> {
        let app = app.replace('"', "");
        let script = format!(
            r#"with timeout of 10 seconds
  tell application "{app}" to quit
end timeout
return "asked to quit""#
        );
        match osascript(&script, &[]) {
            Ok(s) => Ok(s),
            // A save dialog keeps the quit event from returning. The app is
            // deciding, which is the point of asking politely.
            Err(e) if e.to_string().contains("-1712") => {
                Ok("asked to quit; it is waiting on a dialog".to_string())
            }
            Err(e) => Err(e),
        }
    }

    pub fn relaunch_app(bundle: &Path, args: &[&str]) -> anyhow::Result<String> {
        if !bundle.join("Contents/Info.plist").is_file() {
            anyhow::bail!("{} is not an application bundle", bundle.display());
        }
        let mut cmd = Command::new("open");
        cmd.arg(bundle);
        if !args.is_empty() {
            cmd.arg("--args").args(args);
        }
        let out = cmd
            .stdin(Stdio::null())
            .output()
            .map_err(|e| anyhow::anyhow!("could not run open: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if stderr.is_empty() {
                anyhow::bail!("open exited with {}", out.status);
            }
            anyhow::bail!("{stderr}");
        }
        Ok(format!("opened {}", bundle.display()))
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    pub fn close_tab(_app: &str, _tab_id: i32, _url: &str) -> anyhow::Result<String> {
        anyhow::bail!("closing tabs is only implemented on macOS so far")
    }

    pub fn quit_app(_app: &str) -> anyhow::Result<String> {
        anyhow::bail!("asking an app to quit is only implemented on macOS so far")
    }

    pub fn relaunch_app(_bundle: &Path, _args: &[&str]) -> anyhow::Result<String> {
        anyhow::bail!("opening an app again is only implemented on macOS so far")
    }
}
