//! Persist wizard choices using the same config and service paths as Settings.
use super::{Settings, cli, off_thread, service, settings_now};
use autotrim::config::Config;

#[derive(serde::Deserialize)]
pub(crate) struct Choices {
    focus_areas: Vec<String>,
    stale_after_hours: f64,
    tab_stale_after_hours: f64,
    notify: bool,
    background: bool,
    open_window_at_launch: bool,
}

impl Choices {
    fn pairs(&self) -> anyhow::Result<Vec<(&'static str, String)>> {
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
}

#[tauri::command]
pub(crate) async fn complete_onboarding(choices: Choices) -> Result<Settings, String> {
    off_thread(move || {
        let pairs = choices.pairs()?;
        // Never overwrite an unreadable config with wizard defaults.
        Config::load()?;
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
        Config::set_values(&pairs)?;
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
}
