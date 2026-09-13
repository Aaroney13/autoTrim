//! autoTrim's menu bar companion. It reads what the daemon wrote, shows the
//! summary and the current advice in a tray menu, and opens a window with
//! the full picture and one-click actions. It never samples on its own
//! unless the daemon is not running.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use autotrim::agents::SessionState;
use autotrim::config::Config;
use autotrim::rules::{self, Thresholds};
use autotrim::{Snapshot, actions, app as cli, daemon, fmt, scan_now, service};
use std::sync::Mutex;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Runtime};

const TRAY_ID: &str = "autotrim";
const REFRESH: Duration = Duration::from_secs(5);

struct AppState {
    /// Re-read from the config file on every refresh, so an edit or a
    /// switch in the window reaches the app's own scans too.
    thresholds: Mutex<Thresholds>,
    /// Seconds between the daemon's samples, from the shared config, so the
    /// window can show how long until the next snapshot lands.
    interval_secs: u64,
    latest: Mutex<Option<Snapshot>>,
}

fn thresholds(state: &AppState) -> Thresholds {
    state.thresholds.lock().unwrap().clone()
}

/// Where the numbers come from right now.
#[derive(serde::Serialize, Clone)]
struct Source {
    daemon_running: bool,
    /// When the daemon's latest snapshot was taken, epoch seconds. The window
    /// counts its age from this against its own clock, so the count keeps
    /// moving between polls.
    snapshot_taken_at: Option<u64>,
    /// How often the daemon writes a snapshot.
    interval_secs: u64,
}

fn source(interval_secs: u64) -> Source {
    match daemon::latest() {
        Ok(Some((snap, age))) => Source {
            daemon_running: age <= 120,
            snapshot_taken_at: Some(snap.taken_at),
            interval_secs,
        },
        _ => Source {
            daemon_running: false,
            snapshot_taken_at: None,
            interval_secs,
        },
    }
}

/// The settings the window can change, straight from the file.
#[derive(serde::Serialize, Clone)]
struct Settings {
    auto_close_sessions: bool,
    auto_stop_servers: bool,
    auto_dry_run: bool,
    auto_grace_minutes: u64,
    auto_hosts: Vec<String>,
    open_window_at_launch: bool,
    config_path: Option<String>,
    /// Set when the file exists but could not be read; the values shown
    /// are then the defaults.
    config_error: Option<String>,
}

fn settings_now() -> Settings {
    let (cfg, path, err) = match Config::load() {
        Ok((c, p)) => (c, p, None),
        Err(e) => (Config::default(), None, Some(e.to_string())),
    };
    Settings {
        auto_close_sessions: cfg.auto_close_sessions,
        auto_stop_servers: cfg.auto_stop_servers,
        auto_dry_run: cfg.auto_dry_run,
        auto_grace_minutes: cfg.auto_grace_minutes,
        auto_hosts: cfg.auto_hosts.clone(),
        open_window_at_launch: cfg.open_window_at_launch,
        config_path: path.or_else(Config::path).map(|p| p.display().to_string()),
        config_error: err,
    }
}

/// The menu's one switch. On means "close stale sessions" (servers stay
/// as configured); off turns both verbs off. Dry run is left alone.
fn toggle_auto(on: bool) -> anyhow::Result<()> {
    let pairs: Vec<(&str, String)> = if on {
        vec![("auto_close_sessions", "true".to_string())]
    } else {
        vec![
            ("auto_close_sessions", "false".to_string()),
            ("auto_stop_servers", "false".to_string()),
        ]
    };
    Config::set_values(&pairs)?;
    Ok(())
}

/// The daemon's snapshot while it is alive, otherwise a quick scan.
fn current_snapshot(t: &Thresholds) -> Snapshot {
    match daemon::latest() {
        Ok(Some((snap, age))) if age <= 120 => snap,
        _ => scan_now(t, Duration::from_millis(800)),
    }
}

/// A fresh scan for right after an action, so the window shows what just
/// changed rather than the daemon's last tick, which can be half a minute
/// old. A scan has no history of its own, so the trends and the auto-mode
/// state come from the daemon's snapshot when there is one and the advice
/// is re-evaluated with them.
fn fresh_snapshot(t: &Thresholds) -> Snapshot {
    let mut snap = scan_now(t, Duration::from_millis(300));
    if let Ok(Some((d, age))) = daemon::latest()
        && age <= 120
    {
        snap.trends = d.trends;
        snap.auto = d.auto;
        snap.advice = rules::evaluate(
            &snap.system,
            &snap.groups,
            &snap.sessions,
            &snap.browsers,
            &snap.ports,
            &snap.trends,
            None,
            t,
        );
    }
    snap
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

    // Auto mode: the file's setting as a check mark, and what the daemon
    // is about to do with it underneath.
    let cfg = settings_now();
    let on = cfg.auto_close_sessions || cfg.auto_stop_servers;
    let auto_label = if on && cfg.auto_dry_run {
        "Auto mode (dry run)"
    } else {
        "Auto mode"
    };
    menu.append(&CheckMenuItem::with_id(
        app,
        "auto",
        auto_label,
        true,
        on,
        None::<&str>,
    )?)?;
    if let Some(a) = &snap.auto
        && !a.pending.is_empty()
    {
        let soonest = a
            .pending
            .iter()
            .map(|p| p.due_at.saturating_sub(snap.taken_at))
            .min()
            .unwrap_or(0);
        menu.append(&MenuItem::with_id(
            app,
            "pending",
            format!(
                "{} {} in {}",
                if a.dry_run { "Would close" } else { "Closing" },
                if a.pending.len() == 1 {
                    "1 target".to_string()
                } else {
                    format!("{} targets", a.pending.len())
                },
                fmt::dur(soonest)
            ),
            false,
            None::<&str>,
        )?)?;
    }
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
    if let Ok((c, _)) = Config::load() {
        *state.thresholds.lock().unwrap() = c.thresholds();
    }
    let snap = current_snapshot(&thresholds(&state));
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let title = snap
            .system
            .free_pct
            .map(|f| format!("{f}%"))
            .unwrap_or_else(|| fmt::pct(snap.system.used_swap, snap.system.total_swap));
        let _ = tray.set_title(Some(title));
        let auto = snap
            .auto
            .as_ref()
            .filter(|a| a.close_sessions || a.stop_servers)
            .map(|a| {
                if a.dry_run {
                    " · auto mode (dry run)"
                } else {
                    " · auto mode on"
                }
            })
            .unwrap_or("");
        let _ = tray.set_tooltip(Some(format!(
            "autoTrim · swap {} · {} advice{auto}",
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
/// `main` keeps the app alive once the last window is gone.
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
    let t = thresholds(&state);
    let app = app.clone();
    std::thread::spawn(move || {
        for pid in pids {
            let _ = actions::close_by_pid(pid, &t, false, false, "manual");
        }
        refresh(&app);
    });
}

/// The snapshot the window renders. `fresh` asks for a scan taken now,
/// which the window does after an action; otherwise the daemon's. Off the
/// main thread either way, since a scan takes a few hundred milliseconds.
#[tauri::command]
async fn snapshot(
    fresh: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<Snapshot, String> {
    let t = thresholds(&state);
    let snap = off_thread(move || {
        Ok(if fresh.unwrap_or(false) {
            fresh_snapshot(&t)
        } else {
            current_snapshot(&t)
        })
    })
    .await?;
    let json = serde_json::to_value(&snap).map_err(|e| e.to_string())?;
    *state.latest.lock().unwrap() = Some(snap);
    serde_json::from_value(json).map_err(|e| e.to_string())
}

#[tauri::command]
fn source_info(state: tauri::State<'_, AppState>) -> Source {
    source(state.interval_secs)
}

#[tauri::command]
fn settings() -> Settings {
    settings_now()
}

/// Change any of the auto-mode switches. Each is optional so the window can
/// send just the one that was clicked. The daemon re-reads the file on its
/// next tick.
#[tauri::command(rename_all = "snake_case")]
async fn set_auto(
    close_sessions: Option<bool>,
    stop_servers: Option<bool>,
    dry_run: Option<bool>,
) -> Result<Settings, String> {
    off_thread(move || {
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if let Some(v) = close_sessions {
            pairs.push(("auto_close_sessions", v.to_string()));
        }
        if let Some(v) = stop_servers {
            pairs.push(("auto_stop_servers", v.to_string()));
        }
        if let Some(v) = dry_run {
            pairs.push(("auto_dry_run", v.to_string()));
        }
        if !pairs.is_empty() {
            Config::set_values(&pairs)?;
        }
        Ok(settings_now())
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn set_open_window_at_launch(value: bool) -> Result<Settings, String> {
    off_thread(move || {
        Config::set_values(&[("open_window_at_launch", value.to_string())])?;
        Ok(settings_now())
    })
    .await
}

#[tauri::command]
fn service_info() -> service::ServiceInfo {
    service::info()
}

/// Install the daemon as a login service, running the command-line binary
/// from wherever it can be found: next to this app, inside its bundle, or
/// on PATH.
#[tauri::command]
async fn service_install() -> Result<service::ServiceInfo, String> {
    off_thread(|| {
        let Some(exe) = cli::find_cli() else {
            anyhow::bail!(
                "the autotrim command was not found next to this app, inside its bundle, or on PATH; build it with `cargo build --release`, or set AUTOTRIM_CLI to its path"
            );
        };
        service::install(Some(&exe))
    })
    .await
}

#[tauri::command]
async fn service_uninstall() -> Result<service::ServiceInfo, String> {
    off_thread(|| {
        service::uninstall()?;
        Ok(service::info())
    })
    .await
}

#[tauri::command]
async fn service_restart() -> Result<service::ServiceInfo, String> {
    off_thread(|| {
        service::restart()?;
        Ok(service::info())
    })
    .await
}

/// Close the window. The menu bar item and the daemon carry on.
#[tauri::command]
fn hide_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.close();
    }
}

/// Run an action off the main thread: each one takes a fresh snapshot and
/// may wait on another process, and the window should stay responsive.
async fn off_thread<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn close_session(
    pid: u32,
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    let t = thresholds(&state);
    off_thread(move || actions::close_by_pid(pid, &t, false, force, "manual")).await
}

#[tauri::command]
async fn stop_server(
    pid: u32,
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    let t = thresholds(&state);
    off_thread(move || actions::stop_by_pid(pid, &t, false, force, "manual")).await
}

#[tauri::command]
async fn close_tabs(
    browser: String,
    ids: Vec<i32>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<actions::ActionRecord>, String> {
    let t = thresholds(&state);
    off_thread(move || actions::close_tabs_by_id(&browser, &ids, &t, false, "manual")).await
}

#[tauri::command]
async fn quit_app(
    name: String,
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    let t = thresholds(&state);
    off_thread(move || actions::quit_app_by_name(&name, &t, false, force, "manual")).await
}

#[tauri::command]
async fn restart_app(
    name: String,
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    let t = thresholds(&state);
    off_thread(move || actions::restart_app_by_name(&name, &t, false, force, "manual")).await
}

#[tauri::command]
fn action_log() -> Result<Vec<actions::ActionRecord>, String> {
    actions::read_log(30).map_err(|e| e.to_string())
}

fn main() {
    let cfg = Config::load().map(|(c, _)| c).unwrap_or_default();
    let open_window = cfg.open_window_at_launch;
    tauri::Builder::default()
        // First, so a second launch only focuses the window of the first.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_window(app);
        }))
        .manage(AppState {
            thresholds: Mutex::new(cfg.thresholds()),
            interval_secs: cfg.interval_secs,
            latest: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            source_info,
            settings,
            set_auto,
            set_open_window_at_launch,
            service_info,
            service_install,
            service_uninstall,
            service_restart,
            hide_window,
            close_session,
            stop_server,
            close_tabs,
            quit_app,
            restart_app,
            action_log
        ])
        .setup(move |app| {
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
                    "auto" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let now = settings_now();
                            let on = now.auto_close_sessions || now.auto_stop_servers;
                            if let Err(e) = toggle_auto(!on) {
                                eprintln!("could not change auto mode: {e}");
                            }
                            refresh(&app);
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            refresh(&handle);
            // Show the window on launch so the app is not just a small icon
            // among many, unless the person asked for the menu bar item
            // only. Closing it leaves the tray running either way.
            if open_window {
                show_window(&handle);
            }
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(REFRESH);
                    refresh(&handle);
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("autoTrim tray failed to start")
        .run(|_app, event| {
            // Tauri exits when its last window is destroyed, which would take
            // the menu bar item with the window. That request carries no exit
            // code; "Quit autoTrim" calls `exit(0)`, which does, and goes
            // through.
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit();
            }
        });
}
