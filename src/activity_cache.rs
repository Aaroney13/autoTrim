//! Bounded transcript reads. The cache retains no transcript contents.
use std::{
    collections::HashMap,
    fs::{File, Metadata},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};
const BLOCK: usize = 64 * 1024;
const MAX_LINE: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 128;
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    identity: (u64, u64),
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    change: (i64, i64),
}
impl Stamp {
    fn read(file: &File) -> io::Result<Self> {
        let m = file.metadata()?;
        if !m.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        Ok(Self {
            identity: identity(file, &m)?,
            len: m.len(),
            modified: m.modified().ok(),
            created: m.created().ok(),
            change: change(&m),
        })
    }
}
#[cfg(unix)]
fn identity(_: &File, m: &Metadata) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((m.dev(), m.ino()))
}
#[cfg(unix)]
fn change(m: &Metadata) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (m.ctime(), m.ctime_nsec())
}
#[cfg(not(unix))]
fn change(_: &Metadata) -> (i64, i64) {
    (0, 0)
}
#[cfg(windows)]
fn identity(file: &File, _: &Metadata) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    // BY_HANDLE_FILE_INFORMATION, from the Windows SDK. File index is stable
    // across appends and changes when the path is replaced.
    #[repr(C)]
    struct Info {
        attrs: u32,
        created: [u32; 2],
        accessed: [u32; 2],
        written: [u32; 2],
        volume: u32,
        size_hi: u32,
        size_lo: u32,
        links: u32,
        index_hi: u32,
        index_lo: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Info) -> i32;
    }
    let mut info = std::mem::MaybeUninit::<Info>::uninit();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok((
        info.volume as u64,
        (info.index_hi as u64) << 32 | info.index_lo as u64,
    ))
}
#[cfg(not(any(unix, windows)))]
fn identity(_: &File, _: &Metadata) -> io::Result<(u64, u64)> {
    Err(io::Error::other("file identity unsupported"))
}
struct Entry {
    usable_append: bool,
    stamp: Stamp,
    activity: Option<u64>,
    complete_activity: Option<u64>,
    partial_start: u64,
    touched: u64,
}
#[derive(Default)]
pub(crate) struct ActivityCache {
    entries: HashMap<(PathBuf, usize), Entry>,
    tick: u64,
    pub bytes_read: u64,
    pub peak_buffer: usize,
}
impl ActivityCache {
    pub fn read(&mut self, path: &Path, parser: fn(&str) -> Option<u64>) -> Option<u64> {
        let key = (path.to_path_buf(), parser as usize);
        self.tick += 1;
        let result = self.read_inner(path, &key, parser);
        if let Err(error) = &result {
            self.entries.remove(&key);
            // Cache stable unsupported files as unknown too: repeatedly
            // checking an unchanged oversized line must not re-read it.
            if error.kind() == io::ErrorKind::InvalidData
                && let Ok(file) = File::open(path)
                && let Ok(stamp) = Stamp::read(&file)
            {
                self.evict();
                self.entries.insert(
                    key,
                    Entry {
                        usable_append: false,
                        stamp,
                        activity: None,
                        complete_activity: None,
                        partial_start: 0,
                        touched: self.tick,
                    },
                );
            }
        }
        result.ok().flatten()
    }
    fn read_inner(
        &mut self,
        path: &Path,
        key: &(PathBuf, usize),
        parser: fn(&str) -> Option<u64>,
    ) -> io::Result<Option<u64>> {
        // Opening and statting verifies file identity without reading contents.
        let mut file = File::open(path)?;
        let stamp = Stamp::read(&file)?;
        if let Some(e) = self.entries.get_mut(key)
            && e.stamp == stamp
        {
            e.touched = self.tick;
            return Ok(e.activity);
        }
        let previous = self.entries.remove(key);
        let (activity, complete_activity, partial_start) = if let Some(e) = previous
            && e.usable_append
            && e.stamp.identity == stamp.identity
            && e.stamp.created == stamp.created
            && stamp.len > e.stamp.len
        {
            self.append(
                &mut file,
                e.partial_start,
                stamp.len,
                e.complete_activity,
                parser,
            )?
        } else {
            self.historical(&mut file, stamp.len, parser)?
        };
        // Do not cache an inconsistent view of a file changed during the read.
        if Stamp::read(&file)? != stamp {
            return Err(io::Error::other("transcript changed during read"));
        }
        self.evict();
        self.entries.insert(
            key.clone(),
            Entry {
                usable_append: true,
                stamp,
                activity,
                complete_activity,
                partial_start,
                touched: self.tick,
            },
        );
        Ok(activity)
    }
    fn evict(&mut self) {
        if self.entries.len() >= MAX_ENTRIES
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
    }
    fn block(&mut self, file: &mut File, start: u64, end: u64) -> io::Result<Vec<u8>> {
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0; (end - start) as usize];
        file.read_exact(&mut buf)?;
        self.bytes_read += buf.len() as u64;
        Ok(buf)
    }
    fn historical(
        &mut self,
        file: &mut File,
        len: u64,
        parser: fn(&str) -> Option<u64>,
    ) -> io::Result<(Option<u64>, Option<u64>, u64)> {
        let mut end = len;
        let mut line = Vec::new();
        let mut final_line = true;
        let mut final_activity = None;
        let mut partial_start = len;
        while end > 0 {
            let start = end.saturating_sub(BLOCK as u64);
            let buf = self.block(file, start, end)?;
            for (i, &byte) in buf.iter().enumerate().rev() {
                if byte == b'\n' {
                    if final_line {
                        line.reverse();
                        final_activity = std::str::from_utf8(&line).ok().and_then(parser);
                        partial_start = start + i as u64 + 1;
                        final_line = false;
                    } else {
                        line.reverse();
                        if let Some(ts) = std::str::from_utf8(&line).ok().and_then(parser) {
                            return Ok((final_activity.or(Some(ts)), Some(ts), partial_start));
                        }
                    }
                    line.clear();
                } else {
                    line.push(byte);
                    self.peak_buffer = self.peak_buffer.max(buf.len() + line.capacity());
                    if line.len() > MAX_LINE {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "transcript line exceeds 1 MiB; activity unknown",
                        ));
                    }
                }
            }
            end = start;
        }
        line.reverse();
        let first = std::str::from_utf8(&line).ok().and_then(parser);
        if final_line {
            Ok((first, None, 0))
        } else {
            Ok((final_activity.or(first), first, partial_start))
        }
    }
    fn append(
        &mut self,
        file: &mut File,
        start: u64,
        len: u64,
        mut activity: Option<u64>,
        parser: fn(&str) -> Option<u64>,
    ) -> io::Result<(Option<u64>, Option<u64>, u64)> {
        let mut offset = start;
        let mut partial_start = start;
        let mut line = Vec::new();
        while offset < len {
            let end = len.min(offset + BLOCK as u64);
            let buf = self.block(file, offset, end)?;
            for (i, &byte) in buf.iter().enumerate() {
                if byte == b'\n' {
                    if let Some(ts) = std::str::from_utf8(&line).ok().and_then(parser) {
                        activity = Some(ts);
                    }
                    line.clear();
                    partial_start = offset + i as u64 + 1;
                } else {
                    line.push(byte);
                    self.peak_buffer = self.peak_buffer.max(buf.len() + line.capacity());
                    if line.len() > MAX_LINE {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "transcript line exceeds 1 MiB; activity unknown",
                        ));
                    }
                }
            }
            offset = end;
        }
        Ok((
            std::str::from_utf8(&line)
                .ok()
                .and_then(parser)
                .or(activity),
            activity,
            partial_start,
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    fn parse(s: &str) -> Option<u64> {
        s.strip_prefix("activity:")?.parse().ok()
    }
    fn other(s: &str) -> Option<u64> {
        s.strip_prefix("other:")?.parse().ok()
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                "autotrim-activity-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    fn append(path: &Path, bytes: &[u8]) {
        File::options()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    #[test]
    fn unchanged_appends_partial_bookkeeping_and_parser_identity() {
        let f = Fixture::new();
        fs::write(&f.0, "activity:1\nother:9\nactiv").unwrap();
        let mut c = ActivityCache::default();
        assert_eq!(c.read(&f.0, parse), Some(1));
        let bytes = c.bytes_read;
        for _ in 0..20 {
            assert_eq!(c.read(&f.0, parse), Some(1));
        }
        assert_eq!(c.bytes_read, bytes);
        assert_eq!(c.read(&f.0, other), Some(9));
        append(&f.0, b"ity:2");
        assert_eq!(c.read(&f.0, parse), Some(2));
        append(&f.0, b"0\nbookkeeping\n");
        assert_eq!(c.read(&f.0, parse), Some(20));
        append(&f.0, b"activity:30\n");
        assert_eq!(c.read(&f.0, parse), Some(30));
    }
    #[test]
    fn truncation_replacement_deletion_malformed_and_eviction() {
        let f = Fixture::new();
        let mut c = ActivityCache::default();
        fs::write(&f.0, "activity:1000\n").unwrap();
        assert_eq!(c.read(&f.0, parse), Some(1000));
        fs::write(&f.0, "activity:2\n").unwrap();
        assert_eq!(c.read(&f.0, parse), Some(2));
        let replacement = Fixture::new();
        fs::write(&replacement.0, "activity:3\n").unwrap();
        fs::remove_file(&f.0).unwrap();
        fs::rename(&replacement.0, &f.0).unwrap();
        assert_eq!(c.read(&f.0, parse), Some(3));
        fs::remove_file(&f.0).unwrap();
        assert_eq!(c.read(&f.0, parse), None);
        fs::write(&f.0, "malformed\n").unwrap();
        assert_eq!(c.read(&f.0, parse), None);
        let fixtures: Vec<_> = (0..MAX_ENTRIES + 5).map(|_| Fixture::new()).collect();
        for f in &fixtures {
            fs::write(&f.0, "activity:5\n").unwrap();
            c.read(&f.0, parse);
        }
        assert_eq!(c.entries.len(), MAX_ENTRIES);
    }
    #[test]
    fn oversized_line_returns_unknown_and_recovers() {
        let f = Fixture::new();
        fs::write(&f.0, vec![b'x'; MAX_LINE + 1]).unwrap();
        let mut c = ActivityCache::default();
        assert_eq!(c.read(&f.0, parse), None);
        let bytes = c.bytes_read;
        assert_eq!(c.read(&f.0, parse), None);
        assert_eq!(c.bytes_read, bytes);
        fs::write(&f.0, "activity:1\n").unwrap();
        assert_eq!(c.read(&f.0, parse), Some(1));
    }
    #[test]
    #[ignore = "repeatable 128 MiB I/O benchmark; cargo test -p autotrim activity_cache::tests::benchmark -- --ignored --nocapture"]
    fn benchmark() {
        let f = Fixture::new();
        let mut file = File::create(&f.0).unwrap();
        file.write_all(b"activity:1\n").unwrap();
        let block = b"bookkeeping entry without activity\n".repeat(1900);
        for _ in 0..2048 {
            file.write_all(&block).unwrap();
        }
        drop(file);
        let mut c = ActivityCache::default();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(c.read(&f.0, parse), Some(1));
        }
        let before = c.bytes_read;
        append(&f.0, b"activity:2\n");
        assert_eq!(c.read(&f.0, parse), Some(2));
        eprintln!(
            "file_bytes={} ticks=100 bytes_read={} append_bytes_read={} peak_buffer_bytes={} elapsed_ms={}",
            fs::metadata(&f.0).unwrap().len(),
            before,
            c.bytes_read - before,
            c.peak_buffer,
            start.elapsed().as_millis()
        );
        eprintln!(
            "process_peak_footprint_bytes={:?}",
            crate::footprint::peak_footprint(std::process::id())
        );
        assert_eq!(before, fs::metadata(&f.0).unwrap().len() - 11);
        assert!(c.peak_buffer <= 2 * MAX_LINE + BLOCK);
    }
}
