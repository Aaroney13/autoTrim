//! Synthetic fixtures: no host scans or process actions.
use crate::{Snapshot, agents::AgentSession, ports::PortInfo};
use serde_json::json;
pub fn session() -> AgentSession {
    serde_json::from_value(json!({
        "pid": 100, "kind": "claude_code", "host": "terminal", "cwd": "/work/old",
        "project": "old", "age_secs": 50000, "start_time": 10, "cpu": 0.0,
        "rss": 1024, "procs": 1, "state": "stale", "is_self": false,
        "cpu_window_mean": 0.0, "quiet_for_secs": 10000, "last_activity": 100,
        "idle_secs": 40000, "transcript": "/synthetic/transcript", "pids": [100]
    }))
    .unwrap()
}
pub fn port() -> PortInfo {
    serde_json::from_value(json!({"pid": 200, "port": 3000, "protocol": "TCP",
        "addr": "127.0.0.1", "process": "node", "owner": "node", "dev_runtime": true,
        "owner_age_secs": 100000, "open_for_secs": 100000, "start_time": 10
    }))
    .unwrap()
}
pub fn snapshot() -> Snapshot {
    serde_json::from_value(json!({"taken_at": 50000, "scanner_pid": 1,
        "system": {"os": "fixture", "total_mem": 0, "used_mem": 0,
        "available_mem": 0, "total_swap": 0, "used_swap": 0, "uptime_secs": 0},
        "groups": [], "sessions": [], "browsers": [], "advice": []}))
    .unwrap()
}

pub fn chrome_snapshot() -> Snapshot {
    serde_json::from_value(serde_json::json!({
        "taken_at": 100, "scanner_pid": 1,
        "system": {
            "os": "test", "total_mem": 0, "used_mem": 0, "available_mem": 0,
            "total_swap": 0, "used_swap": 0, "uptime_secs": 0
        },
        "groups": [], "sessions": [], "advice": [],
        "browsers": [{
            "name": "Google Chrome", "rss": 0, "procs": 1, "renderers": 0,
            "extension_renderers": 0, "gpu": 0, "utility": 0,
            "can_close_tabs": true,
            "tabs": [{
                "id": 7, "window_id": 1, "profile": "Default", "index": 1,
                "url": "chrome://newtab/", "site": "chrome://newtab", "title": "New Tab",
                "pinned": false, "active": false, "last_active": null, "idle_secs": null
            }]
        }]
    }))
    .unwrap()
}
