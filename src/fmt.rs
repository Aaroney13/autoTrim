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

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD` in UTC for an epoch timestamp.
pub fn date_utc(epoch: u64) -> String {
    let (y, m, d) = civil_from_days((epoch / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD HH:MM:SSZ` for an epoch timestamp.
pub fn stamp_utc(epoch: u64) -> String {
    let t = epoch % 86_400;
    format!(
        "{} {:02}:{:02}:{:02}Z",
        date_utc(epoch),
        t / 3_600,
        (t % 3_600) / 60,
        t % 60
    )
}
