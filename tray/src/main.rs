//! autoTrim's menu bar companion. It reads what the daemon wrote, shows the
//! summary and the current advice in a tray menu, and opens a window with
//! the full picture and one-click actions. It never samples on its own
//! unless the daemon is not running.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use autotrim::agents::SessionState;
use autotrim::config::Config;
use autotrim::rules::Thresholds;
use autotrim::{Snapshot, actions, daemon, fmt, scan_now};
use std::sync::Mutex;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Runtime};

const TRAY_ID: &str = "autotrim";
const REFRESH: Duration = Duration::from_secs(5);

struct AppState {
    thresholds: Thresholds,
    latest: Mutex<Option<Snapshot>>,
}

/// Where the numbers come from right now.
#[derive(serde::Serialize, Clone)]
struct Source {
    daemon_running: bool,
    snapshot_age_secs: Option<u64>,
}

fn source() -> Source {
    match daemon::latest() {
        Ok(Some((_, age))) => Source {
            daemon_running: age <= 120,
            snapshot_age_secs: Some(age),
        },
        _ => Source {
            daemon_running: false,
            snapshot_age_secs: None,
        },
    }
}

/// The daemon's snapshot while it is alive, otherwise a quick scan.
fn current_snapshot(t: &Thresholds) -> Snapshot {
    match daemon::latest() {
        Ok(Some((snap, age))) if age <= 120 => snap,
        _ => scan_now(t, Duration::from_millis(800)),
    }
}

/// A 22-pixel ring, black on transparent, so macOS can treat it as a
/// template image and recolour it for light and dark menu bars.
fn tray_image() -> Image<'static> {
    const N: u32 = 22;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let dx = x as f32 - 10.5;
            let dy = y as f32 - 10.5;
            let r = (dx * dx + dy * dy).sqrt();
            let a = if (5.2..=8.6).contains(&r) { 255 } else { 0 };
            rgba.extend_from_slice(&[0, 0, 0, a]);
        }
    }
    Image::new_owned(rgba, N, N)
}

fn stale_sessions(snap: &Snapshot) -> Vec<u32> {
    snap.sessions
        .iter()
        .filter(|s| s.state == SessionState::Stale && !s.is_self)
        .map(|s| s.pid)
        .collect()
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, snap: &Snapshot) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    let sys = &snap.system;
    let mut head = vec![format!("swap {}", fmt::pct(sys.used_swap, sys.total_swap))];
    if let Some(f) = sys.free_pct {
        head.push(format!("free {f}%"));
    }
    head.push(format!("cpu {:.0}%", sys.cpu_pct));
    head.push(format!("{} sessions", snap.sessions.len()));
    menu.append(&MenuItem::with_id(
        app,
        "head",
        head.join(" · "),
        false,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    if snap.advice.is_empty() {
        menu.append(&MenuItem::with_id(
            app,
            "none",
            "Nothing to do",
            false,
            None::<&str>,
        )?)?;
    }
    for a in snap.advice.iter().take(5) {
        let glyph = match a.severity {
            autotrim::rules::Severity::High => "●",
            autotrim::rules::Severity::Medium => "◐",
            autotrim::rules::Severity::Low => "○",
        };
        menu.append(&MenuItem::with_id(
            app,
            format!("advice:{}", a.id),
            format!("{glyph} {}", a.title),
            false,
            None::<&str>,
        )?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    let stale = stale_sessions(snap);
    let label = if stale.is_empty() {
        "No stale sessions".to_string()
    } else {
        format!(
            "Close {} stale session{}",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        )
    };
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open autoTrim…",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "close_stale",
        label,
        !stale.is_empty(),
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "refresh",
        "Refresh",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        "Quit autoTrim",
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}

fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let snap = current_snapshot(&state.thresholds);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let title = snap
            .system
            .free_pct
            .map(|f| format!("{f}%"))
            .unwrap_or_else(|| fmt::pct(snap.system.used_swap, snap.system.total_swap));
        let _ = tray.set_title(Some(title));
        let _ = tray.set_tooltip(Some(format!(
            "autoTrim · swap {} · {} advice",
            fmt::pct(snap.system.used_swap, snap.system.total_swap),
            snap.advice.len()
        )));
        if let Ok(menu) = build_menu(app, &snap) {
            let _ = tray.set_menu(Some(menu));
        }
    }
    *state.latest.lock().unwrap() = Some(snap);
}

/// The window is built on first open and destroyed on close, so an idle
/// tray is only the menu item: no web view sitting in memory for nothing.
fn show_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let built =
        tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
            .title("autoTrim")
            .inner_size(1000.0, 740.0)
            .min_inner_size(760.0, 500.0)
            .build();
    match built {
        Ok(w) => {
            let _ = w.set_focus();
        }
        Err(e) => eprintln!("could not open the autoTrim window: {e}"),
    }
}

fn close_all_stale<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let pids: Vec<u32> = state
        .latest
        .lock()
        .unwrap()
        .as_ref()
        .map(stale_sessions)
        .unwrap_or_default();
    let t = state.thresholds.clone();
    let app = app.clone();
    std::thread::spawn(move || {
        for pid in pids {
            let _ = actions::close_by_pid(pid, &t, false, false, "manual");
        }
        refresh(&app);
    });
}

#[tauri::command]
fn snapshot(state: tauri::State<'_, AppState>) -> Result<Snapshot, String> {
    let snap = current_snapshot(&state.thresholds);
    let json = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
    *state.latest.lock().unwrap() = Some(snap);
    serde_json::from_value(json).map_err(|e| e.to_string())
}

#[tauri::command]
fn source_info() -> Source {
    source()
}

#[tauri::command]
fn close_session(
    pid: u32,
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    actions::close_by_pid(pid, &state.thresholds, false, force, "manual").map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_server(
    pid: u32,
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    actions::stop_by_pid(pid, &state.thresholds, false, force, "manual").map_err(|e| e.to_string())
}

#[tauri::command]
fn action_log() -> Result<Vec<actions::ActionRecord>, String> {
    actions::read_log(30).map_err(|e| e.to_string())
}

fn main() {
    let cfg = Config::load().map(|(c, _)| c).unwrap_or_default();
    tauri::Builder::default()
        .manage(AppState {
            thresholds: cfg.thresholds(),
            latest: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            source_info,
            close_session,
            stop_server,
            action_log
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let _tray: TrayIcon = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_image())
                .icon_as_template(true)
                .tooltip("autoTrim")
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open" => show_window(app),
                    "refresh" => {
                        let app = app.clone();
                        std::thread::spawn(move || refresh(&app));
                    }
                    "close_stale" => close_all_stale(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            refresh(&handle);
            // Show the window on launch so the app is not just a small icon
            // among many. Closing it leaves the tray running.
            show_window(&handle);
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(REFRESH);
                    refresh(&handle);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("autoTrim tray failed to start");
}
