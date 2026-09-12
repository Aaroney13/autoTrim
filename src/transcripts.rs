//! Read what agents leave on disk to learn when a session last did anything
//! real, and what to call it. Today: Claude Code, which keeps
//! `~/.claude/sessions/<pid>.json` with the session id and working
//! directory, and appends every message to
//! `~/.claude/projects/<encoded cwd>/<session id>.jsonl`; and Codex, which
//! keeps one rollout file per thread under `~/.codex/sessions`.
//!
//! Nothing here writes. Beyond entry types and timestamps, the only message
//! content read is the first thing the user typed, shortened to one line,
//! so a session can be named by what it was for.

use crate::fmt::days_from_civil;
use crate::openfiles::open_files;
use crate::system::home;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone)]
pub struct ClaudeSession {
    pub session_id: String,
    /// The name in the session file, and where it came from ("derived"
    /// means Claude Code made it up from the directory).
    pub name: Option<String>,
    pub name_source: Option<String>,
    pub transcript: Option<PathBuf>,
    /// Epoch seconds of the last user or assistant entry in the transcript.
    pub last_activity: Option<u64>,
    /// A title the user gave the session, if the transcript records one.
    pub title: Option<String>,
    /// The first thing the user asked, shortened.
    pub first_prompt: Option<String>,
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
    let name_source = v
        .get("nameSource")
        .and_then(|c| c.as_str())
        .map(str::to_string);

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
    let (title, first_prompt) = transcript
        .as_deref()
        .map(|p| names_in(p, claude_title, claude_prompt))
        .unwrap_or((None, None));
    Some(ClaudeSession {
        session_id,
        name,
        name_source,
        transcript,
        last_activity,
        title,
        first_prompt,
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

#[derive(Debug, Clone)]
pub struct CodexSession {
    /// Rollout files the process currently holds open: its live threads.
    pub threads: usize,
    /// The most recently active of those rollouts.
    pub transcript: Option<PathBuf>,
    /// Working directory recorded in that rollout's metadata.
    pub cwd: Option<String>,
    /// Newest response or event timestamp across the open rollouts.
    pub last_activity: Option<u64>,
    /// The first thing the user asked in the newest thread, shortened.
    pub first_prompt: Option<String>,
}

/// Codex keeps one rollout file per thread under `~/.codex/sessions` and
/// holds the live ones open, so the process's own file table is the map.
/// Returns None when the process has no rollout open; the caller falls back
/// to CPU evidence.
pub fn codex_session(pid: u32) -> Option<CodexSession> {
    let rollouts: Vec<PathBuf> = open_files(pid)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .map(|f| {
                    let f = f.to_string_lossy();
                    f.starts_with("rollout-") && f.ends_with(".jsonl")
                })
                .unwrap_or(false)
        })
        .collect();
    if rollouts.is_empty() {
        return None;
    }
    let mut best: Option<(u64, PathBuf)> = None;
    for r in &rollouts {
        if let Some(ts) = last_activity_with(r, codex_activity_timestamp)
            && best.as_ref().is_none_or(|(b, _)| ts > *b)
        {
            best = Some((ts, r.clone()));
        }
    }
    let transcript = best
        .as_ref()
        .map(|(_, p)| p.clone())
        .or_else(|| rollouts.first().cloned());
    let cwd = transcript.as_deref().and_then(codex_meta_cwd);
    let first_prompt = transcript
        .as_deref()
        .and_then(|p| names_in(p, |_| None, codex_prompt).1);
    Some(CodexSession {
        threads: rollouts.len(),
        transcript,
        cwd,
        last_activity: best.map(|(ts, _)| ts),
        first_prompt,
    })
}

/// The `cwd` field of a rollout's first line, its session metadata.
fn codex_meta_cwd(path: &Path) -> Option<String> {
    let f = fs::File::open(path).ok()?;
    let first = BufReader::new(f).lines().next()?.ok()?;
    let v: serde_json::Value = serde_json::from_str(&first).ok()?;
    v.get("payload")?.get("cwd")?.as_str().map(str::to_string)
}

fn codex_activity_timestamp(line: &str) -> Option<u64> {
    if !(line.contains("\"type\":\"response_item\"") || line.contains("\"type\":\"event_msg\"")) {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    match v.get("type").and_then(|t| t.as_str()) {
        Some("response_item") | Some("event_msg") => {}
        _ => return None,
    }
    v.get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(parse_iso8601)
}

/// Last user/assistant timestamp in a transcript, reading from the tail so a
/// multi-megabyte file costs a few hundred kilobytes at most. Entries with
/// other types (bookkeeping the host app writes) are ignored, which is the
/// whole point: the file's modification time is not an activity signal.
pub fn last_activity_in(path: &Path) -> Option<u64> {
    last_activity_with(path, activity_timestamp)
}

fn last_activity_with(path: &Path, pick: fn(&str) -> Option<u64>) -> Option<u64> {
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
            if let Some(ts) = pick(line) {
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

/// What a transcript says a session is called. Transcripts are append-only,
/// so each file is read once in full and then only what was appended since,
/// which keeps a daemon tick to a stat per session.
#[derive(Default, Clone)]
struct Names {
    /// Bytes scanned so far, always at a line boundary.
    scanned_to: u64,
    title: Option<String>,
    first_prompt: Option<String>,
}

static NAMES: LazyLock<Mutex<HashMap<PathBuf, Names>>> = LazyLock::new(Default::default);

/// (title, first prompt) for a transcript, using `title_of` and `prompt_of`
/// to read one line each. The title is the last one written; the prompt is
/// the first one found.
fn names_in(
    path: &Path,
    title_of: fn(&str) -> Option<String>,
    prompt_of: fn(&str) -> Option<String>,
) -> (Option<String>, Option<String>) {
    let mut cache = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let entry = cache.entry(path.to_path_buf()).or_default();
    let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len < entry.scanned_to {
        *entry = Names::default();
    }
    if len > entry.scanned_to
        && let Ok(mut f) = fs::File::open(path)
        && f.seek(SeekFrom::Start(entry.scanned_to)).is_ok()
    {
        let mut r = BufReader::with_capacity(64 * 1024, f);
        let mut line = String::new();
        loop {
            line.clear();
            let n = match r.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if !line.ends_with('\n') {
                break; // still being written; pick it up next time
            }
            entry.scanned_to += n as u64;
            if let Some(t) = title_of(&line) {
                entry.title = Some(t);
            }
            if entry.first_prompt.is_none() {
                entry.first_prompt = prompt_of(&line);
            }
        }
    }
    (entry.title.clone(), entry.first_prompt.clone())
}

/// Drop the host's own `<system-reminder>` blocks from a prompt.
fn strip_reminders(s: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        match rest[start..].find(CLOSE) {
            Some(end) => rest = &rest[start + end + CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let keep: String = s.chars().take(n - 1).collect();
    format!("{}…", keep.trim_end())
}

/// One line of at most 80 characters, or None when what is left is not
/// something a person typed (tool output, slash-command wrappers).
fn tidy_prompt(s: &str) -> Option<String> {
    let s = strip_reminders(s);
    let s = s.trim();
    if s.is_empty() || s.starts_with('<') {
        return None;
    }
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(truncate_chars(&one_line, 80))
}

fn claude_title(line: &str) -> Option<String> {
    if !line.contains("\"custom-title\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "custom-title" {
        return None;
    }
    let t = v.get("customTitle")?.as_str()?.trim();
    (!t.is_empty()).then(|| truncate_chars(t, 80))
}

fn claude_prompt(line: &str) -> Option<String> {
    if !line.contains("\"type\":\"user\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "user" || v.get("isMeta").and_then(|m| m.as_bool()) == Some(true)
    {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    tidy_prompt(&text)
}

fn codex_prompt(line: &str) -> Option<String> {
    if !line.contains("\"role\":\"user\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "response_item" {
        return None;
    }
    let payload = v.get("payload")?;
    if payload.get("type")?.as_str()? != "message" || payload.get("role")?.as_str()? != "user" {
        return None;
    }
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("input_text"))
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        // Codex prepends the repo's AGENTS.md and an environment block as
        // user messages of their own; neither is the person.
        .filter(|t| !t.starts_with("# AGENTS.md") && !t.trim_start().starts_with('<'))
        .collect::<Vec<_>>()
        .join(" ");
    tidy_prompt(&text)
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
    fn codex_entries() {
        assert!(
            codex_activity_timestamp(
                r#"{"type":"session_meta","timestamp":"1970-01-02T00:00:00Z"}"#
            )
            .is_none()
        );
        assert_eq!(
            codex_activity_timestamp(
                r#"{"type":"response_item","timestamp":"1970-01-02T00:00:00Z","payload":{}}"#
            ),
            Some(86_400)
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

    #[test]
    fn names_from_claude_lines() {
        assert_eq!(
            claude_title(
                r#"{"type":"custom-title","customTitle":"  Dashboard restyle ","sessionId":"x"}"#
            ),
            Some("Dashboard restyle".to_string())
        );
        assert_eq!(
            claude_prompt(
                r#"{"type":"user","message":{"role":"user","content":"<system-reminder>\nnoise\n</system-reminder>\nfix the   login\nbug"}}"#
            ),
            Some("fix the login bug".to_string())
        );
        // Tool results and slash-command wrappers are not a person typing.
        assert!(
            claude_prompt(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#
            )
            .is_none()
        );
        assert!(
            claude_prompt(
                r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#
            )
            .is_none()
        );
        let long = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"{}"}}}}"#,
            "word ".repeat(40)
        );
        let got = claude_prompt(&long).unwrap();
        assert!(got.ends_with('…'));
        assert!(got.chars().count() <= 80);
    }

    #[test]
    fn names_from_codex_lines() {
        assert!(
            codex_prompt(
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /x\n\n<INSTRUCTIONS>"}]}}"##
            )
            .is_none()
        );
        assert_eq!(
            codex_prompt(
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"reduce duplication\n"}]}}"#
            ),
            Some("reduce duplication".to_string())
        );
    }

    #[test]
    fn scans_incrementally() {
        let dir = std::env::temp_dir().join(format!("autotrim-names-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"first ask\"}}\n",
        )
        .unwrap();
        assert_eq!(
            names_in(&path, claude_title, claude_prompt),
            (None, Some("first ask".to_string()))
        );
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        f.write_all(b"{\"type\":\"custom-title\",\"customTitle\":\"Named\"}\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"second\"}}\n{\"partial")
            .unwrap();
        assert_eq!(
            names_in(&path, claude_title, claude_prompt),
            (Some("Named".to_string()), Some("first ask".to_string()))
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
