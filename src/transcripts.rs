//! Read what agents leave on disk to learn when a session last did anything
//! real, and what to call it.
//!
//! Claude Code keeps `~/.claude/sessions/<pid>.json` with the session id and
//! working directory, and appends every message to
//! `~/.claude/projects/<encoded cwd>/<session id>.jsonl`. The others are
//! found through the process's own file table (`openfiles`), so nothing is
//! guessed: Codex holds one rollout file per thread open under
//! `~/.codex/sessions`, Copilot CLI one `events.jsonl` per session under
//! `~/.copilot/session-state`, and Cursor's CLI the per-chat SQLite store
//! under `~/.cursor/chats`.
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
use std::time::UNIX_EPOCH;

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
    let names = transcript
        .as_deref()
        .map(|p| names_in(p, &CLAUDE_LINES))
        .unwrap_or_default();
    Some(ClaudeSession {
        session_id,
        name,
        name_source,
        transcript,
        last_activity,
        title: names.title,
        first_prompt: names.first_prompt,
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

/// The threads an agent process is serving, read from the transcripts it
/// holds open. One process can serve several: an app's engine does.
#[derive(Debug, Clone, Default)]
pub struct Threads {
    /// Live threads a person opened, not counting helpers the agent started
    /// for itself (a Codex guardian review, a sub-agent).
    pub count: usize,
    /// The most recently active thread's transcript. What a close action
    /// logs next to the resume command.
    pub transcript: Option<PathBuf>,
    /// That thread's id, the thing a resume command takes.
    pub session_id: Option<String>,
    /// Working directory recorded for that thread, when the agent keeps one.
    pub cwd: Option<String>,
    /// Newest real activity across every open transcript.
    pub last_activity: Option<u64>,
    /// The agent's own name for the thread, when it keeps one.
    pub name: Option<String>,
    /// The first thing the user asked in that thread, shortened.
    pub first_prompt: Option<String>,
}

/// Files matching `keep` that the process tree holds open, from the first
/// process (root first) that holds any. The root usually does; the loop is
/// for agents that hand their transcript to a worker.
fn open_in_tree(pids: &[u32], keep: fn(&Path) -> bool) -> Vec<PathBuf> {
    for pid in pids {
        let mut found: Vec<PathBuf> = open_files(*pid).into_iter().filter(|p| keep(p)).collect();
        if !found.is_empty() {
            found.sort();
            found.dedup();
            return found;
        }
    }
    Vec::new()
}

/// The session directory an open file belongs to: the directory `depth`
/// levels below the nearest ancestor called `marker`. So
/// `session-state/<id>/events.jsonl` at depth 1 gives `session-state/<id>`,
/// and `chats/<hash>/<id>/store.db-wal` at depth 2 gives `chats/<hash>/<id>`.
fn session_dir_below(p: &Path, marker: &str, depth: usize) -> Option<PathBuf> {
    let ancestors: Vec<&Path> = p.ancestors().collect();
    let k = ancestors
        .iter()
        .position(|a| a.file_name().is_some_and(|f| f == marker))?;
    Some(ancestors[k.checked_sub(depth)?].to_path_buf())
}

/// Session directories the process tree holds any file open in, from the
/// first process (root first) that holds one. The transcript, an in-use
/// lock, a database and its write-ahead log all count: what matters is
/// that the process has the session open, not which file it is holding.
fn open_session_dirs(pids: &[u32], marker: &str, depth: usize) -> Vec<PathBuf> {
    for pid in pids {
        let mut found: Vec<PathBuf> = open_files(*pid)
            .into_iter()
            .filter_map(|p| session_dir_below(&p, marker, depth))
            .collect();
        if !found.is_empty() {
            found.sort();
            found.dedup();
            return found;
        }
    }
    Vec::new()
}

fn mtime_secs(m: &fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mtime_of(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().map(|m| mtime_secs(&m))
}

// ---------------------------------------------------------------- Codex

#[derive(Debug, Clone, Default)]
struct CodexMeta {
    id: Option<String>,
    cwd: Option<String>,
    /// A thread Codex started for itself (a guardian review, a sub-agent)
    /// rather than one a person opened.
    helper: bool,
}

static CODEX_META: LazyLock<Mutex<HashMap<PathBuf, CodexMeta>>> = LazyLock::new(Default::default);

/// A rollout's first line is its session metadata and never changes, so it
/// is read once per file.
fn codex_meta(path: &Path) -> CodexMeta {
    let mut cache = CODEX_META.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = cache.get(path) {
        return m.clone();
    }
    let meta = fs::File::open(path)
        .ok()
        .and_then(|f| BufReader::new(f).lines().next()?.ok())
        .and_then(|l| codex_meta_line(&l))
        .unwrap_or_default();
    if meta.id.is_some() {
        cache.insert(path.to_path_buf(), meta.clone());
    }
    meta
}

fn codex_meta_line(line: &str) -> Option<CodexMeta> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let p = v.get("payload")?;
    // A person's thread has a plain `source` ("cli", "vscode", "exec"); a
    // helper's is an object naming the sub-agent, and it has a parent.
    let helper = p.get("parent_thread_id").is_some_and(|x| x.is_string())
        || p.get("source").is_some_and(|s| s.is_object());
    Some(CodexMeta {
        id: p.get("id").and_then(|x| x.as_str()).map(str::to_string),
        cwd: p.get("cwd").and_then(|x| x.as_str()).map(str::to_string),
        helper,
    })
}

/// `rollout-<timestamp>-<uuid>.jsonl`: the uuid is the thread id.
pub fn rollout_uuid(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    Some(id.to_string())
}

/// Codex keeps one rollout file per thread under `~/.codex/sessions` and
/// holds the live ones open. Returns None when the process has no rollout
/// open; the caller falls back to CPU evidence, or drops an app server
/// with nothing loaded.
pub fn codex_threads(pids: &[u32]) -> Option<Threads> {
    let rollouts = open_in_tree(pids, |p| {
        p.file_name()
            .map(|f| {
                let f = f.to_string_lossy();
                f.starts_with("rollout-") && f.ends_with(".jsonl")
            })
            .unwrap_or(false)
    });
    if rollouts.is_empty() {
        return None;
    }
    let scanned: Vec<(Option<u64>, &PathBuf, bool)> = rollouts
        .iter()
        .map(|r| {
            (
                last_activity_with(r, codex_activity_timestamp),
                r,
                codex_meta(r).helper,
            )
        })
        .collect();
    // Helpers still count as activity (a review running means the thread is
    // busy) but not as threads. Only helpers open would mean their thread
    // just closed; count them rather than nothing.
    let mut own: Vec<&(Option<u64>, &PathBuf, bool)> = scanned.iter().filter(|s| !s.2).collect();
    if own.is_empty() {
        own = scanned.iter().collect();
    }
    let newest = own.iter().max_by_key(|s| s.0)?;
    let transcript = newest.1.clone();
    let meta = codex_meta(&transcript);
    let id = meta.id.clone().or_else(|| rollout_uuid(&transcript));
    Some(Threads {
        count: own.len(),
        last_activity: scanned.iter().filter_map(|s| s.0).max(),
        name: id
            .as_deref()
            .and_then(|i| codex_thread_name(&transcript, i)),
        first_prompt: names_in(&transcript, &CODEX_LINES).first_prompt,
        cwd: meta.cwd,
        session_id: id,
        transcript: Some(transcript),
    })
}

type IndexCache = HashMap<PathBuf, ((u64, u64), HashMap<String, String>)>;
static CODEX_INDEX: LazyLock<Mutex<IndexCache>> = LazyLock::new(Default::default);

/// Codex names every thread after its first exchange and records the name
/// in `session_index.jsonl` beside the sessions directory, wherever that
/// lives (`CODEX_HOME` moves it). Re-read only when the file changes.
fn codex_thread_name(rollout: &Path, id: &str) -> Option<String> {
    let home = rollout
        .ancestors()
        .find(|a| a.file_name().is_some_and(|f| f == "sessions"))?
        .parent()?;
    let index = home.join("session_index.jsonl");
    let meta = fs::metadata(&index).ok()?;
    let sig = (meta.len(), mtime_secs(&meta));
    let mut cache = CODEX_INDEX.lock().unwrap_or_else(|e| e.into_inner());
    let entry = cache.entry(index.clone()).or_default();
    if entry.0 != sig {
        entry.1 = fs::read_to_string(&index)
            .ok()?
            .lines()
            .filter_map(codex_index_line)
            .collect();
        entry.0 = sig;
    }
    entry.1.get(id).cloned()
}

fn codex_index_line(line: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let id = v.get("id")?.as_str()?.to_string();
    let name = v.get("thread_name")?.as_str()?.trim();
    (!name.is_empty()).then(|| (id, truncate_chars(name, 80)))
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

// -------------------------------------------------------------- Copilot

/// Copilot CLI keeps one directory per session at `session-state/<id>`
/// under `~/.copilot` (or `COPILOT_HOME`): `events.jsonl` for the
/// transcript, `workspace.yaml` for the session's name, and an in-use lock
/// while a process has it open. VS Code runs the same CLI as its engine,
/// so one process may serve several.
pub fn copilot_threads(pids: &[u32]) -> Option<Threads> {
    let dirs: Vec<PathBuf> = open_session_dirs(pids, "session-state", 1)
        .into_iter()
        .filter(|d| d.join("events.jsonl").is_file() || d.join("workspace.yaml").is_file())
        .collect();
    if dirs.is_empty() {
        return None;
    }
    let scanned: Vec<(Option<u64>, &PathBuf, Option<PathBuf>)> = dirs
        .iter()
        .map(|d| {
            let log = Some(d.join("events.jsonl")).filter(|l| l.is_file());
            let ts = log
                .as_deref()
                .and_then(|l| last_activity_with(l, copilot_activity_timestamp));
            (ts, d, log)
        })
        .collect();
    let newest = scanned.iter().max_by_key(|s| s.0)?;
    let dir = newest.1;
    let yaml = fs::read_to_string(dir.join("workspace.yaml")).unwrap_or_default();
    let names = newest
        .2
        .as_deref()
        .map(|l| names_in(l, &COPILOT_LINES))
        .unwrap_or_default();
    Some(Threads {
        count: dirs.len(),
        last_activity: scanned.iter().filter_map(|s| s.0).max(),
        name: yaml_top_level(&yaml, "name"),
        cwd: yaml_top_level(&yaml, "cwd").or(names.cwd),
        first_prompt: names.first_prompt,
        session_id: dir.file_name().map(|f| f.to_string_lossy().into_owned()),
        transcript: newest.2.clone().or_else(|| Some(dir.clone())),
    })
}

/// User turns, assistant turns, and tool runs are activity; session
/// bookkeeping (`session.start`, model changes, shutdown) is not.
fn copilot_activity_timestamp(line: &str) -> Option<u64> {
    if !(line.contains("\"type\":\"user.")
        || line.contains("\"type\":\"assistant.")
        || line.contains("\"type\":\"tool."))
    {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let t = v.get("type")?.as_str()?;
    if !(t.starts_with("user.") || t.starts_with("assistant.") || t.starts_with("tool.")) {
        return None;
    }
    timestamp_of(v.get("timestamp")?)
}

fn copilot_prompt(line: &str) -> Option<String> {
    if !line.contains("\"type\":\"user.message\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "user.message" {
        return None;
    }
    tidy_prompt(v.get("data")?.get("content")?.as_str()?)
}

/// The working directory, from the session start when it records one, else
/// from the folder-trust notice ("Folder /x has been added to trusted
/// folders.").
fn copilot_cwd(line: &str) -> Option<String> {
    if !line.contains("\"type\":\"session.") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let d = v.get("data")?;
    match v.get("type")?.as_str()? {
        "session.start" => d
            .get("cwd")
            .or_else(|| d.get("context").and_then(|c| c.get("cwd")))
            .and_then(|c| c.as_str())
            .map(str::to_string),
        "session.info" => {
            if d.get("infoType")?.as_str()? != "folder_trust" {
                return None;
            }
            let m = d.get("message")?.as_str()?;
            let rest = m.split_once("Folder ")?.1;
            let path = rest.split(" has been").next()?.trim();
            (!path.is_empty()).then(|| path.to_string())
        }
        _ => None,
    }
}

/// The value of a top-level `key:` line in a small YAML file: plain,
/// quoted, or a block scalar (`key: |-` with the text indented below),
/// which is how Copilot writes session names.
fn yaml_top_level(text: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let i = lines
        .iter()
        .position(|l| l.strip_prefix(key).is_some_and(|r| r.starts_with(':')))?;
    let value = lines[i][key.len() + 1..].trim();
    let value = match value {
        "|" | "|-" | "|+" | ">" | ">-" | ">+" => lines[i + 1..]
            .iter()
            .take_while(|l| l.starts_with([' ', '\t']) || l.trim().is_empty())
            .map(|l| l.trim())
            .find(|l| !l.is_empty())?
            .to_string(),
        v if v.len() >= 2
            && ((v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\''))) =>
        {
            v[1..v.len() - 1].to_string()
        }
        v => v.to_string(),
    };
    let value = value.trim();
    (!value.is_empty()).then(|| truncate_chars(value, 80))
}

// --------------------------------------------------------------- Cursor

/// Cursor's CLI keeps one SQLite store per chat at
/// `~/.cursor/chats/<hash of cwd>/<id>/store.db` and holds it (and its
/// write-ahead log) open, and writes a plain transcript under
/// `~/.cursor/projects`. The transcript has no timestamps and only ever
/// gains messages, so its modification time is the idle signal.
pub fn cursor_threads(pids: &[u32], cwd: Option<&str>) -> Option<Threads> {
    let dirs: Vec<PathBuf> = open_session_dirs(pids, "chats", 2)
        .into_iter()
        .filter(|d| d.join("store.db").is_file())
        .collect();
    if dirs.is_empty() {
        return None;
    }
    let mut newest: Option<(Option<u64>, PathBuf, Option<PathBuf>)> = None;
    let mut last_activity = None;
    for d in &dirs {
        let Some(id) = d.file_name() else {
            continue;
        };
        // <root>/chats/<hash>/<id>
        let Some(root) = d.ancestors().nth(3) else {
            continue;
        };
        let transcript = cursor_transcript(root, cwd, &id.to_string_lossy());
        let ts = transcript.as_deref().and_then(mtime_of);
        last_activity = last_activity.max(ts);
        if newest.as_ref().is_none_or(|n| ts >= n.0) {
            newest = Some((ts, d.clone(), transcript));
        }
    }
    let (_, dir, transcript) = newest?;
    let id = dir.file_name()?.to_string_lossy().into_owned();
    let store = dir.join("store.db");
    Some(Threads {
        count: dirs.len(),
        last_activity,
        name: cursor_chat_name(&store, &id),
        first_prompt: transcript
            .as_deref()
            .and_then(|t| names_in(t, &CURSOR_LINES).first_prompt),
        cwd: None,
        session_id: Some(id),
        transcript: transcript.or(Some(store)),
    })
}

/// Cursor files transcripts under the project path with `/` as `-` and no
/// leading separator: `/Users/a/code/x` becomes `Users-a-code-x`.
fn cursor_project_dir(cwd: &str) -> String {
    cwd.trim_start_matches(['/', '\\'])
        .replace(['/', '\\'], "-")
}

/// `<root>/projects/<project>/agent-transcripts/<id>/<id>.jsonl`, or the
/// older flat `agent-transcripts/<id>.jsonl`. Tried under the process's own
/// directory first, then wherever the id turns up.
fn cursor_transcript(root: &Path, cwd: Option<&str>, id: &str) -> Option<PathBuf> {
    let projects = root.join("projects");
    let file = format!("{id}.jsonl");
    let in_project = |dir: &Path| -> Option<PathBuf> {
        let t = dir.join("agent-transcripts");
        [t.join(id).join(&file), t.join(&file)]
            .into_iter()
            .find(|p| p.is_file())
    };
    if let Some(cwd) = cwd
        && let Some(p) = in_project(&projects.join(cursor_project_dir(cwd)))
    {
        return Some(p);
    }
    fs::read_dir(&projects)
        .ok()?
        .flatten()
        .find_map(|e| in_project(&e.path()))
}

fn cursor_prompt(line: &str) -> Option<String> {
    if !line.contains("\"role\":\"user\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("role")?.as_str()? != "user" {
        return None;
    }
    let text = v
        .get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    tidy_prompt(
        &text
            .replace("<user_query>", " ")
            .replace("</user_query>", " "),
    )
}

type ChatNameCache = HashMap<PathBuf, ((u64, u64, u64, u64), Option<String>)>;
static CURSOR_NAMES: LazyLock<Mutex<ChatNameCache>> = LazyLock::new(Default::default);

/// The chat's own name from its SQLite store, read without SQLite: the
/// `meta` row is one small JSON document, hex-encoded, and the newest copy
/// of its page is at the end of the write-ahead log or in the main file.
/// The name is only accepted beside an `agentId` matching the chat's
/// directory. Re-read only when either file changes.
fn cursor_chat_name(store: &Path, id: &str) -> Option<String> {
    let wal = store.with_extension("db-wal");
    let sig_of = |p: &Path| {
        fs::metadata(p)
            .map(|m| (m.len(), mtime_secs(&m)))
            .unwrap_or((0, 0))
    };
    let (a, b) = (sig_of(store), sig_of(&wal));
    let sig = (a.0, a.1, b.0, b.1);
    let mut cache = CURSOR_NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((s, n)) = cache.get(store)
        && *s == sig
    {
        return n.clone();
    }
    let name = read_tail(&wal, 4 << 20)
        .and_then(|b| meta_name_in(&b, id))
        .or_else(|| read_head(store, 256 << 10).and_then(|b| meta_name_in(&b, id)));
    cache.insert(store.to_path_buf(), (sig, name.clone()));
    name
}

fn read_head(p: &Path, max: u64) -> Option<Vec<u8>> {
    let f = fs::File::open(p).ok()?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn read_tail(p: &Path, max: u64) -> Option<Vec<u8>> {
    let mut f = fs::File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// The `name` of the JSON document holding `"agentId":"<id>"` somewhere in
/// `bytes`, whether the document is stored as text or hex-encoded. The last
/// copy wins, which in a write-ahead log is the newest.
fn meta_name_in(bytes: &[u8], id: &str) -> Option<String> {
    let anchor = format!("\"agentId\":\"{id}\"");
    let key = "\"name\":\"";
    for hex in [true, false] {
        let (anchor, key) = if hex {
            (hex_of(&anchor), hex_of(key))
        } else {
            (anchor.clone().into_bytes(), key.as_bytes().to_vec())
        };
        let Some(at) = find_last(bytes, &anchor) else {
            continue;
        };
        // The document is a few hundred bytes; look around the anchor only,
        // and in hex only at pair-aligned offsets.
        let lo = at.saturating_sub(4096);
        let hi = (at + 4096).min(bytes.len());
        let window = &bytes[lo..hi];
        let parity = (at - lo) % 2;
        let Some(k) = find_last_where(window, &key, |i| !hex || i % 2 == parity) else {
            continue;
        };
        let Some(raw) = json_string_at(window, k + key.len(), hex) else {
            continue;
        };
        let Ok(s) = serde_json::from_str::<String>(&format!("\"{raw}\"")) else {
            continue;
        };
        let s = s.trim();
        if !s.is_empty() {
            return Some(truncate_chars(s, 80));
        }
    }
    None
}

fn hex_of(s: &str) -> Vec<u8> {
    s.bytes()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .into_bytes()
}

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    find_last_where(hay, needle, |_| true)
}

fn find_last_where(hay: &[u8], needle: &[u8], ok: impl Fn(usize) -> bool) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .rev()
        .find(|&i| ok(i) && &hay[i..i + needle.len()] == needle)
}

/// The body of a JSON string that starts at `at` (just past its opening
/// quote), up to its closing quote, escapes left in place. Hex-encoded input
/// is decoded pairwise first.
fn json_string_at(bytes: &[u8], at: usize, hex: bool) -> Option<String> {
    let mut out = Vec::new();
    let mut i = at;
    let mut escaped = false;
    loop {
        let b = if hex {
            let pair = std::str::from_utf8(bytes.get(i..i + 2)?).ok()?;
            i += 2;
            u8::from_str_radix(pair, 16).ok()?
        } else {
            let b = *bytes.get(i)?;
            i += 1;
            b
        };
        if escaped {
            out.push(b);
            escaped = false;
        } else {
            match b {
                b'\\' => {
                    out.push(b);
                    escaped = true;
                }
                b'"' => break,
                _ => out.push(b),
            }
        }
        if out.len() > 4096 {
            return None;
        }
    }
    String::from_utf8(out).ok()
}

// --------------------------------------------------------------- shared

/// Last user/assistant timestamp in a transcript, reading from the tail so a
/// multi-megabyte file costs a few hundred kilobytes at most. Entries with
/// other types (bookkeeping the host app writes) are ignored, which is the
/// whole point: the file's modification time is not an activity signal.
pub fn last_activity_in(path: &Path) -> Option<u64> {
    last_activity_with(path, activity_timestamp)
}

fn last_activity_with(path: &Path, pick: fn(&str) -> Option<u64>) -> Option<u64> {
    static CACHE: LazyLock<Mutex<crate::activity_cache::ActivityCache>> =
        LazyLock::new(|| Mutex::new(Default::default()));
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .read(path, pick)
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

/// An ISO 8601 UTC string, or epoch seconds or milliseconds.
fn timestamp_of(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::String(s) => parse_iso8601(s),
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f < 0.0 {
                return None;
            }
            Some(if f > 1e11 {
                (f / 1000.0) as u64
            } else {
                f as u64
            })
        }
        _ => None,
    }
}

/// How to read one line of a transcript: what a title looks like, what a
/// person's prompt looks like, and where the working directory is noted.
struct LineReaders {
    title: fn(&str) -> Option<String>,
    prompt: fn(&str) -> Option<String>,
    cwd: fn(&str) -> Option<String>,
}

fn nothing(_: &str) -> Option<String> {
    None
}

const CLAUDE_LINES: LineReaders = LineReaders {
    title: claude_title,
    prompt: claude_prompt,
    cwd: nothing,
};
const CODEX_LINES: LineReaders = LineReaders {
    title: nothing,
    prompt: codex_prompt,
    cwd: nothing,
};
const COPILOT_LINES: LineReaders = LineReaders {
    title: nothing,
    prompt: copilot_prompt,
    cwd: copilot_cwd,
};
const CURSOR_LINES: LineReaders = LineReaders {
    title: nothing,
    prompt: cursor_prompt,
    cwd: nothing,
};

/// What a transcript says a session is called. Transcripts are append-only,
/// so each file is read once in full and then only what was appended since,
/// which keeps a daemon tick to a stat per session.
#[derive(Default, Clone)]
struct Names {
    /// Bytes scanned so far, always at a line boundary.
    scanned_to: u64,
    /// The last title written.
    title: Option<String>,
    /// The first prompt found.
    first_prompt: Option<String>,
    /// The first working directory noted.
    cwd: Option<String>,
}

static NAMES: LazyLock<Mutex<HashMap<PathBuf, Names>>> = LazyLock::new(Default::default);

fn names_in(path: &Path, r: &LineReaders) -> Names {
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
        let mut reader = BufReader::with_capacity(64 * 1024, f);
        let mut line = String::new();
        loop {
            line.clear();
            let n = match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if !line.ends_with('\n') {
                break; // still being written; pick it up next time
            }
            entry.scanned_to += n as u64;
            if let Some(t) = (r.title)(&line) {
                entry.title = Some(t);
            }
            if entry.first_prompt.is_none() {
                entry.first_prompt = (r.prompt)(&line);
            }
            if entry.cwd.is_none() {
                entry.cwd = (r.cwd)(&line);
            }
        }
    }
    entry.clone()
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
    fn codex_meta_tells_threads_from_helpers() {
        let own = codex_meta_line(
            r#"{"type":"session_meta","payload":{"id":"01a0-own","cwd":"/w/x","source":"vscode","thread_source":"user"}}"#,
        )
        .unwrap();
        assert_eq!(own.id.as_deref(), Some("01a0-own"));
        assert_eq!(own.cwd.as_deref(), Some("/w/x"));
        assert!(!own.helper);
        let helper = codex_meta_line(
            r#"{"type":"session_meta","payload":{"id":"01a0-h","parent_thread_id":"01a0-own","cwd":"/w/x","source":{"subagent":{"other":"guardian"}}}}"#,
        )
        .unwrap();
        assert!(helper.helper);
        assert!(codex_meta_line(r#"{"type":"event_msg","payload":{}}"#).is_none());
        assert_eq!(
            rollout_uuid(Path::new(
                "/h/.codex/sessions/2026/09/13/rollout-2026-09-13T07-59-25-01a09aa2-f2fc-79f1-a218-5182ef537138.jsonl"
            ))
            .as_deref(),
            Some("01a09aa2-f2fc-79f1-a218-5182ef537138")
        );
    }

    #[test]
    fn codex_index_names_threads() {
        assert_eq!(
            codex_index_line(
                r#"{"id":"01a0","thread_name":"Check low trade volume","updated_at":"2026-09-13T11:59:38Z"}"#
            ),
            Some(("01a0".to_string(), "Check low trade volume".to_string()))
        );
        assert!(codex_index_line(r#"{"id":"01a0","thread_name":"  "}"#).is_none());
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
    fn copilot_entries() {
        assert_eq!(
            copilot_activity_timestamp(
                r#"{"type":"user.message","timestamp":"1970-01-02T00:00:00Z","data":{"content":"fix the build"}}"#
            ),
            Some(86_400)
        );
        assert_eq!(
            copilot_activity_timestamp(
                r#"{"type":"tool.execution_complete","timestamp":1757000000000,"data":{}}"#
            ),
            Some(1_757_000_000)
        );
        assert!(
            copilot_activity_timestamp(
                r#"{"type":"session.shutdown","timestamp":"1970-01-02T00:00:00Z","data":{}}"#
            )
            .is_none()
        );
        assert_eq!(
            copilot_prompt(
                r#"{"type":"user.message","timestamp":"1970-01-02T00:00:00Z","data":{"content":"fix the\n build"}}"#
            ),
            Some("fix the build".to_string())
        );
        assert_eq!(
            copilot_cwd(
                r#"{"type":"session.info","data":{"infoType":"folder_trust","message":"Folder /Users/a/code/x has been added to trusted folders."}}"#
            ),
            Some("/Users/a/code/x".to_string())
        );
        assert_eq!(
            copilot_cwd(r#"{"type":"session.start","data":{"sessionId":"s","cwd":"/w"}}"#),
            Some("/w".to_string())
        );
        assert!(copilot_cwd(r#"{"type":"session.start","data":{"sessionId":"s"}}"#).is_none());
    }

    #[test]
    fn copilot_workspace_names() {
        assert_eq!(
            yaml_top_level("id: x\nname: Fix the build\ncwd: /w\n", "name"),
            Some("Fix the build".to_string())
        );
        assert_eq!(
            yaml_top_level("name: \"Quoted: yes\"\n", "name"),
            Some("Quoted: yes".to_string())
        );
        assert_eq!(
            yaml_top_level(
                "id: x\nname: |-\n  Block scalar title\n  second line\ncwd: /w\n",
                "name"
            ),
            Some("Block scalar title".to_string())
        );
        assert_eq!(
            yaml_top_level("id: x\nname: |-\n  Block scalar title\ncwd: /w\n", "cwd"),
            Some("/w".to_string())
        );
        assert!(yaml_top_level("name: |-\n", "name").is_none());
        assert!(yaml_top_level("  name: nested\n", "name").is_none());
    }

    #[test]
    fn session_dirs_from_open_files() {
        let dir = |p: &str, m: &str, d: usize| {
            session_dir_below(Path::new(p), m, d).map(|p| p.to_string_lossy().into_owned())
        };
        assert_eq!(
            dir(
                "/h/.copilot/session-state/abc/events.jsonl",
                "session-state",
                1
            )
            .as_deref(),
            Some("/h/.copilot/session-state/abc")
        );
        assert_eq!(
            dir(
                "/h/.copilot/session-state/abc/checkpoints/1.md",
                "session-state",
                1
            )
            .as_deref(),
            Some("/h/.copilot/session-state/abc")
        );
        assert_eq!(
            dir("/h/.cursor/chats/9f8e/0084fd6c/store.db-wal", "chats", 2).as_deref(),
            Some("/h/.cursor/chats/9f8e/0084fd6c")
        );
        // Nothing below the marker, or the marker missing: not a session.
        assert!(dir("/h/.copilot/session-state", "session-state", 1).is_none());
        assert!(dir("/h/.cursor/chats/9f8e", "chats", 2).is_none());
        assert!(dir("/h/.codex/sessions/x.jsonl", "chats", 2).is_none());
    }

    #[test]
    fn cursor_prompts_and_paths() {
        assert_eq!(
            cursor_prompt(
                r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nrun ls command\n</user_query>"}]}}"#
            ),
            Some("run ls command".to_string())
        );
        assert!(
            cursor_prompt(
                r#"{"role":"assistant","message":{"content":[{"type":"text","text":"Running it."}]}}"#
            )
            .is_none()
        );
        assert_eq!(
            cursor_project_dir("/Users/alexm/Repository/Codex-History"),
            "Users-alexm-Repository-Codex-History"
        );
    }

    #[test]
    fn cursor_chat_name_from_store_bytes() {
        let id = "0084fd6c-3541-418e-a5eb-0b704e2882f5";
        let doc = format!(
            r#"{{"agentId":"{id}","name":"Fix the \"login\" bug","mode":"default","createdAt":1757000000000}}"#
        );
        let mut hex = b"SQLite format 3\0junk".to_vec();
        hex.extend(hex_of(&doc));
        hex.extend(b"\0more junk");
        assert_eq!(
            meta_name_in(&hex, id).as_deref(),
            Some("Fix the \"login\" bug")
        );
        // A newer copy later in the buffer (a WAL frame) wins.
        let newer = doc.replace("Fix the \\\"login\\\" bug", "Renamed chat");
        hex.extend(hex_of(&newer));
        assert_eq!(meta_name_in(&hex, id).as_deref(), Some("Renamed chat"));
        // Plain text is accepted too, but only beside the matching agentId.
        let plain = format!("xx{doc}yy").into_bytes();
        assert_eq!(
            meta_name_in(&plain, id).as_deref(),
            Some("Fix the \"login\" bug")
        );
        assert!(meta_name_in(&plain, "other-id").is_none());
        assert!(meta_name_in(b"nothing here", id).is_none());
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
        let n = names_in(&path, &CLAUDE_LINES);
        assert_eq!(
            (n.title, n.first_prompt),
            (None, Some("first ask".to_string()))
        );
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        f.write_all(b"{\"type\":\"custom-title\",\"customTitle\":\"Named\"}\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"second\"}}\n{\"partial")
            .unwrap();
        let n = names_in(&path, &CLAUDE_LINES);
        assert_eq!(
            (n.title, n.first_prompt),
            (Some("Named".to_string()), Some("first ask".to_string()))
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
