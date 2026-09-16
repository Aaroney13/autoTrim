//! Signed macOS updates. The webview can request a check or install the
//! cached offer, but cannot supply a URL, key, signature, or version.
use std::sync::Mutex;
use std::time::Duration;

use autotrim::{app as cli, service};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Serialize)]
pub struct Status {
    current_version: String,
    enabled: bool,
    phase: &'static str,
    version: Option<String>,
    notes: Option<String>,
    error: Option<String>,
    service_error: Option<String>,
}

pub struct Updates {
    status: Mutex<Status>,
    offer: Mutex<Option<Update>>,
    operation: tokio::sync::Mutex<()>,
}

impl Updates {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(Status {
                current_version: env!("CARGO_PKG_VERSION").into(),
                enabled: cfg!(target_os = "macos") && cli::bundled_cli().is_some(),
                phase: "idle",
                version: None,
                notes: None,
                error: None,
                service_error: None,
            }),
            offer: Mutex::new(None),
            operation: tokio::sync::Mutex::new(()),
        }
    }

    pub fn menu_label(&self) -> String {
        let s = self.status.lock().unwrap();
        match &s.version {
            Some(v) => format!("Update available: {v}…"),
            None => "Check for updates…".into(),
        }
    }
}

#[tauri::command]
pub fn update_status(state: tauri::State<'_, Updates>) -> Status {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
pub async fn check_updates(app: AppHandle) -> Result<Status, String> {
    let state = app.state::<Updates>();
    let _guard = state
        .operation
        .try_lock()
        .map_err(|_| "An update operation is already in progress")?;
    {
        let mut s = state.status.lock().unwrap();
        if !s.enabled {
            return Err("Updates are available in the installed macOS app".into());
        }
        s.phase = "checking";
        s.error = None;
    }
    let result = async {
        app.updater_builder()
            .timeout(Duration::from_secs(5 * 60))
            .build()?
            .check()
            .await
    }
    .await;
    let mut s = state.status.lock().unwrap();
    match result {
        Ok(offer) => {
            s.version = offer.as_ref().map(|u| u.version.clone());
            s.notes = offer.as_ref().and_then(|u| u.body.clone());
            s.phase = if offer.is_some() {
                "available"
            } else {
                "current"
            };
            *state.offer.lock().unwrap() = offer;
        }
        Err(e) => {
            // Retain an earlier offer across transient network failures.
            s.phase = if s.version.is_some() {
                "available"
            } else {
                "error"
            };
            s.error = Some(format!("Could not check for updates: {e}"));
        }
    }
    Ok(s.clone())
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let state = app.state::<Updates>();
    let _guard = state
        .operation
        .try_lock()
        .map_err(|_| "An update operation is already in progress")?;
    let offer = state
        .offer
        .lock()
        .unwrap()
        .clone()
        .ok_or("Check for an update first")?;
    {
        let mut s = state.status.lock().unwrap();
        s.phase = "installing";
        s.error = None;
    }
    // Tauri verifies the downloaded archive against the embedded public key
    // before replacing the bundle. Leave the existing daemon running until
    // the new app launches successfully and rebinds the service below.
    if let Err(e) = offer.download_and_install(|_, _| {}, || {}).await {
        let message = format!("Could not install the update: {e}");
        let mut s = state.status.lock().unwrap();
        s.phase = "available";
        s.error = Some(message.clone());
        return Err(message);
    }
    app.restart();
}

/// A source-installed daemon may still point outside the app. Rebind an
/// existing login service on first launch and after every app update. Never
/// enable a service the user has turned off. The marker is written only
/// after launchd successfully loads the bundled daemon, so failure retries.
fn sync_daemon() -> anyhow::Result<()> {
    let Some(exe) = cli::bundled_cli() else {
        return Ok(());
    };
    let info = service::info();
    if !info.installed {
        return Ok(());
    }
    let Some(dir) = autotrim::paths::data_dir() else {
        return Ok(());
    };
    let marker = dir.join("app-daemon-version");
    let version = env!("CARGO_PKG_VERSION");
    let previous = std::fs::read_to_string(&marker).ok();
    if needs_daemon_sync(
        previous.as_deref(),
        version,
        info.binary.as_deref(),
        &exe.to_string_lossy(),
    ) {
        service::install(Some(&exe))?;
        std::fs::write(marker, version)?;
    }
    Ok(())
}

fn needs_daemon_sync(
    previous: Option<&str>,
    version: &str,
    binary: Option<&str>,
    bundled: &str,
) -> bool {
    previous != Some(version) || binary != Some(bundled)
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(e) = sync_daemon() {
            app.state::<Updates>().status.lock().unwrap().service_error = Some(format!(
                "Could not update the background monitor: {e}. Use Run in background to retry."
            ));
        }
        tauri::async_runtime::spawn(async move {
            loop {
                let _ = check_updates(app.clone()).await;
                tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_migrates_then_restarts_only_when_the_app_version_or_path_changes() {
        let bundled = "/Applications/autoTrim.app/Contents/Resources/autotrim";
        assert!(needs_daemon_sync(None, "0.2.0", Some(bundled), bundled));
        assert!(needs_daemon_sync(
            Some("0.1.0"),
            "0.2.0",
            Some(bundled),
            bundled
        ));
        assert!(needs_daemon_sync(
            Some("0.2.0"),
            "0.2.0",
            Some("/usr/local/bin/autotrim"),
            bundled
        ));
        assert!(!needs_daemon_sync(
            Some("0.2.0"),
            "0.2.0",
            Some(bundled),
            bundled
        ));
    }
}
