//! Read what agents leave on disk to learn when a session last did anything
//! real. Today: Claude Code, which keeps `~/.claude/sessions/<pid>.json` with
//! the session id and working directory, and appends every message to
//! `~/.claude/projects/<encoded cwd>/<session id>.jsonl`.
//!
//! Nothing here writes. Nothing here reads message contents beyond the
//! entry type and timestamp.

use crate::fmt::days_from_civil;
use crate::system::home;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ClaudeSession {
    pub session_id: String,
    pub name: Option<String>,
    pub transcript: Option<PathBuf>,
    /// Epoch seconds of the last user or assistant entry in the transcript.
    pub last_activity: Option<u64>,
}

fn claude_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    home().map(|h| h.join(".claude"))
}

/// Claude Code names project directories by replacing every character that
/// is not alphanumeric with a dash.
fn encode_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn claude_session(pid: u32) -> Option<ClaudeSession> {
    let base = claude_dir()?;
    let raw = fs::read(base.join("sessions").join(format!("{pid}.json"))).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let session_id = v.get("sessionId")?.as_str()?.to_string();
    let cwd = v.get("cwd").and_then(|c| c.as_str()).map(str::to_string);
    let name = v.get("name").and_then(|c| c.as_str()).map(str::to_string);

    let projects = base.join("projects");
    let mut transcript = cwd
        .as_deref()
        .map(|c| {
            projects
                .join(encode_cwd(c))
                .join(format!("{session_id}.jsonl"))
        })
        .filter(|p| p.is_file());
    if transcript.is_none() {
        transcript = find_transcript(&projects, &session_id);
    }
    let last_activity = transcript.as_deref().and_then(last_activity_in);
    Some(ClaudeSession {
        session_id,
        name,
        transcript,
        last_activity,
    })
}

fn find_transcript(projects: &Path, session_id: &str) -> Option<PathBuf> {
    let want = format!("{session_id}.jsonl");
    for e in fs::read_dir(projects).ok()?.flatten() {
        let p = e.path().join(&want);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Last user/assistant timestamp in a transcript, reading from the tail so a
/// multi-megabyte file costs a few hundred kilobytes at most. Entries with
/// other types (bookkeeping the host app writes) are ignored, which is the
/// whole point: the file's modification time is not an activity signal.
pub fn last_activity_in(path: &Path) -> Option<u64> {
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    for chunk in [256 * 1024u64, 4 * 1024 * 1024, u64::MAX] {
        let start = len.saturating_sub(chunk);
        f.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = Vec::with_capacity((len - start) as usize);
        f.read_to_end(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<&str> = text.lines().collect();
        if start > 0 && !lines.is_empty() {
            lines.remove(0); // partial line
        }
        for line in lines.iter().rev() {
            if let Some(ts) = activity_timestamp(line) {
                return Some(ts);
            }
        }
        if start == 0 {
            break;
        }
    }
    None
}

fn activity_timestamp(line: &str) -> Option<u64> {
    // Cheap pre-filter before parsing JSON.
    if !(line.contains("\"type\":\"user\"") || line.contains("\"type\":\"assistant\"")) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    match v.get("type").and_then(|t| t.as_str()) {
        Some("user") | Some("assistant") => {}
        _ => return None,
    }
    v.get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(parse_iso8601)
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` to epoch seconds. Only the UTC form the
/// transcripts use; anything else returns None.
pub fn parse_iso8601(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |from: usize, to: usize| s.get(from..to)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !s.ends_with('Z') {
        return None;
    }
    let days = days_from_civil(y, m as u32, d as u32);
    let secs = days * 86_400 + hh * 3_600 + mm * 60 + ss;
    u64::try_from(secs).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_roundtrip() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso8601("2026-09-05T03:00:27.284Z"),
            Some(1_788_577_227)
        );
        assert_eq!(parse_iso8601("2026-09-05 03:00:27Z"), None);
    }

    #[test]
    fn encodes_cwd_like_claude_code() {
        assert_eq!(
            encode_cwd("/Users/a/Library/Application Support/x.y"),
            "-Users-a-Library-Application-Support-x-y"
        );
    }

    #[test]
    fn picks_last_real_entry() {
        assert!(activity_timestamp(r#"{"type":"bridge-session","x":1}"#).is_none());
        assert_eq!(
            activity_timestamp(r#"{"type":"assistant","timestamp":"1970-01-02T00:00:00Z"}"#),
            Some(86_400)
        );
    }
}
