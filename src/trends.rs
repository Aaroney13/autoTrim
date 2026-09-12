//! Rolling series per app group and per session, and what a line through
//! them says: growth rate, how steady it is, sustained CPU. The daemon
//! feeds these every tick; a one-shot scan has none.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// One observation: seconds since the epoch, resident bytes, CPU percent.
pub type Sample = (u64, u64, f32);

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Series {
    pub name: String,
    /// "app", "agent", "session", or "renderer" (one browser page).
    pub kind: String,
    pub samples: VecDeque<Sample>,
}

impl Series {
    pub fn push(&mut self, s: Sample, window: u64) {
        self.samples.push_back(s);
        let now = s.0;
        while self
            .samples
            .front()
            .is_some_and(|(t, _, _)| now.saturating_sub(*t) > window)
        {
            self.samples.pop_front();
        }
    }
}

/// What the series says right now.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Trend {
    pub key: String,
    pub name: String,
    pub kind: String,
    pub samples: usize,
    pub span_secs: u64,
    pub rss_now: u64,
    pub rss_start: u64,
    /// Last minus first, may be negative.
    pub growth: i64,
    /// Least-squares slope of memory over time.
    pub bytes_per_hour: f64,
    /// Fraction of consecutive samples that went up.
    pub rising_frac: f32,
    /// Goodness of the linear fit, 0 to 1.
    pub r2: f32,
    /// CPU over the most recent `cpu_span_secs` of the series.
    pub cpu_mean: f32,
    pub cpu_min: f32,
    pub cpu_span_secs: u64,
}

pub fn analyze(key: &str, s: &Series, cpu_window: u64) -> Option<Trend> {
    let n = s.samples.len();
    if n < 3 {
        return None;
    }
    let (t0, rss0, _) = s.samples[0];
    let (t1, rss1, _) = s.samples[n - 1];
    let span = t1.saturating_sub(t0);
    if span == 0 {
        return None;
    }
    // Least squares of rss (bytes) against hours since t0.
    let xs: Vec<f64> = s
        .samples
        .iter()
        .map(|(t, _, _)| (t - t0) as f64 / 3600.0)
        .collect();
    let ys: Vec<f64> = s.samples.iter().map(|(_, r, _)| *r as f64).collect();
    let mx = xs.iter().sum::<f64>() / n as f64;
    let my = ys.iter().sum::<f64>() / n as f64;
    let sxx: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let ss_tot: f64 = ys.iter().map(|y| (y - my).powi(2)).sum();
    let ss_res: f64 = xs
        .iter()
        .zip(&ys)
        .map(|(x, y)| (y - (my + slope * (x - mx))).powi(2))
        .sum();
    let r2 = if ss_tot > 0.0 {
        (1.0 - ss_res / ss_tot).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let rising = s
        .samples
        .iter()
        .zip(s.samples.iter().skip(1))
        .filter(|(a, b)| b.1 > a.1)
        .count();
    let rising_frac = rising as f32 / (n - 1) as f32;

    let cut = t1.saturating_sub(cpu_window);
    let recent: Vec<f32> = s
        .samples
        .iter()
        .filter(|(t, _, _)| *t >= cut)
        .map(|(_, _, c)| *c)
        .collect();
    let cpu_mean = recent.iter().sum::<f32>() / recent.len().max(1) as f32;
    let cpu_min = recent.iter().cloned().fold(f32::INFINITY, f32::min);
    let cpu_span = s
        .samples
        .iter()
        .find(|(t, _, _)| *t >= cut)
        .map(|(t, _, _)| t1.saturating_sub(*t))
        .unwrap_or(0);

    Some(Trend {
        key: key.to_string(),
        name: s.name.clone(),
        kind: s.kind.clone(),
        samples: n,
        span_secs: span,
        rss_now: rss1,
        rss_start: rss0,
        growth: rss1 as i64 - rss0 as i64,
        bytes_per_hour: slope,
        rising_frac,
        r2: r2 as f32,
        cpu_mean,
        cpu_min: if cpu_min.is_finite() { cpu_min } else { 0.0 },
        cpu_span_secs: cpu_span,
    })
}

/// Everything the daemon remembers about the recent past.
#[derive(Serialize, Deserialize, Default)]
pub struct History {
    pub series: HashMap<String, Series>,
    /// (ts, used_swap, compressed, used_mem)
    pub system: VecDeque<(u64, u64, u64, u64)>,
}

impl History {
    pub fn push_system(
        &mut self,
        now: u64,
        used_swap: u64,
        compressed: u64,
        used_mem: u64,
        window: u64,
    ) {
        self.system
            .push_back((now, used_swap, compressed, used_mem));
        while self
            .system
            .front()
            .is_some_and(|(t, ..)| now.saturating_sub(*t) > window)
        {
            self.system.pop_front();
        }
    }

    pub fn observe(&mut self, key: String, name: &str, kind: &str, sample: Sample, window: u64) {
        let s = self.series.entry(key).or_default();
        if s.name.is_empty() {
            s.name = name.to_string();
            s.kind = kind.to_string();
        }
        s.push(sample, window);
    }

    /// Drop series that have not been seen this tick for a whole window.
    pub fn prune(&mut self, now: u64, window: u64) {
        self.series.retain(|_, s| {
            s.samples
                .back()
                .is_some_and(|(t, ..)| now.saturating_sub(*t) <= window)
        });
    }

    pub fn trends(&self, cpu_window: u64) -> Vec<Trend> {
        let mut out: Vec<Trend> = self
            .series
            .iter()
            .filter_map(|(k, s)| analyze(k, s, cpu_window))
            .collect();
        out.sort_by_key(|t| std::cmp::Reverse(t.growth));
        out
    }

    /// Swap growth over the most recent `secs`, in bytes (may be negative).
    pub fn swap_growth(&self, secs: u64) -> Option<(i64, u64)> {
        let (t1, s1, ..) = *self.system.back()?;
        let cut = t1.saturating_sub(secs);
        let (t0, s0, ..) = *self.system.iter().find(|(t, ..)| *t >= cut)?;
        if t1 == t0 {
            return None;
        }
        Some((s1 as i64 - s0 as i64, t1 - t0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_growth_is_measured() {
        let mut s = Series {
            name: "leaky".into(),
            kind: "app".into(),
            samples: VecDeque::new(),
        };
        for i in 0..20u64 {
            s.push((i * 60, 1_000_000_000 + i * 10_000_000, 5.0), 7_200);
        }
        let t = analyze("g:leaky", &s, 600).unwrap();
        assert!(t.rising_frac > 0.99);
        assert!(t.r2 > 0.99);
        // 10 MB per minute is 600 MB per hour.
        assert!(
            (t.bytes_per_hour - 600_000_000.0).abs() < 1_000.0,
            "{}",
            t.bytes_per_hour
        );
        assert_eq!(t.growth, 190_000_000);
    }

    #[test]
    fn flat_series_has_no_slope() {
        let mut s = Series {
            name: "flat".into(),
            kind: "app".into(),
            samples: VecDeque::new(),
        };
        for i in 0..10u64 {
            s.push((i * 60, 500_000_000, 1.0), 7_200);
        }
        let t = analyze("g:flat", &s, 600).unwrap();
        assert_eq!(t.bytes_per_hour, 0.0);
        assert_eq!(t.rising_frac, 0.0);
    }
}
