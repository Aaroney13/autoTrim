//! Private state files and bounded logs shared by every interface.

use anyhow::{Context, Result, bail};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const LOG_BYTES: u64 = 5 * 1024 * 1024;
pub const LOG_BACKUPS: usize = 3;

/// The override must be a dedicated autoTrim directory, not a shared folder.
pub fn private_dir(dir: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        bail!("data directory must be a real directory: {}", dir.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn options() -> OpenOptions {
    let mut opts = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    opts.read(true).write(true);
    opts
}

fn append_options() -> OpenOptions {
    let mut opts = options();
    // Windows append mode removes FILE_WRITE_DATA, so set_len fails during
    // rotation or rollback. Use write access there and seek to EOF under the
    // file lock. Keep atomic append on Unix for launchd's external log writer.
    opts.create(true).append(!cfg!(windows));
    opts
}

fn secure_file(file: &File) -> Result<()> {
    if !file.metadata()?.is_file() {
        bail!("state/log path must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if file.metadata()?.nlink() != 1 {
            bail!("refusing a state/log file with multiple hard links");
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Tighten files from earlier versions, including retained history and backups.
pub fn harden_existing(dir: &Path) -> Result<()> {
    private_dir(dir)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let owned = matches!(
            name.as_ref(),
            "latest.json" | "state.json" | "daemon.pid" | "config.toml" | "crash.json"
        ) || name.starts_with("history-") && name.ends_with(".jsonl")
            || ["daemon.log", "actions.jsonl"].iter().any(|base| {
                name == *base || (1..=LOG_BACKUPS).any(|i| name == format!("{base}.{i}"))
            })
            || matches!(
                name.as_ref(),
                "latest.tmp" | "state.tmp" | "daemon.tmp" | "config.toml.tmp"
            );
        if owned {
            let file = options().open(entry.path())?;
            secure_file(&file).with_context(|| format!("securing {}", entry.path().display()))?;
        }
    }
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().context("state path has no parent")?;
    private_dir(dir)?;
    // Unique, exclusive temporary files avoid following stale symlinks and
    // prevent concurrent config writers from sharing a temporary file.
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = options()
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    let result: Result<()> = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        #[cfg(unix)]
        File::open(dir)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp);
    result.with_context(|| format!("writing {}", path.display()))
}

fn backup(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    name.into()
}

pub(crate) fn rotation_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = fs::metadata(backup(path, 1)).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// All writers lock the current file, including across CLI/tray/daemon
/// processes. Copy/truncate preserves the inode launchd has open on upgrades.
pub fn append_log(path: &Path, bytes: &[u8]) -> Result<()> {
    append_bounded(path, bytes, LOG_BYTES, LOG_BACKUPS, false)
}

pub fn append_journal(path: &Path, bytes: &[u8]) -> Result<()> {
    append_bounded(path, bytes, LOG_BYTES, LOG_BACKUPS, true)
}

/// Panic reporting must never wait for a log lock held by the panicking thread.
pub(crate) fn append_diagnostic(path: &Path, bytes: &[u8]) -> Result<()> {
    append_with_lock(path, bytes, LOG_BYTES, LOG_BACKUPS, false, true)
}

fn append_bounded(
    path: &Path,
    bytes: &[u8],
    limit: u64,
    backups: usize,
    durable: bool,
) -> Result<()> {
    append_with_lock(path, bytes, limit, backups, durable, false)
}

fn append_with_lock(
    path: &Path,
    bytes: &[u8],
    limit: u64,
    backups: usize,
    durable: bool,
    nonblocking: bool,
) -> Result<()> {
    if bytes.len() as u64 > limit {
        bail!("log record exceeds {limit} bytes");
    }
    private_dir(path.parent().context("log path has no parent")?)?;
    let mut file = append_options().open(path)?;
    secure_file(&file)?;
    if nonblocking {
        file.try_lock()?;
    } else {
        file.lock()?;
    }
    if file.metadata()?.len().saturating_add(bytes.len() as u64) > limit {
        rotate(&mut file, path, limit, backups)?;
    }
    // Serialize before opening the file so an encoding error never leaves
    // half a JSON record behind. Roll back a short/failed write as well.
    let before = file.seek(SeekFrom::End(0))?;
    if let Err(error) = file.write_all(bytes) {
        let _ = file.set_len(before);
        return Err(error.into());
    }
    if durable {
        file.sync_all()?;
        #[cfg(unix)]
        File::open(path.parent().context("log path has no parent")?)?.sync_all()?;
    }
    Ok(())
}

fn rotate(file: &mut File, path: &Path, limit: u64, backups: usize) -> Result<()> {
    for i in (1..backups).rev() {
        match fs::rename(backup(path, i), backup(path, i + 1)) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    if backups > 0 {
        let len = file.metadata()?.len();
        let start = len.saturating_sub(limit);
        file.seek(SeekFrom::Start(start))?;
        let mut tail = Vec::new();
        Read::by_ref(file).take(limit).read_to_end(&mut tail)?;
        // An oversized legacy log only contributes complete tail records.
        let skip = if start > 0 {
            tail.iter()
                .position(|&b| b == b'\n')
                .map_or(tail.len(), |i| i + 1)
        } else {
            0
        };
        write_atomic(&backup(path, 1), &tail[skip..])?;
    }
    file.set_len(0)?;
    Ok(())
}

pub fn append_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    append_log(path, &bytes)
}

/// Daily history is age-retained by the daemon, rather than size-rotated.
pub fn append_history(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    private_dir(path.parent().context("history path has no parent")?)?;
    let mut file = append_options().open(path)?;
    secure_file(&file)?;
    file.lock()?;
    let before = file.seek(SeekFrom::End(0))?;
    if let Err(error) = file.write_all(&bytes) {
        let _ = file.set_len(before);
        return Err(error.into());
    }
    Ok(())
}

/// Read backwards in small blocks; parse only until `last` valid records
/// have been found. Memory is bounded even for a giant malformed legacy line.
pub fn tail_records<T>(
    path: &Path,
    last: usize,
    mut parse: impl FnMut(&str) -> Option<T>,
) -> Result<Vec<T>> {
    let mut result = Vec::new();
    if last == 0 {
        return Ok(result);
    }
    let current = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e.into()),
    };
    current.lock_shared()?;
    for index in 0..=LOG_BACKUPS {
        let mut file = if index == 0 {
            current.try_clone()?
        } else {
            match File::open(backup(path, index)) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            }
        };
        let mut pos = file.metadata()?.len();
        // A view must also be bounded before an oversized legacy log has
        // received its first write/rotation.
        let floor = pos.saturating_sub(LOG_BYTES);
        let mut line = Vec::new();
        let mut oversized = false;
        let mut block = [0u8; 8192];
        while pos > floor && result.len() < last {
            let n = (pos - floor).min(block.len() as u64) as usize;
            pos -= n as u64;
            file.seek(SeekFrom::Start(pos))?;
            file.read_exact(&mut block[..n])?;
            for &byte in block[..n].iter().rev() {
                if byte == b'\n' {
                    push_record(&mut line, oversized, &mut parse, &mut result);
                    oversized = false;
                    if result.len() == last {
                        break;
                    }
                } else if !oversized {
                    if line.len() as u64 == LOG_BYTES {
                        oversized = true;
                        line.clear();
                    } else {
                        line.push(byte);
                    }
                }
            }
        }
        if result.len() < last && floor == 0 {
            push_record(&mut line, oversized, &mut parse, &mut result);
        }
        if result.len() == last {
            break;
        }
    }
    result.reverse();
    Ok(result)
}

fn push_record<T>(
    line: &mut Vec<u8>,
    oversized: bool,
    parse: &mut impl FnMut(&str) -> Option<T>,
    result: &mut Vec<T>,
) {
    if !oversized && !line.is_empty() {
        line.reverse();
        if let Ok(text) = std::str::from_utf8(line)
            && let Some(record) = parse(text.trim_end_matches('\r'))
        {
            result.push(record);
        }
    }
    line.clear();
}

pub fn tail_lines(path: &Path, last: usize) -> Result<Vec<String>> {
    tail_records(path, last, |s| Some(s.to_string()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Scratch(pub PathBuf);
    impl Scratch {
        pub(crate) fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "autotrim-storage-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            private_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rotation_bounds_disk_use_and_reads_across_backups() {
        let dir = Scratch::new();
        let path = dir.0.join("actions.jsonl");
        for i in 0..21 {
            append_bounded(&path, format!("{i:02}\n").as_bytes(), 6, 3, false).unwrap();
        }
        assert_eq!(
            tail_lines(&path, 100).unwrap(),
            (14..21).map(|i| format!("{i:02}")).collect::<Vec<_>>()
        );
        assert_eq!(tail_lines(&path, 3).unwrap(), ["18", "19", "20"]);
        assert!(tail_lines(&path, 0).unwrap().is_empty());
        for entry in fs::read_dir(&dir.0).unwrap() {
            assert!(entry.unwrap().metadata().unwrap().len() <= 6);
        }
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 4);
    }

    #[test]
    fn rotation_preserves_an_open_launchd_style_writer() {
        let dir = Scratch::new();
        let path = dir.0.join("daemon.log");
        append_bounded(&path, b"old\nold\n", 10, 3, false).unwrap();
        let mut held = OpenOptions::new().append(true).open(&path).unwrap();
        append_bounded(&path, b"next\n", 10, 3, false).unwrap();
        held.write_all(b"external\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "next\nexternal\n");
        assert_eq!(fs::read_to_string(backup(&path, 1)).unwrap(), "old\nold\n");
    }

    #[test]
    fn crash_logging_does_not_wait_for_an_already_held_log_lock() {
        let dir = Scratch::new();
        let path = dir.0.join("daemon.log");
        append_log(&path, b"existing\n").unwrap();
        let held = options().open(&path).unwrap();
        held.lock().unwrap();
        assert!(append_diagnostic(&path, b"panic\n").is_err());
        drop(held);
        append_diagnostic(&path, b"panic\n").unwrap();
        assert_eq!(tail_lines(&path, 1).unwrap(), ["panic"]);
    }

    #[test]
    fn oversized_legacy_logs_keep_only_a_bounded_complete_tail() {
        let dir = Scratch::new();
        let path = dir.0.join("daemon.log");
        fs::write(&path, "old record\n".repeat(10000)).unwrap();
        append_bounded(&path, b"new\n", 64, 3, false).unwrap();
        let archive = fs::read_to_string(backup(&path, 1)).unwrap();
        assert!(archive.len() <= 64);
        assert!(archive.lines().all(|line| line == "old record"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
    }

    #[test]
    fn concurrent_writers_do_not_interleave_or_lose_records() {
        let dir = Scratch::new();
        let path = dir.0.join("actions.jsonl");
        std::thread::scope(|scope| {
            for worker in 0..4 {
                let path = &path;
                scope.spawn(move || {
                    for i in 0..20 {
                        append_json(path, &serde_json::json!({"worker": worker, "i": i})).unwrap();
                    }
                });
            }
        });
        let records = tail_records(&path, 100, |s| {
            serde_json::from_str::<serde_json::Value>(s).ok()
        })
        .unwrap();
        assert_eq!(records.len(), 80);
        let distinct: std::collections::HashSet<_> = records
            .iter()
            .map(|v| (v["worker"].as_u64().unwrap(), v["i"].as_u64().unwrap()))
            .collect();
        assert_eq!(distinct.len(), 80);
    }

    #[test]
    fn history_appends_preserve_existing_records() {
        let dir = Scratch::new();
        let path = dir.0.join("history-2026-09-14.jsonl");
        fs::write(&path, b"\"existing\"\n").unwrap();
        append_history(&path, &"second").unwrap();
        append_history(&path, &"third").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\"existing\"\n\"second\"\n\"third\"\n"
        );
    }

    #[test]
    fn tails_handle_utf8_block_boundaries_and_skip_invalid_json() {
        let dir = Scratch::new();
        let path = dir.0.join("actions.jsonl");
        let long = "é".repeat(9000);
        fs::write(
            &path,
            format!(
                "\"first\"\n{}\ninvalid\n\"last\"",
                serde_json::to_string(&long).unwrap()
            ),
        )
        .unwrap();
        let records = tail_records(&path, 2, |s| serde_json::from_str::<String>(s).ok()).unwrap();
        assert_eq!(records, [long, "last".into()]);
        let mut calls = 0;
        tail_records(&path, 1, |s| {
            calls += 1;
            Some(s.to_string())
        })
        .unwrap();
        assert_eq!(calls, 1, "only requested records should be parsed");
    }

    #[test]
    fn legacy_log_views_stop_at_the_byte_budget_even_with_too_few_records() {
        let dir = Scratch::new();
        let path = dir.0.join("actions.jsonl");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"\"before\"\n").unwrap();
        file.set_len(LOG_BYTES * 2).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(b"\n\"after\"\n").unwrap();
        drop(file);
        let records = tail_records(&path, 10, |s| serde_json::from_str::<String>(s).ok()).unwrap();
        assert_eq!(records, ["after"]);
    }

    #[test]
    fn failed_atomic_replace_leaves_no_temporary_files() {
        let dir = Scratch::new();
        let path = dir.0.join("latest.json");
        fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, b"new").is_err());
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn private_modes_apply_to_new_files_and_existing_installations() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new();
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
        for name in [
            "latest.json",
            "state.json",
            "config.toml",
            "actions.jsonl.1",
            "history-2026-09-01.jsonl",
        ] {
            fs::write(dir.0.join(name), b"old").unwrap();
            fs::set_permissions(dir.0.join(name), fs::Permissions::from_mode(0o644)).unwrap();
        }
        harden_existing(&dir.0).unwrap();
        write_atomic(&dir.0.join("latest.json"), b"private prompt").unwrap();
        append_json(&dir.0.join("actions.jsonl"), &"private URL").unwrap();
        append_history(&dir.0.join("history-2026-09-02.jsonl"), &"private project").unwrap();
        assert_eq!(
            fs::metadata(&dir.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in fs::read_dir(&dir.0).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn log_writes_refuse_symlinks_and_hard_links() {
        let dir = Scratch::new();
        let target = dir.0.join("unrelated");
        fs::write(&target, b"leave alone").unwrap();
        let path = dir.0.join("daemon.log");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(append_log(&path, b"sensitive").is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&target, &path).unwrap();
        assert!(append_log(&path, b"sensitive").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"leave alone");
    }
}
