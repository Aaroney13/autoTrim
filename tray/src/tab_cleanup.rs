//! Domain rule editing and a read-only preview. Never calls action automation.
use super::{Settings, off_thread, settings_now};
use autotrim::tab_rules::{
    DomainRule, TabAssessment, assess_tab, domain_of_url, inactivity_secs, matches_url,
    serialize_rules, validate_domain_rules,
};
use autotrim::{Snapshot, config::Config, scan_now};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub(crate) struct TabRulePreview {
    browser: String,
    profile: String,
    window_id: i32,
    id: i32,
    title: String,
    domain: String,
    idle_secs: Option<u64>,
    status: &'static str,
    reason: String,
}

fn preview_rows(
    snap: &Snapshot,
    rules: &[DomainRule],
    inactive_hours: f64,
) -> anyhow::Result<Vec<TabRulePreview>> {
    let mut rows = Vec::new();
    for browser in &snap.browsers {
        // The feature concerns Chrome only; don't list other browser sessions.
        if browser.name != "Google Chrome" {
            continue;
        }
        if let Some(note) = &browser.tabs_note {
            anyhow::bail!("Could not review all Chrome tabs: {note}");
        }
        for tab in &browser.tabs {
            if !rules.iter().any(|rule| matches_url(rule, &tab.url)) {
                continue;
            }
            let (status, reason) = match assess_tab(
                browser,
                tab,
                rules,
                inactivity_secs(inactive_hours),
                snap.taken_at,
            ) {
                TabAssessment::Eligible(_) => {
                    ("eligible", "Eligible after the warning period".into())
                }
                TabAssessment::Pinned => ("pinned", "Pinned tabs stay open".into()),
                TabAssessment::Selected => ("selected", "Selected tabs stay open".into()),
                TabAssessment::UnknownActivity => (
                    "unknown_activity",
                    "No reliable last-selected time; stays open".into(),
                ),
                TabAssessment::Waiting { remaining_secs } => (
                    "waiting",
                    format!("Eligible in {}", autotrim::fmt::dur(remaining_secs)),
                ),
                TabAssessment::Unavailable => (
                    "unavailable",
                    "Tab closing is unavailable on this platform".into(),
                ),
                TabAssessment::NotListed => continue,
            };
            let profile = browser
                .open_profiles
                .iter()
                .find(|p| p.dir == tab.profile)
                .map(|p| p.label.clone())
                .unwrap_or_else(|| tab.profile.clone());
            rows.push(TabRulePreview {
                browser: browser.name.clone(),
                profile,
                window_id: tab.window_id,
                id: tab.id,
                title: tab.title.clone(),
                domain: domain_of_url(&tab.url).unwrap_or_default(),
                idle_secs: tab
                    .last_active
                    .filter(|last| *last > 0 && *last <= snap.taken_at)
                    .map(|last| snap.taken_at - last),
                status,
                reason,
            });
        }
    }
    Ok(rows)
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn set_tab_rules(
    domains: Vec<DomainRule>,
    inactive_hours: f64,
    expected_revision: String,
) -> Result<Settings, String> {
    off_thread(move || {
        let rules = validate_domain_rules(&domains, inactive_hours)?;
        let (cfg, _) = Config::load()?;
        let current = autotrim::daemon::DaemonConfig::from_config(&cfg, &Default::default());
        anyhow::ensure!(
            current.auto.tab_rules_revision() == expected_revision,
            "settings changed; review the current rules and try again"
        );
        Config::set_values_if_unchanged(
            &[
                ("auto_tab_domains", serialize_rules(&rules)?),
                ("auto_tab_inactive_hours", inactive_hours.to_string()),
            ],
            &cfg.file_revision,
        )?;
        Ok(settings_now())
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn preview_tab_rules(
    domains: Vec<DomainRule>,
    inactive_hours: f64,
) -> Result<Vec<TabRulePreview>, String> {
    off_thread(move || {
        let rules = validate_domain_rules(&domains, inactive_hours)?;
        let (cfg, _) = Config::load()?;
        let snap = scan_now(&cfg.thresholds(), Duration::from_millis(300));
        preview_rows(&snap, &rules, inactive_hours)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_reports_each_protection_and_omits_unlisted_domains() {
        let tab = |id, url, active, pinned, last| {
            serde_json::json!({
                "id": id, "window_id": 1, "profile": "Default", "index": id,
                "url": url, "site": "display label is not authorization", "title": "Fixture",
                "active": active, "pinned": pinned, "last_active": last,
            })
        };
        let snap: Snapshot = serde_json::from_value(serde_json::json!({
            "taken_at": 100000, "scanner_pid": 1,
            "system": {"os":"fixture", "total_mem":0,"used_mem":0,"available_mem":0,"total_swap":0,"used_swap":0,"uptime_secs":0},
            "groups":[],"sessions":[],"advice":[],
            "browsers":[{"name":"Google Chrome","rss":0,"procs":1,"renderers":0,"extension_renderers":0,"gpu":0,"utility":0,"can_close_tabs":true,
                "tabs":[tab(1,"https://reddit.com",false,false,Some(1)),
                    tab(2,"https://reddit.com",true,false,Some(1)),
                    tab(3,"https://reddit.com",false,true,Some(1)),
                    tab(4,"https://reddit.com",false,false,None),
                    tab(5,"https://reddit.com",false,false,Some(99000)),
                    tab(6,"https://reddit.com.example.org",false,false,Some(1))]}]
        })).unwrap();
        let rules = vec![DomainRule {
            domain: "reddit.com".into(),
            include_subdomains: false,
        }];
        let rows = preview_rows(&snap, &rules, 24.0).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.status).collect::<Vec<_>>(),
            [
                "eligible",
                "selected",
                "pinned",
                "unknown_activity",
                "waiting"
            ]
        );
        assert_eq!(rows[0].domain, "reddit.com");
        assert_eq!(rows[0].idle_secs, Some(99999));
    }

    #[test]
    fn preview_distinguishes_unavailable_data_from_no_matches() {
        let mut snap: Snapshot = serde_json::from_value(serde_json::json!({
            "taken_at": 100000, "scanner_pid": 1,
            "system": {"os":"fixture", "total_mem":0,"used_mem":0,"available_mem":0,"total_swap":0,"used_swap":0,"uptime_secs":0},
            "groups":[],"sessions":[],"advice":[],
            "browsers":[{"name":"Google Chrome","rss":0,"procs":1,"renderers":0,"extension_renderers":0,"gpu":0,"utility":0,
                "can_close_tabs":true,"tabs":[],"tabs_note":"no session file is readable"}]
        })).unwrap();
        let rules = vec![DomainRule {
            domain: "example.com".into(),
            include_subdomains: false,
        }];
        assert!(preview_rows(&snap, &rules, 24.0).is_err());
        snap.browsers[0].tabs_note = None;
        assert!(preview_rows(&snap, &rules, 24.0).unwrap().is_empty());
    }
}
