//! Which regular files a process has open. Used to tie an agent process to
//! the transcripts it is appending to, with no guessing and no `lsof`.

use std::path::PathBuf;

/// Whether this platform can list a process's open files at all. Where it
/// cannot, agents whose transcripts are found that way fall back to CPU
/// evidence, and an app's agent engine is reported even with nothing open.
pub const fn supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}

#[cfg(target_os = "macos")]
pub fn open_files(pid: u32) -> Vec<PathBuf> {
    macos::open_files(pid)
}

#[cfg(target_os = "linux")]
pub fn open_files(pid: u32) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .filter(|p| p.is_absolute())
        .collect()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn open_files(_pid: u32) -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CStr;
    use std::mem;
    use std::os::raw::{c_int, c_void};
    use std::path::PathBuf;

    /// From <sys/proc_info.h>. libc does not bind it.
    #[repr(C)]
    struct ProcFileInfo {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: i64,
        fi_type: i32,
        fi_guardflags: u32,
    }

    #[repr(C)]
    struct VnodeFdInfoWithPath {
        pfi: ProcFileInfo,
        pvip: libc::vnode_info_path,
    }

    const PROC_PIDFDVNODEPATHINFO: c_int = 2;

    unsafe extern "C" {
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
        fn proc_pidfdinfo(
            pid: c_int,
            fd: c_int,
            flavor: c_int,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
    }

    pub fn open_files(pid: u32) -> Vec<PathBuf> {
        let pid = pid as c_int;
        let mut out = Vec::new();
        let needed =
            unsafe { proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return out;
        }
        let n = needed as usize / mem::size_of::<libc::proc_fdinfo>();
        let mut fds: Vec<libc::proc_fdinfo> = Vec::with_capacity(n);
        let got = unsafe {
            proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr() as *mut c_void,
                needed,
            )
        };
        if got <= 0 {
            return out;
        }
        unsafe { fds.set_len(got as usize / mem::size_of::<libc::proc_fdinfo>()) };

        for fd in fds {
            if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
                continue;
            }
            let mut info: VnodeFdInfoWithPath = unsafe { mem::zeroed() };
            let size = mem::size_of::<VnodeFdInfoWithPath>() as c_int;
            let r = unsafe {
                proc_pidfdinfo(
                    pid,
                    fd.proc_fd,
                    PROC_PIDFDVNODEPATHINFO,
                    &mut info as *mut VnodeFdInfoWithPath as *mut c_void,
                    size,
                )
            };
            if r != size {
                continue;
            }
            // vip_path is declared as [[c_char; 32]; 32] to dodge an old
            // rustc limit; it is one 1024-byte NUL-terminated buffer.
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(info.pvip.vip_path.as_ptr() as *const u8, 1024)
            };
            let Ok(c) = CStr::from_bytes_until_nul(bytes) else {
                continue;
            };
            let s = c.to_string_lossy();
            if s.starts_with('/') {
                out.push(PathBuf::from(s.into_owned()));
            }
        }
        out
    }
}
