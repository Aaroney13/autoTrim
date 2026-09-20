//! Native notifications. Thin on purpose: the daemon decides what and when,
//! this only delivers.

use anyhow::Result;

pub fn send(title: &str, subtitle: &str, body: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    initialize_macos()?;
    let mut n = notify_rust::Notification::new();
    n.appname("autoTrim").summary(title).body(body);
    #[cfg(target_os = "macos")]
    n.subtitle(subtitle);
    #[cfg(not(target_os = "macos"))]
    let _ = subtitle;
    n.show()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn initialize_macos() -> Result<()> {
    use std::sync::OnceLock;
    static INITIALIZED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    let result = INITIALIZED.get_or_init(|| {
        // appname() does not set macOS's notification identity. Leaving it
        // unset makes mac-notification-sys resolve an application literally
        // named "use_default" through AppleScript, which opens a chooser.
        let bundled = std::env::current_exe()
            .ok()
            .and_then(|exe| crate::groups::bundle_path(&exe))
            .is_some_and(|bundle| bundle.file_name().is_some_and(|n| n == "autoTrim.app"));
        let installed = std::path::Path::new("/Applications/autoTrim.app/Contents/Info.plist")
            .is_file()
            || crate::system::home().is_some_and(|home| {
                home.join("Applications/autoTrim.app/Contents/Info.plist")
                    .is_file()
            });
        let identifier = if bundled || installed {
            "com.autotrim.tray"
        } else {
            // CLI-only installations still have a registered system sender.
            "com.apple.Terminal"
        };
        #[allow(deprecated)] // Explicit setup for notify-rust's current macOS backend.
        notify_rust::set_application(identifier).map_err(|error| error.to_string())
    });
    result
        .as_ref()
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("could not initialize macOS notifications: {error}"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    #[ignore = "sends two real macOS notifications; run explicitly for delivery checks"]
    fn notification_delivery_after_explicit_initialization() {
        for _ in 0..2 {
            super::send(
                "autoTrim notification check",
                "Application identity fixed",
                "This notification should arrive without an application chooser.",
            )
            .unwrap();
        }
    }
}
