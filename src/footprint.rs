//! What a process would give back if it exited. On macOS that is
//! `phys_footprint`: private, compressed and IOKit memory, the number
//! Activity Monitor's Memory column and `footprint(1)` show. It leaves out
//! the pages shared with every other process (the dyld cache, framework
//! text), which resident size charges to each of them, and it counts pages
//! the compressor is holding, which resident size never did. Elsewhere the
//! caller falls back to resident size.

#[cfg(target_os = "macos")]
pub fn footprint(pid: u32) -> Option<u64> {
    macos::rusage(pid).map(|r| r.ri_phys_footprint)
}

/// The most `footprint` has ever been for this process, from the same call.
#[cfg(target_os = "macos")]
pub fn peak_footprint(pid: u32) -> Option<u64> {
    macos::rusage(pid).map(|r| r.ri_lifetime_max_phys_footprint)
}

#[cfg(not(target_os = "macos"))]
pub fn footprint(_pid: u32) -> Option<u64> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn peak_footprint(_pid: u32) -> Option<u64> {
    None
}

#[cfg(target_os = "macos")]
mod macos {
    /// One `proc_pid_rusage` call, no allocation. Fails for another user's
    /// process (where sysinfo's resident size is 0 too) and for one that
    /// has exited since the process table was read.
    pub fn rusage(pid: u32) -> Option<libc::rusage_info_v4> {
        // SAFETY: rusage_info_v4 is plain data. The kernel fills exactly
        // that many bytes for RUSAGE_INFO_V4, and the struct is only read
        // when the call reports success. The header types the buffer as
        // `rusage_info_t *`, a void pointer's address, hence the cast.
        let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        let r = unsafe {
            libc::proc_pid_rusage(
                pid as libc::c_int,
                libc::RUSAGE_INFO_V4,
                &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
            )
        };
        (r == 0).then_some(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn own_process_has_a_footprint() {
        let me = std::process::id();
        let now = footprint(me).expect("footprint of this process");
        assert!(now > 0 && now < 1 << 30, "{now}");
        let peak = peak_footprint(me).expect("peak of this process");
        assert!(peak >= now, "peak {peak} < now {now}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn no_such_process_is_none() {
        // Above macOS's pid ceiling, so it can never exist.
        assert_eq!(footprint(999_999), None);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_platforms_fall_back() {
        assert_eq!(footprint(std::process::id()), None);
        assert_eq!(peak_footprint(std::process::id()), None);
    }
}
