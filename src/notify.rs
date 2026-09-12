//! Native notifications. Thin on purpose: the daemon decides what and when,
//! this only delivers.

use anyhow::Result;

pub fn send(title: &str, subtitle: &str, body: &str) -> Result<()> {
    let mut n = notify_rust::Notification::new();
    n.appname("autoTrim").summary(title).body(body);
    #[cfg(target_os = "macos")]
    n.subtitle(subtitle);
    #[cfg(not(target_os = "macos"))]
    let _ = subtitle;
    n.show()?;
    Ok(())
}
