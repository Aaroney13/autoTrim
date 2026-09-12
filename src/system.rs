//! System-wide memory totals, swap, compressed memory, and uptime.
//! Platform-specific detail lives in the `platform` submodule.

use serde::Serialize;
use std::path::PathBuf;
use sysinfo::System;

#[derive(Serialize, Clone, Debug)]
pub struct SystemInfo {
    pub os: String,
    pub total_mem: u64,
    pub used_mem: u64,
    pub available_mem: u64,
    pub total_swap: u64,
    pub used_swap: u64,
    /// Bytes held by the memory compressor (macOS). None where unknown.
    pub compressed: Option<u64>,
    /// Bytes wired by the kernel and drivers (macOS). None where unknown.
    pub wired: Option<u64>,
    /// System-wide free percentage as the OS reports it (macOS). None where unknown.
    pub free_pct: Option<u8>,
    pub uptime_secs: u64,
}

impl SystemInfo {
    pub fn swap_frac(&self) -> f64 {
        if self.total_swap == 0 {
            0.0
        } else {
            self.used_swap as f64 / self.total_swap as f64
        }
    }
}

pub fn collect(sys: &mut System) -> SystemInfo {
    sys.refresh_memory();
    let (compressed, wired) = platform::vm_details();
    SystemInfo {
        os: System::long_os_version().unwrap_or_else(|| "unknown".to_string()),
        total_mem: sys.total_memory(),
        used_mem: sys.used_memory(),
        available_mem: sys.available_memory(),
        total_swap: sys.total_swap(),
        used_swap: sys.used_swap(),
        compressed,
        wired,
        free_pct: platform::free_pct(),
        uptime_secs: System::uptime(),
    }
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::process::Command;

    /// Parse `vm_stat` for compressor and wired page counts.
    /// TODO: replace with host_statistics64 via mach once the daemon exists;
    /// shelling out is fine for a one-shot scan.
    pub fn vm_details() -> (Option<u64>, Option<u64>) {
        let Ok(out) = Command::new("vm_stat").output() else {
            return (None, None);
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let page_size = text
            .lines()
            .next()
            .and_then(|l| l.split("page size of ").nth(1))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(4096);
        let pages = |key: &str| -> Option<u64> {
            text.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split(':').nth(1))
                .map(|v| v.trim().trim_end_matches('.'))
                .and_then(|v| v.parse::<u64>().ok())
                .map(|n| n * page_size)
        };
        (
            pages("Pages occupied by compressor"),
            pages("Pages wired down"),
        )
    }

    pub fn free_pct() -> Option<u8> {
        let out = Command::new("memory_pressure").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .find(|l| l.contains("free percentage"))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.trim().trim_end_matches('%'))
            .and_then(|v| v.parse::<u8>().ok())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn vm_details() -> (Option<u64>, Option<u64>) {
        (None, None)
    }
    pub fn free_pct() -> Option<u8> {
        None
    }
}
