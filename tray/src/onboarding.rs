//! Persist wizard choices using the same config and service paths as Settings.
use super::{Settings, cli, off_thread, service, settings_now};
use autotrim::config::Config;
use autotrim::tab_rules::{DomainRule, matches_url, normalize_domain, serialize_rules};

#[derive(serde::Deserialize)]
pub(crate) struct Choices {
    focus_areas: Vec<String>,
    stale_after_hours: f64,
    tab_stale_after_hours: f64,
    notify: bool,
    background: bool,
    open_window_at_launch: bool,
    #[serde(default)]
    add_tab_domains: Vec<String>,
    #[serde(default)]
    tab_rules_revision: String,
}

impl Choices {
    fn pairs(&self) -> anyhow::Result<Vec<(&'static str, String)>> {
        for domain in &self.add_tab_domains {
            normalize_domain(domain)?;
        }
        anyhow::ensure!(
            !self.focus_areas.is_empty()
                && self.focus_areas.len() <= 3
                && self
                    .focus_areas
                    .iter()
                    .all(|s| matches!(s.as_str(), "browser" | "agent" | "app")),
            "Choose at least one of browser tabs, AI sessions, or apps."
        );
        for hours in [self.stale_after_hours, self.tab_stale_after_hours] {
            anyhow::ensure!(
                hours.is_finite() && (0.25..=8760.0).contains(&hours),
                "Idle thresholds must be between 0.25 and 8760 hours."
            );
        }
        Ok(vec![
            ("focus_areas", serde_json::to_string(&self.focus_areas)?),
            ("stale_after_hours", self.stale_after_hours.to_string()),
            (
                "tab_stale_after_hours",
                self.tab_stale_after_hours.to_string(),
            ),
            ("notify", self.notify.to_string()),
            (
                "open_window_at_launch",
                self.open_window_at_launch.to_string(),
            ),
        ])
    }

    fn merged_rules(&self, cfg: &Config) -> anyhow::Result<Vec<DomainRule>> {
        let mut rules = cfg.auto_tab_domains.clone();
        for domain in &self.add_tab_domains {
            let domain = normalize_domain(domain)?;
            if !rules
                .iter()
                .any(|rule| matches_url(rule, &format!("https://{domain}/")))
            {
                rules.push(DomainRule {
                    domain,
                    include_subdomains: false,
                });
            }
        }
        // Allow a retry after preferences saved but service startup failed.
        if rules != cfg.auto_tab_domains {
            let current = autotrim::daemon::DaemonConfig::from_config(cfg, &Default::default());
            anyhow::ensure!(
                current.auto.tab_rules_revision() == self.tab_rules_revision,
                "Chrome cleanup settings changed. Cancel and reopen setup to review the current rules."
            );
        }
        Ok(rules)
    }
}

#[tauri::command]
pub(crate) async fn complete_onboarding(choices: Choices) -> Result<Settings, String> {
    off_thread(move || {
        let mut pairs = choices.pairs()?;
        // Never overwrite an unreadable config with wizard defaults.
        let (cfg, _) = Config::load()?;
        let rules = choices.merged_rules(&cfg)?;
        if rules != cfg.auto_tab_domains {
            pairs.push(("auto_tab_domains", serialize_rules(&rules)?));
        }
        let info = service::info();
        anyhow::ensure!(!choices.background || info.supported,
            "Background startup is currently supported on macOS only.");
        let exe = if choices.background && (!info.installed || !info.running) {
            Some(cli::find_cli().ok_or_else(|| anyhow::anyhow!(
                "The autotrim command could not be found. Install the app bundle or choose Open manually."
            ))?)
        } else {
            None
        };
        Config::set_values_if_unchanged(&pairs, &cfg.file_revision)?;
        // Save settings first so a newly started daemon uses the chosen thresholds.
        // Leave first-run incomplete on failure: the user can retry or choose manual.
        let result = if let Some(exe) = exe {
            service::install(Some(&exe)).map(|_| ())
        } else if !choices.background && info.installed {
            service::uninstall()
        } else {
            Ok(())
        };
        result.map_err(|e| anyhow::anyhow!(
            "Preferences saved, but startup could not be changed: {e}. Retry or choose Open manually."
        ))?;
        Config::set_values(&[("onboarding_completed", "true".into())])?;
        Ok(settings_now())
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices() -> Choices {
        Choices {
            focus_areas: vec!["browser".into(), "agent".into()],
            stale_after_hours: 6.0,
            tab_stale_after_hours: 24.0,
            notify: true,
            background: false,
            open_window_at_launch: true,
            add_tab_domains: vec![],
            tab_rules_revision: String::new(),
        }
    }

    #[test]
    fn rejects_invalid_choices_before_any_side_effect() {
        let mut c = choices();
        c.focus_areas.clear();
        assert!(c.pairs().is_err());
        c.focus_areas = vec!["unknown".into()];
        assert!(c.pairs().is_err());
        c = choices();
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, 8761.0] {
            c.stale_after_hours = invalid;
            assert!(c.pairs().is_err());
        }
    }

    #[test]
    fn setup_does_not_enable_cleanup_or_mark_completion_early() {
        let pairs = choices().pairs().unwrap();
        assert!(
            pairs
                .iter()
                .all(|(key, _)| !key.starts_with("auto_") && *key != "onboarding_completed")
        );
        assert!(
            pairs
                .iter()
                .any(|(key, value)| *key == "focus_areas" && value == "[\"browser\",\"agent\"]")
        );
    }

    #[test]
    fn whitelist_additions_preserve_rules_and_retry_without_duplicates() {
        let mut cfg = Config::default();
        cfg.auto_tab_domains.push(DomainRule {
            domain: "example.com".into(),
            include_subdomains: true,
        });
        let mut c = choices();
        c.tab_rules_revision =
            autotrim::daemon::DaemonConfig::from_config(&cfg, &Default::default())
                .auto
                .tab_rules_revision();
        c.add_tab_domains = vec![
            "news.example.com".into(),
            " Shop.example.org. ".into(),
            "shop.example.org".into(),
        ];
        let merged = c.merged_rules(&cfg).unwrap();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0], cfg.auto_tab_domains[0]);
        assert_eq!(
            merged[1],
            DomainRule {
                domain: "shop.example.org".into(),
                include_subdomains: false
            }
        );
        cfg.auto_tab_domains = merged.clone();
        assert_eq!(c.merged_rules(&cfg).unwrap(), merged);
        assert!(
            c.pairs()
                .unwrap()
                .iter()
                .all(|(key, _)| !key.starts_with("auto_"))
        );
    }

    #[test]
    fn whitelist_rejects_invalid_domains_and_stale_authorization() {
        let mut c = choices();
        c.add_tab_domains = vec!["https://example.com".into()];
        assert!(c.pairs().is_err());
        c.add_tab_domains = vec!["example.com".into()];
        assert!(c.merged_rules(&Config::default()).is_err());
    }
}
