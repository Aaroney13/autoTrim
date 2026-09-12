//! Small formatting helpers shared by the report and the rules.

pub fn bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    let f = b as f64;
    if f >= KB * KB * KB {
        format!("{:.1} GB", f / (KB * KB * KB))
    } else if f >= KB * KB {
        format!("{:.0} MB", f / (KB * KB))
    } else if f >= KB {
        format!("{:.0} KB", f / KB)
    } else {
        format!("{b} B")
    }
}

pub fn dur(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

pub fn pct(num: u64, den: u64) -> String {
    if den == 0 {
        "n/a".to_string()
    } else {
        format!("{:.0}%", num as f64 * 100.0 / den as f64)
    }
}

/// Fit a string into `width` columns, truncating from the left (keeps the
/// tail, which is the informative end of a path).
pub fn fit_left(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    let keep: String = s.chars().skip(n - (width - 1)).collect();
    format!("…{keep}")
}

/// Fit a string into `width` columns, truncating from the right.
pub fn fit_right(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    let keep: String = s.chars().take(width - 1).collect();
    format!("{keep}…")
}
