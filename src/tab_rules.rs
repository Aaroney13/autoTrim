//! Domain validation and the shared classification used by preview, policy,
//! and the action-time safety check for automatic Chrome tab cleanup.

use crate::browser::{BrowserInfo, TabInfo};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use url::{Host, Url};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DomainRule {
    pub domain: String,
    #[serde(default)]
    pub include_subdomains: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoTabReason {
    EmptyNewTab,
    Domain { domain: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabAssessment {
    Eligible(AutoTabReason),
    Pinned,
    Selected,
    UnknownActivity,
    Waiting { remaining_secs: u64 },
    NotListed,
    Unavailable,
}

/// Validate and canonicalize a bare DNS hostname for storage in a rule.
pub fn normalize_domain(input: &str) -> Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("domain cannot be empty");
    }
    if trimmed.chars().any(char::is_whitespace) {
        bail!("domain cannot contain whitespace");
    }
    if trimmed.contains(['/', '?', '#', '@', ':', '*', '%']) {
        bail!("enter a bare domain without a scheme, path, port, or wildcard");
    }

    let bare = trimmed.strip_suffix('.').unwrap_or(trimmed);
    let Host::Domain(domain) = Host::parse(bare).map_err(|_| anyhow::anyhow!("invalid domain"))?
    else {
        bail!("IP addresses are not supported");
    };
    let domain = domain.to_ascii_lowercase();
    if domain.len() > 253 || !domain.contains('.') {
        bail!("domain must contain at least two DNS labels");
    }
    for label in domain.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            bail!("domain contains an invalid DNS label");
        }
    }
    Ok(domain)
}

/// The canonical hostname of an HTTP(S) URL, excluding IP literals.
pub fn domain_of_url(input: &str) -> Option<String> {
    let parsed = Url::parse(input).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    match parsed.host()? {
        Host::Domain(domain) => normalize_domain(domain).ok(),
        Host::Ipv4(_) | Host::Ipv6(_) => None,
    }
}

pub fn matches_url(rule: &DomainRule, input: &str) -> bool {
    let Ok(rule_domain) = normalize_domain(&rule.domain) else {
        return false;
    };
    let Some(host) = domain_of_url(input) else {
        return false;
    };
    host == rule_domain
        || (rule.include_subdomains
            && host
                .strip_suffix(&rule_domain)
                .is_some_and(|prefix| prefix.ends_with('.')))
}

fn normalize_rules(domains: &[DomainRule]) -> Result<Vec<DomainRule>> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(domains.len());
    for rule in domains {
        let domain = normalize_domain(&rule.domain)?;
        if !seen.insert(domain.clone()) {
            bail!("duplicate auto-close domain: {domain}");
        }
        normalized.push(DomainRule {
            domain,
            include_subdomains: rule.include_subdomains,
        });
    }
    Ok(normalized)
}

pub fn validate_domain_rules(
    domains: &[DomainRule],
    inactive_hours: f64,
) -> Result<Vec<DomainRule>> {
    if !(1.0 / 6.0..=8760.0).contains(&inactive_hours) {
        bail!("auto tab inactivity must be between 10 minutes and 8760 hours");
    }
    normalize_rules(domains)
}

/// Convert validated fractional hours to the shared second-based runtime timer.
pub fn inactivity_secs(hours: f64) -> u64 {
    (hours * 3600.0).round() as u64
}

/// Serialize canonical rules as the inline TOML array accepted by Config::set_values.
pub fn serialize_rules(domains: &[DomainRule]) -> Result<String> {
    let normalized = normalize_rules(domains)?;
    let entries = normalized
        .iter()
        .map(|rule| {
            format!(
                "{{ domain = \"{}\", include_subdomains = {} }}",
                rule.domain, rule.include_subdomains
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!("[{entries}]"))
}

fn is_empty_new_tab(url: &str) -> bool {
    matches!(
        url.strip_suffix('/').unwrap_or(url),
        "chrome://newtab" | "chrome://new-tab-page" | "chrome://new-tab-page-third-party"
    )
}

fn available(browser: &BrowserInfo) -> bool {
    browser.name == "Google Chrome" && browser.can_close_tabs
}

pub fn assess_tab(
    browser: &BrowserInfo,
    tab: &TabInfo,
    rules: &[DomainRule],
    inactive_secs: u64,
    now: u64,
) -> TabAssessment {
    if is_empty_new_tab(&tab.url) {
        if !available(browser) {
            return TabAssessment::Unavailable;
        }
        if tab.pinned {
            return TabAssessment::Pinned;
        }
        if tab.active {
            return TabAssessment::Selected;
        }
        return TabAssessment::Eligible(AutoTabReason::EmptyNewTab);
    }

    let Some(host) = domain_of_url(&tab.url) else {
        return TabAssessment::NotListed;
    };
    let matched_rule = rules
        .iter()
        .filter_map(|rule| {
            normalize_domain(&rule.domain)
                .ok()
                .filter(|domain| {
                    host == *domain
                        || (rule.include_subdomains
                            && host
                                .strip_suffix(domain.as_str())
                                .is_some_and(|prefix| prefix.ends_with('.')))
                })
                .map(|domain| (domain.len(), domain))
        })
        .max_by(|a, b| a.cmp(b));
    let Some((_, matched_domain)) = matched_rule else {
        return TabAssessment::NotListed;
    };

    if !available(browser) {
        return TabAssessment::Unavailable;
    }
    if tab.pinned {
        return TabAssessment::Pinned;
    }
    if tab.active {
        return TabAssessment::Selected;
    }
    let Some(last_active) = tab.last_active.filter(|last| *last != 0 && *last <= now) else {
        return TabAssessment::UnknownActivity;
    };
    let elapsed = now - last_active;
    if elapsed < inactive_secs {
        return TabAssessment::Waiting {
            remaining_secs: inactive_secs - elapsed,
        };
    }
    TabAssessment::Eligible(AutoTabReason::Domain {
        domain: matched_domain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{BrowserInfo, PageKind, TabInfo};

    fn rule(domain: &str, include_subdomains: bool) -> DomainRule {
        DomainRule {
            domain: domain.to_string(),
            include_subdomains,
        }
    }

    fn browser(name: &str, can_close_tabs: bool) -> BrowserInfo {
        BrowserInfo {
            name: name.to_string(),
            rss: 0,
            procs: 0,
            renderers: 0,
            tab_sized_renderers: 0,
            small_renderers: 0,
            extension_renderers: 0,
            gpu: 0,
            utility: 0,
            profiles: None,
            renderer_rss: 0,
            tabs: Vec::new(),
            windows: 0,
            open_profiles: Vec::new(),
            sites: Vec::new(),
            stale_tabs: 0,
            chat_tabs: 0,
            stale_chat_tabs: 0,
            per_tab_estimate: None,
            can_close_tabs,
            tabs_note: None,
            renderer_procs: Vec::new(),
        }
    }

    fn tab(url: &str, last_active: Option<u64>) -> TabInfo {
        TabInfo {
            id: 1,
            window_id: 2,
            profile: "Default".to_string(),
            index: 0,
            url: url.to_string(),
            site: String::new(),
            title: "test".to_string(),
            pinned: false,
            active: false,
            last_active,
            idle_secs: None,
            kind: PageKind::Page,
        }
    }

    #[test]
    fn normalize_domain_canonicalizes_case_idna_and_one_trailing_dot() {
        assert_eq!(normalize_domain(" Example.COM. ").unwrap(), "example.com");
        assert_eq!(normalize_domain("bücher.de").unwrap(), "xn--bcher-kva.de");
    }

    #[test]
    fn normalize_domain_rejects_non_dns_or_non_bare_hosts() {
        for invalid in [
            "example%2Ecom",
            "%65xample.com",
            "",
            "example com",
            "https://example.com",
            "example.com/path",
            "example.com?q=1",
            "example.com#part",
            "me@example.com",
            "example.com:443",
            "*.example.com",
            "127.0.0.1",
            "[::1]",
            "localhost",
            "internal",
            "-bad.example",
            "bad-.example",
            "bad_name.example",
            "example.com..",
        ] {
            assert!(normalize_domain(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn matches_url_obeys_exact_and_subdomain_boundaries() {
        let exact = rule("reddit.com", false);
        let nested = rule("reddit.com", true);
        assert!(matches_url(&exact, "https://reddit.com:8443/r/rust"));
        assert!(!matches_url(&exact, "https://old.reddit.com/"));
        assert!(matches_url(&nested, "http://old.reddit.com/"));
        assert!(!matches_url(&nested, "https://notreddit.com/"));
        assert!(!matches_url(&nested, "https://reddit.com.example.org/"));
        assert!(!matches_url(&nested, "https://example.org/path/reddit.com"));
    }

    #[test]
    fn matches_url_requires_http_domain_hosts() {
        let domain = rule("example.com", true);
        assert!(!matches_url(&domain, "ftp://example.com/file"));
        assert!(!matches_url(&domain, "https://127.0.0.1/"));
        assert!(!matches_url(&domain, "https://example.com../"));
        assert!(!matches_url(&domain, "not a url"));
        assert_eq!(
            domain_of_url("HTTPS://BÜCHER.DE.:443/a"),
            Some("xn--bcher-kva.de".to_string())
        );
        assert_eq!(domain_of_url("https://example.com../"), None);
    }

    #[test]
    fn validate_domain_rules_canonicalizes_and_rejects_duplicates_and_bad_hours() {
        assert_eq!(
            validate_domain_rules(&[rule(" Example.COM. ", true)], 24.0).unwrap(),
            vec![rule("example.com", true)]
        );
        assert!(validate_domain_rules(&[rule("example.com", false)], 0.0).is_err());
        assert!(validate_domain_rules(&[rule("example.com", false)], 8761.0).is_err());
        assert!(
            validate_domain_rules(
                &[rule("EXAMPLE.com", false), rule("example.com.", true)],
                24.0
            )
            .is_err()
        );
    }

    #[test]
    fn serialize_rules_is_a_single_line_inline_toml_value() {
        let value = serialize_rules(&[rule("Example.COM", true), rule("x.com.", false)]).unwrap();
        assert_eq!(
            value,
            r#"[{ domain = "example.com", include_subdomains = true }, { domain = "x.com", include_subdomains = false }]"#
        );
        assert!(!value.contains('\n'));
    }

    #[test]
    fn assess_tab_matches_before_reporting_protections() {
        let chrome = browser("Google Chrome", true);
        let mut listed = tab("https://old.reddit.com/post", Some(1));
        listed.pinned = true;
        let mut unrelated = listed.clone();
        unrelated.url = "https://example.org/".to_string();
        unrelated.active = true;
        assert_eq!(
            assess_tab(&chrome, &listed, &[rule("reddit.com", true)], 10, 100),
            TabAssessment::Pinned
        );
        assert_eq!(
            assess_tab(&chrome, &unrelated, &[rule("reddit.com", true)], 10, 100),
            TabAssessment::NotListed
        );
    }

    #[test]
    fn assess_tab_reports_selection_activity_and_waiting() {
        let chrome = browser("Google Chrome", true);
        let rules = [rule("example.com", false)];
        let mut selected = tab("https://example.com", Some(1));
        selected.active = true;
        assert_eq!(
            assess_tab(&chrome, &selected, &rules, 60, 100),
            TabAssessment::Selected
        );
        for last_active in [None, Some(0), Some(101)] {
            let unknown = tab("https://example.com", last_active);
            assert_eq!(
                assess_tab(&chrome, &unknown, &rules, 60, 100),
                TabAssessment::UnknownActivity
            );
        }
        let waiting = tab("https://example.com", Some(70));
        assert_eq!(
            assess_tab(&chrome, &waiting, &rules, 60, 100),
            TabAssessment::Waiting { remaining_secs: 30 }
        );
        let eligible = tab("https://example.com", Some(40));
        assert_eq!(
            assess_tab(&chrome, &eligible, &rules, 60, 100),
            TabAssessment::Eligible(AutoTabReason::Domain {
                domain: "example.com".to_string()
            })
        );
    }

    #[test]
    fn assess_tab_reports_the_most_specific_matching_rule() {
        let chrome = browser("Google Chrome", true);
        let rules = [rule("example.com", true), rule("news.example.com", true)];
        let eligible = tab("https://local.news.example.com/article", Some(1));
        assert_eq!(
            assess_tab(&chrome, &eligible, &rules, 10, 100),
            TabAssessment::Eligible(AutoTabReason::Domain {
                domain: "news.example.com".to_string()
            })
        );
    }

    #[test]
    fn assess_tab_preserves_empty_new_tab_behavior_without_activity() {
        let chrome = browser("Google Chrome", true);
        let new_tab = tab("chrome://newtab/", None);
        assert_eq!(
            assess_tab(&chrome, &new_tab, &[], 86_400, 100),
            TabAssessment::Eligible(AutoTabReason::EmptyNewTab)
        );

        let mut pinned = new_tab.clone();
        pinned.pinned = true;
        assert_eq!(
            assess_tab(&chrome, &pinned, &[], 86_400, 100),
            TabAssessment::Pinned
        );
    }

    #[test]
    fn assess_tab_only_marks_matching_tabs_unavailable() {
        let chromium = browser("Chromium", false);
        let rules = [rule("example.com", false)];
        assert_eq!(
            assess_tab(
                &chromium,
                &tab("https://example.com", Some(1)),
                &rules,
                10,
                100
            ),
            TabAssessment::Unavailable
        );
        assert_eq!(
            assess_tab(
                &chromium,
                &tab("https://other.example", Some(1)),
                &rules,
                10,
                100
            ),
            TabAssessment::NotListed
        );
        assert_eq!(
            assess_tab(
                &chromium,
                &tab("chrome://new-tab-page", None),
                &rules,
                10,
                100
            ),
            TabAssessment::Unavailable
        );
    }
}
