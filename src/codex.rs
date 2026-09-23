//! Individual task lifecycle through an already-running, opt-in desktop bridge.
//! No backend is launched, killed, or reconfigured here. Missing evidence locks tasks.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub backend_pid: u32,
    pub backend_created_at: u64,
    pub id: String,
    pub codex_home: String,
    pub revision: String,
    pub name: String,
    pub cwd: String,
    pub transcript: String,
    pub updated_at: u64,
    pub state: String,
    pub protection: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backend {
    pub pid: u32,
    pub tasks: Vec<Task>,
    pub error: Option<String>,
}

pub fn inspect() -> Vec<Backend> {
    match request("inspect", None) {
        Ok(value) => serde_json::from_value(value).unwrap_or_else(|e| unavailable(e.to_string())),
        Err(e) => unavailable(e.to_string()),
    }
}

fn unavailable(error: String) -> Vec<Backend> {
    vec![Backend {
        pid: 0,
        tasks: vec![],
        error: Some(error),
    }]
}

pub(crate) fn prepare(task: &Task) -> Result<Task> {
    serde_json::from_value(request("prepare", Some(task))?).context("invalid task review")
}

pub(crate) fn perform(operation: &str, task: &Task) -> Result<String> {
    request(operation, Some(task))?["result"]
        .as_str()
        .map(str::to_owned)
        .context("invalid Codex action response")
}

/// Re-read opt-in and the exact configuration revision used for the warning.
/// Pass its file identity to the helper so disabling cleanup during RPC checks
/// prevents the final archive as well.
pub(crate) fn auto_guard(cfg: &crate::daemon::DaemonConfig, revision: &str) -> Result<Value> {
    let (file, path) = crate::config::Config::load()?;
    anyhow::ensure!(
        file.auto_archive_codex
            && cfg.auto.archive_codex
            && file.auto_dry_run == cfg.auto.dry_run
            && file.file_revision == cfg.auto.config_file_revision
            && revision == cfg.codex_rules_revision(),
        "Codex archive settings changed; a fresh warning is required"
    );
    let path = path.context("automatic archiving requires saved settings")?;
    let meta = std::fs::metadata(&path)?;
    anyhow::ensure!(
        crate::config::file_revision(&meta)? == file.file_revision,
        "settings changed during revalidation"
    );
    Ok(
        json!({"config_path": path, "config_stamp": config_stamp(&meta)?,
        "stale_after_secs": cfg.thresholds.stale_after_secs}),
    )
}

#[cfg(unix)]
fn config_stamp(meta: &std::fs::Metadata) -> Result<Value> {
    use std::os::unix::fs::MetadataExt;
    Ok(json!([
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128,
        meta.ctime() as i128 * 1_000_000_000 + meta.ctime_nsec() as i128
    ]))
}
#[cfg(not(unix))]
fn config_stamp(_meta: &std::fs::Metadata) -> Result<Value> {
    bail!("automatic Codex archiving requires the macOS bridge")
}

pub(crate) fn perform_auto(
    task: &Task,
    cfg: &crate::daemon::DaemonConfig,
    revision: &str,
) -> Result<String> {
    anyhow::ensure!(!cfg.auto.dry_run, "preview cannot archive tasks");
    let guard = auto_guard(cfg, revision)?;
    request_with("archive", Some(task), Some(guard))?["result"]
        .as_str()
        .map(str::to_owned)
        .context("invalid Codex action response")
}

fn request(operation: &str, task: Option<&Task>) -> Result<Value> {
    request_with(operation, task, None)
}

fn request_with(operation: &str, task: Option<&Task>, automatic: Option<Value>) -> Result<Value> {
    let registry = crate::paths::data_dir()
        .context("no data directory")?
        .join("codex-bridge");
    if operation == "inspect" && !registry.exists() {
        return Ok(json!([]));
    }
    if let Some(task) = task
        && (task.id.len() > 128 || task.revision.len() != 64 || task.backend_pid == 0)
    {
        bail!("invalid task identity");
    }
    let (cfg, _) = crate::config::Config::load()?;
    let protected: Vec<String> = std::env::var("CODEX_THREAD_ID").ok().into_iter().collect();
    run_helper(
        &json!({"operation": operation, "registry": registry, "task": task,
        "automatic": automatic, "protected_ids": protected, "ignore_projects": cfg.ignore_projects, "ignore_apps": cfg.ignore_apps}),
    )
}

#[cfg(target_os = "macos")]
fn run_helper(request: &Value) -> Result<Value> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    // Embedded code and isolated Python prevent imports from writable user paths.
    // This is the same standard-library transport used by the installed bridge.
    const HELPER: &str = concat!(
        include_str!("../experiments/codex-bridge/wire.py"),
        "\n",
        include_str!("codex_control.py")
    );
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-u", "-c", HELPER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Codex bridge requires /usr/bin/python3")?;
    let stdout = child.stdout.take().context("missing helper output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let input = serde_json::to_vec(request)?;
    let sent = child
        .stdin
        .take()
        .context("missing helper input")?
        .write_all(&input);
    let deadline = Instant::now() + Duration::from_secs(25);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if sent.is_err() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("Codex bridge timed out or lost input; inspect Actions before retrying");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Codex helper reader failed"))??;
    if !status.success() || bytes.len() > 4 * 1024 * 1024 {
        bail!("Codex bridge returned incomplete output");
    }
    let response: Value =
        serde_json::from_slice(&bytes).context("invalid Codex bridge response")?;
    if let Some(error) = response.get("error") {
        bail!("{}", error.as_str().unwrap_or("Codex bridge failed"));
    }
    response
        .get("ok")
        .cloned()
        .context("missing Codex bridge response")
}

#[cfg(not(target_os = "macos"))]
fn run_helper(_request: &Value) -> Result<Value> {
    bail!("Codex desktop task control requires the macOS bridge")
}
