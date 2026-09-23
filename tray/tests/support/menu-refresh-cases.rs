use super::*;
use tauri::test::MockRuntime;

pub fn run() {
    let scratch = std::env::temp_dir().join(format!("autotrim-menu-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    // This executable has not started any threads or loaded app settings yet.
    unsafe { std::env::set_var("AUTOTRIM_DATA_DIR", &scratch) };
    let app = tauri::test::mock_app();
    app.manage(updates::Updates::new());
    let menu = Menu::new(app.handle()).unwrap();
    app.manage(menu.clone());
    let mut snap: Snapshot = serde_json::from_value(serde_json::json!({
        "taken_at": 50000, "scanner_pid": 1,
        "system": {"os": "fixture", "total_mem": 100, "used_mem": 90,
            "available_mem": 10, "free_pct": 10,
            "total_swap": 0, "used_swap": 0, "uptime_secs": 0},
        "groups": [], "sessions": [], "browsers": [], "advice": []
    }))
    .unwrap();
    refresh_menu(app.handle(), &snap).unwrap();
    let head = menu.get("head").unwrap().as_menuitem_unchecked().clone();
    let close = menu
        .get("close_stale")
        .unwrap()
        .as_menuitem_unchecked()
        .clone();
    let auto = menu
        .get("auto")
        .unwrap()
        .as_check_menuitem_unchecked()
        .clone();
    assert!(head.text().unwrap().contains("RAM 90% used"));
    assert!(!close.is_enabled().unwrap());
    assert!(!auto.is_checked().unwrap());

    snap.system.used_mem = 20;
    snap.system.free_pct = Some(35); // Unrelated OS counter must not affect the label.
    refresh_menu(app.handle(), &snap).unwrap();
    // Replacing/dropping this native menu cancels macOS menu tracking.
    assert_eq!(
        menu.id(),
        app.state::<Menu<MockRuntime>>().id(),
        "refresh must retain the open native menu"
    );
    assert!(
        head.text().unwrap().contains("RAM 20% used"),
        "the displayed entry must receive new data"
    );
    assert_eq!(
        menu.items().unwrap().len(),
        11,
        "refresh must not duplicate entries"
    );

    snap.advice = (0..6)
        .map(|id| rules::Advice {
            id: id.to_string(),
            severity: rules::Severity::High,
            title: format!("Advice {id}"),
            evidence: vec![],
            action: String::new(),
            recovery: None,
        })
        .collect();
    snap.sessions.push(
        serde_json::from_value(serde_json::json!({
        "pid": 100, "start_time": 1, "kind": "claude_code", "host": "terminal", "age_secs": 50000,
            "cpu": 0, "rss": 1024, "procs": 1, "state": "stale", "is_self": false,
            "pids": [100]
        }))
        .unwrap(),
    );
    snap.auto = Some(
        serde_json::from_value(serde_json::json!({
            "close_sessions": true, "stop_servers": false, "dry_run": true,
            "grace_secs": 600, "hosts": ["terminal"], "pending": [{
                "kind": "session", "pid": 100, "target": "fixture", "detail": "idle",
                "rss": 1024, "since": 50000, "due_at": 50600
            }]
        }))
        .unwrap(),
    );
    Config::set_values(&[
        ("auto_close_sessions", "true".into()),
        ("auto_dry_run", "true".into()),
    ])
    .unwrap();
    refresh_menu(app.handle(), &snap).unwrap();
    assert!(menu.get("none").is_none());
    assert!(menu.get("advice:4").is_some());
    assert!(
        menu.get("advice:5").is_none(),
        "only five advice entries are shown"
    );
    assert!(close.is_enabled().unwrap());
    assert_eq!(close.text().unwrap(), "Close 1 stale session");
    let mut engine = snap.sessions[0].clone();
    engine.pid = 200;
    engine.engine = true;
    snap.sessions.push(engine);
    refresh_menu(app.handle(), &snap).unwrap();
    assert_eq!(close.text().unwrap(), "Close 1 stale session");
    assert_eq!(stale_sessions(&snap), vec![100]);
    assert!(auto.is_checked().unwrap());
    assert_eq!(auto.text().unwrap(), "Auto mode (dry run)");
    let pending = menu.get("pending").unwrap().as_menuitem_unchecked().clone();
    assert!(pending.text().unwrap().contains("Would close 1 target"));

    // Advice can change order/count while the same actionable entries remain.
    snap.advice.reverse();
    snap.advice.truncate(2);
    snap.advice[1].title = "Changed advice".into();
    snap.taken_at += 60;
    refresh_menu(app.handle(), &snap).unwrap();
    let advice_ids: Vec<String> = menu
        .items()
        .unwrap()
        .iter()
        .map(|item| item.id().as_ref().to_string())
        .filter(|id| id.starts_with("advice:"))
        .collect();
    assert_eq!(advice_ids, ["advice:5", "advice:4"]);
    assert_eq!(
        menu.get("advice:4")
            .unwrap()
            .as_menuitem_unchecked()
            .text()
            .unwrap(),
        "● Changed advice"
    );
    assert!(pending.text().unwrap().contains("9m"));

    snap.advice.clear();
    snap.sessions.clear();
    snap.auto = None;
    Config::set_values(&[("auto_close_sessions", "false".into())]).unwrap();
    for _ in 0..4 {
        refresh_menu(app.handle(), &snap).unwrap();
        assert_eq!(menu.id(), app.state::<Menu<MockRuntime>>().id());
        assert_eq!(menu.items().unwrap().len(), 11);
    }
    assert!(menu.get("none").is_some());
    assert!(menu.get("pending").is_none());
    assert!(!close.is_enabled().unwrap());
    assert!(!auto.is_checked().unwrap());
    let item_ids: Vec<String> = menu
        .items()
        .unwrap()
        .iter()
        .filter(|item| !matches!(item, MenuItemKind::Predefined(_)))
        .map(|item| item.id().as_ref().to_string())
        .collect();
    assert_eq!(
        item_ids,
        [
            "head",
            "none",
            "open",
            "close_stale",
            "auto",
            "refresh",
            "updates",
            "quit"
        ]
    );

    Config::set_values(&[("auto_close_tabs", "true".into())]).unwrap();
    refresh_menu(app.handle(), &snap).unwrap();
    assert!(
        auto.is_checked().unwrap(),
        "domain-only cleanup is auto mode"
    );
    toggle_auto(false).unwrap();
    let saved = Config::load().unwrap().0;
    assert!(
        !saved.auto_on(),
        "tray Off must disable all cleanup targets"
    );

    drop(app);
    std::fs::remove_dir_all(scratch).unwrap();
    println!("native menu refresh regression passed");
}
