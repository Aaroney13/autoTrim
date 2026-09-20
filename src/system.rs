//! System-wide memory totals, swap, compressed memory, and uptime.
//! Platform-specific detail lives in the `platform` submodule.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use sysinfo::System;

#[derive(Serialize, Deserialize, Clone, Debug)]
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
    /// Legacy macOS availability counter, not `100 - used / total` and not
    /// unused RAM. Retained for snapshot/history compatibility, not display.
    pub free_pct: Option<u8>,
    pub uptime_secs: u64,
    /// Whole-machine CPU percent over the sample window (100 = every core busy).
    #[serde(default)]
    pub cpu_pct: f32,
    #[serde(default)]
    pub load_one: f64,
    #[serde(default)]
    pub load_five: f64,
    #[serde(default)]
    pub load_fifteen: f64,
}

/// Whole-machine counters sampled around an action. These are observations,
/// not savings attributable to one process; other apps keep running.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MemorySample {
    pub taken_at_ms: u64,
    pub used_mem: u64,
    pub used_swap: u64,
}

impl MemorySample {
    pub fn collect() -> Option<Self> {
        let mut sys = System::new();
        sys.refresh_memory();
        (sys.total_memory() > 0).then(|| Self {
            taken_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            used_mem: sys.used_memory(),
            used_swap: sys.used_swap(),
        })
    }
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
        cpu_pct: sys.global_cpu_usage(),
        load_one: System::load_average().one,
        load_five: System::load_average().five,
        load_fifteen: System::load_average().fifteen,
    }
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::mem;
    use std::sync::OnceLock;

    /// The host port is a send right; asking for it once and keeping it avoids
    /// leaking one per tick. libc flags the mach entry points as deprecated
    /// in favour of the `mach2` crate; the ABI is stable and one dependency
    /// fewer matters here.
    #[allow(deprecated)]
    fn host() -> libc::mach_port_t {
        static HOST: OnceLock<libc::mach_port_t> = OnceLock::new();
        *HOST.get_or_init(|| unsafe { libc::mach_host_self() })
    }

    /// Compressor and wired bytes from `host_statistics64`, the same counters
    /// `vm_stat` prints.
    pub fn vm_details() -> (Option<u64>, Option<u64>) {
        let mut stats: libc::vm_statistics64 = unsafe { mem::zeroed() };
        let mut count = (mem::size_of::<libc::vm_statistics64>()
            / mem::size_of::<libc::integer_t>())
            as libc::mach_msg_type_number_t;
        #[allow(deprecated)]
        let kr = unsafe {
            libc::host_statistics64(
                host(),
                libc::HOST_VM_INFO64,
                &mut stats as *mut libc::vm_statistics64 as *mut libc::integer_t,
                &mut count,
            )
        };
        if kr != 0 {
            return (None, None);
        }
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        (
            Some(stats.compressor_page_count as u64 * page),
            Some(stats.wire_count as u64 * page),
        )
    }

    /// `kern.memorystatus_level` is the number `memory_pressure` reports as
    /// "system-wide memory free percentage".
    pub fn free_pct() -> Option<u8> {
        let mut val: libc::c_int = 0;
        let mut len = mem::size_of::<libc::c_int>();
        let r = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_level".as_ptr(),
                &mut val as *mut libc::c_int as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (r == 0).then(|| val.clamp(0, 100) as u8)
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
