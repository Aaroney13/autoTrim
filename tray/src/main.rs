//! autoTrim's menu bar companion. It reads what the daemon wrote, shows the
//! summary and the current advice in a tray menu, and opens a window with
//! the full picture and one-click actions. It never samples on its own
//! unless the daemon is not running.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod onboarding;
mod tab_cleanup;
mod updates;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/support/menu-refresh-cases.rs"]
pub mod menu_refresh_tests;

use autotrim::agents::SessionState;
use autotrim::config::Config;
use autotrim::rules::{self, Thresholds};
use autotrim::{Snapshot, actions, app as cli, daemon, fmt, scan_now, service};
use std::sync::Mutex;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
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
    auto_close_tabs: bool,
    auto_tab_inactive_hours: f64,
    auto_tab_domains: Vec<autotrim::tab_rules::DomainRule>,
    tab_rules_revision: String,
    auto_dry_run: bool,
    auto_grace_minutes: u64,
    auto_hosts: Vec<String>,
    stale_after_secs: u64,
    tab_stale_after_secs: u64,
    port_stale_after_secs: u64,
    open_window_at_launch: bool,
    onboarding_completed: bool,
    focus_areas: Vec<String>,
    notify: bool,
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
        auto_close_tabs: cfg.auto_close_tabs,
        auto_tab_inactive_hours: cfg.auto_tab_inactive_hours,
        auto_tab_domains: cfg.auto_tab_domains.clone(),
        tab_rules_revision: daemon::DaemonConfig::from_config(&cfg, &Default::default())
            .auto
            .tab_rules_revision(),
        auto_dry_run: cfg.auto_dry_run,
        auto_grace_minutes: cfg.auto_grace_minutes,
        auto_hosts: cfg.auto_hosts.clone(),
        stale_after_secs: cfg.thresholds().stale_after_secs,
        tab_stale_after_secs: cfg.thresholds().tab_stale_after_secs,
        port_stale_after_secs: cfg.thresholds().port_stale_after_secs,
        open_window_at_launch: cfg.open_window_at_launch,
        onboarding_completed: cfg.onboarding_completed,
        focus_areas: cfg.focus_areas.clone(),
        notify: cfg.notify,
        config_path: path.or_else(Config::path).map(|p| p.display().to_string()),
        config_error: err,
    }
}

/// The menu's one switch. On means "close stale sessions" (servers stay
/// as configured); off turns every target off. Dry run is left alone.
fn toggle_auto(on: bool) -> anyhow::Result<()> {
    let pairs: Vec<(&str, String)> = if on {
        vec![("auto_close_sessions", "true".to_string())]
    } else {
        vec![
            ("auto_close_sessions", "false".to_string()),
            ("auto_stop_servers", "false".to_string()),
            ("auto_close_tabs", "false".to_string()),
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

fn refresh_menu<R: Runtime>(app: &AppHandle<R>, snap: &Snapshot) -> tauri::Result<()> {
    // Refreshes create entries, never a temporary native Menu: dropping even a
    // temporary NSMenu asks AppKit to cancel menu tracking.
    let mut items = Vec::new();
    let sys = &snap.system;
    let mut head = vec![format!("swap {}", fmt::pct(sys.used_swap, sys.total_swap))];
    head.push(format!(
        "RAM {} used",
        fmt::pct(sys.used_mem, sys.total_mem)
    ));
    head.push(format!("cpu {:.0}%", sys.cpu_pct));
    head.push(format!("{} sessions", snap.sessions.len()));
    items.push(MenuItemKind::MenuItem(MenuItem::with_id(
        app,
        "head",
        head.join(" · "),
        false,
        None::<&str>,
    )?));
    items.push(MenuItemKind::Predefined(PredefinedMenuItem::separator(
        app,
    )?));

    if snap.advice.is_empty() {
        items.push(MenuItemKind::MenuItem(MenuItem::with_id(
            app,
            "none",
            "Nothing to do",
            false,
            None::<&str>,
        )?));
    }
    for a in snap.advice.iter().take(5) {
        let glyph = match a.severity {
            autotrim::rules::Severity::High => "●",
            autotrim::rules::Severity::Medium => "◐",
            autotrim::rules::Severity::Low => "○",
        };
        items.push(MenuItemKind::MenuItem(MenuItem::with_id(
            app,
            format!("advice:{}", a.id),
            format!("{glyph} {}", a.title),
            false,
            None::<&str>,
        )?));
    }
    items.push(MenuItemKind::Predefined(PredefinedMenuItem::separator(
        app,
    )?));

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
    items.push(MenuItemKind::MenuItem(MenuItem::with_id(
        app,
        "open",
        "Open autoTrim…",
        true,
        None::<&str>,
    )?));
    items.push(MenuItemKind::MenuItem(MenuItem::with_id(
        app,
        "close_stale",
        label,
        !stale.is_empty(),
        None::<&str>,
    )?));

    // Auto mode: the file's setting as a check mark, and what the daemon
    // is about to do with it underneath.
    let cfg = settings_now();
    let on = cfg.auto_close_sessions || cfg.auto_stop_servers || cfg.auto_close_tabs;
    let auto_label = if on && cfg.auto_dry_run {
        "Auto mode (dry run)"
    } else {
        "Auto mode"
    };
    items.push(MenuItemKind::Check(CheckMenuItem::with_id(
        app,
        "auto",
        auto_label,
        true,
        on,
        None::<&str>,
    )?));
    if let Some(a) = &snap.auto
        && !a.pending.is_empty()
    {
        let soonest = a
            .pending
            .iter()
            .map(|p| p.due_at.saturating_sub(snap.taken_at))
            .min()
            .unwrap_or(0);
        items.push(MenuItemKind::MenuItem(MenuItem::with_id(
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
        )?));
    }
    items.push(MenuItemKind::MenuItem(MenuItem::with_id(
        app,
        "refresh",
        "Refresh",
        true,
        None::<&str>,
    )?));
    items.push(MenuItemKind::Predefined(PredefinedMenuItem::separator(
        app,
    )?));
    items.push(MenuItemKind::MenuItem(MenuItem::with_id(
        app,
        "updates",
        app.state::<updates::Updates>().menu_label(),
        true,
        None::<&str>,
    )?));
    items.push(MenuItemKind::MenuItem(MenuItem::with_id(
        app,
        "quit",
        "Quit autoTrim",
        true,
        None::<&str>,
    )?));
    let menu = app.state::<Menu<R>>();
    update_menu(menu.inner(), &items)
}

/// Keep the attached native menu and its surviving entries alive. Dropping and
/// replacing the menu cancels macOS menu tracking, dismissing it mid-interaction.
/// Call on the main thread so simultaneous refreshes cannot interleave edits.
fn update_menu<R: Runtime>(
    current: &Menu<R>,
    desired_items: &[MenuItemKind<R>],
) -> tauri::Result<()> {
    for item in current.items()? {
        if !matches!(item, MenuItemKind::Predefined(_))
            && !desired_items.iter().any(|new| new.id() == item.id())
        {
            current.remove(&item)?;
        }
    }
    for (index, new) in desired_items.iter().enumerate() {
        let items = current.items()?;
        let existing = items.iter().enumerate().skip(index).find(|(_, old)| {
            old.id() == new.id()
                || matches!(
                    (old, new),
                    (MenuItemKind::Predefined(_), MenuItemKind::Predefined(_))
                )
        });
        if let Some((old_index, old)) = existing {
            if old_index != index {
                current.remove(old)?;
                current.insert(old, index)?;
            }
            match (old, new) {
                (MenuItemKind::MenuItem(old), MenuItemKind::MenuItem(new)) => {
                    if old.text()? != new.text()? {
                        old.set_text(new.text()?)?;
                    }
                    if old.is_enabled()? != new.is_enabled()? {
                        old.set_enabled(new.is_enabled()?)?;
                    }
                }
                (MenuItemKind::Check(old), MenuItemKind::Check(new)) => {
                    if old.text()? != new.text()? {
                        old.set_text(new.text()?)?;
                    }
                    if old.is_checked()? != new.is_checked()? {
                        old.set_checked(new.is_checked()?)?;
                    }
                }
                _ => {}
            }
        } else {
            current.insert(new, index)?;
        }
    }
    while current.items()?.len() > desired_items.len() {
        current.remove_at(desired_items.len())?;
    }
    Ok(())
}

fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    if let Ok((c, _)) = Config::load() {
        *state.thresholds.lock().unwrap() = c.thresholds();
    }
    let snap = current_snapshot(&thresholds(&state));
    let handle = app.clone();
    // Native menu edits are one main-thread operation, including when Refresh
    // or an action overlaps the periodic worker.
    let _ = app.run_on_main_thread(move || {
        let app = &handle;
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            *app.state::<AppState>().latest.lock().unwrap() = Some(snap);
            return;
        };
        let title = format!(
            "{} used",
            fmt::pct(snap.system.used_mem, snap.system.total_mem)
        );
        let _ = tray.set_title(Some(title));
        let auto = snap
            .auto
            .as_ref()
            .filter(|a| a.close_sessions || a.stop_servers || a.close_tabs)
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
        if let Err(error) = refresh_menu(app, &snap) {
            eprintln!("could not refresh the autoTrim menu: {error}");
        }
        *app.state::<AppState>().latest.lock().unwrap() = Some(snap);
    });
}

/// The window is built on first open and destroyed on close, so an idle
/// tray is only the menu item: no web view sitting in memory for nothing.
/// `main` keeps the app alive once the last window is gone.
fn show_window<R: Runtime>(app: &AppHandle<R>) {
    show_window_at(app, false);
}

fn show_window_at<R: Runtime>(app: &AppHandle<R>, settings: bool) {
    if let Some(w) = app.get_webview_window("main") {
        if settings {
            let _ = w.eval("state.view = 'settings'; renderAll(true);");
        }
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let built = tauri::WebviewWindowBuilder::new(
        app,
        "main",
        tauri::WebviewUrl::App(
            if settings {
                "index.html#settings"
            } else {
                "index.html"
            }
            .into(),
        ),
    )
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
    close_tabs: Option<bool>,
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
        if let Some(v) = close_tabs {
            pairs.push(("auto_close_tabs", v.to_string()));
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
    expected_start_time: u64,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    let t = thresholds(&state);
    off_thread(move || actions::close_reviewed_session(pid, expected_start_time, &t)).await
}

#[tauri::command]
async fn stop_server(
    pid: u32,
    force: bool,
    expected_start_time: Option<u64>,
    state: tauri::State<'_, AppState>,
) -> Result<actions::ActionRecord, String> {
    let t = thresholds(&state);
    off_thread(move || {
        actions::stop_reviewed_pid(pid, expected_start_time, &t, false, force, "manual")
    })
    .await
}

#[tauri::command]
async fn close_tabs(
    browser: String,
    expected_tabs: Vec<actions::ReviewedTab>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<actions::ActionRecord>, String> {
    let t = thresholds(&state);
    off_thread(move || actions::close_reviewed_tabs(&browser, &expected_tabs, &t)).await
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
    let open_window = !cfg.onboarding_completed || cfg.open_window_at_launch;
    tauri::Builder::default()
        // First, so a second launch only focuses the window of the first.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_window(app);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::Updates::new())
        .manage(AppState {
            thresholds: Mutex::new(cfg.thresholds()),
            interval_secs: cfg.interval_secs,
            latest: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            source_info,
            settings,
            onboarding::complete_onboarding,
            set_auto,
            tab_cleanup::set_tab_rules,
            tab_cleanup::preview_tab_rules,
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
            action_log,
            updates::update_status,
            updates::check_updates,
            updates::install_update
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let menu = Menu::new(app)?;
            app.manage(menu.clone());
            let _tray: TrayIcon = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_image())
                .icon_as_template(true)
                .tooltip("autoTrim")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open" => show_window(app),
                    "updates" => {
                        show_window_at(app, true);
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = updates::check_updates(handle).await;
                        });
                    }
                    "refresh" => {
                        let app = app.clone();
                        std::thread::spawn(move || refresh(&app));
                    }
                    "close_stale" => close_all_stale(app),
                    "auto" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let now = settings_now();
                            let on = now.auto_close_sessions
                                || now.auto_stop_servers
                                || now.auto_close_tabs;
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
            updates::start(handle.clone());
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
