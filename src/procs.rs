//! A point-in-time table of every process, with parent/child navigation.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Serialize, Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub exe: Option<PathBuf>,
    pub cmd: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Resident set size in bytes.
    pub rss: u64,
    /// CPU percent over the sample window (100 = one full core).
    pub cpu: f32,
    /// Seconds since the epoch when the process started.
    pub start_time: u64,
    /// Seconds the process has been running.
    pub run_time: u64,
}

impl Proc {
    pub fn exe_name(&self) -> String {
        self.exe
            .as_ref()
            .and_then(|e| e.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.name.clone())
    }

    pub fn cmd_has(&self, needle: &str) -> bool {
        self.cmd.iter().any(|a| a.contains(needle))
    }
}

pub struct ProcTable {
    pub procs: Vec<Proc>,
    index: HashMap<u32, usize>,
    children: HashMap<u32, Vec<u32>>,
}

impl ProcTable {
    /// Collect every process.
    ///
    /// With `Some(sample)`, two refreshes separated by that long give a real
    /// CPU reading from a fresh `System`. With `None`, a single refresh is
    /// taken and CPU is measured since the caller's previous refresh, which is
    /// what a daemon wants: one refresh per tick, CPU averaged over the tick.
    pub fn collect(sys: &mut System, sample: Option<Duration>) -> ProcTable {
        let kind = ProcessRefreshKind::nothing()
            .with_memory()
            .with_cpu()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet);
        if let Some(sample) = sample {
            sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
            std::thread::sleep(sample.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL));
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);

        let mut procs: Vec<Proc> = sys
            .processes()
            .values()
            .map(|p| Proc {
                pid: p.pid().as_u32(),
                ppid: p.parent().map(|x| x.as_u32()),
                name: p.name().to_string_lossy().into_owned(),
                exe: p.exe().map(|x| x.to_path_buf()),
                cmd: p
                    .cmd()
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
                cwd: p.cwd().map(|x| x.to_path_buf()),
                rss: p.memory(),
                cpu: p.cpu_usage(),
                start_time: p.start_time(),
                run_time: p.run_time(),
            })
            .collect();
        procs.sort_by_key(|p| p.pid);

        let mut index = HashMap::with_capacity(procs.len());
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (i, p) in procs.iter().enumerate() {
            index.insert(p.pid, i);
            if let Some(pp) = p.ppid {
                children.entry(pp).or_default().push(p.pid);
            }
        }
        ProcTable {
            procs,
            index,
            children,
        }
    }

    pub fn get(&self, pid: u32) -> Option<&Proc> {
        self.index.get(&pid).map(|&i| &self.procs[i])
    }

    pub fn children(&self, pid: u32) -> Vec<&Proc> {
        self.children
            .get(&pid)
            .map(|v| v.iter().filter_map(|c| self.get(*c)).collect())
            .unwrap_or_default()
    }

    /// Ancestors nearest-first, stopping at the root or a broken chain.
    pub fn ancestors(&self, pid: u32) -> Vec<&Proc> {
        let mut out = Vec::new();
        let mut cur = self.get(pid).and_then(|p| p.ppid);
        let mut guard = 0;
        while let Some(pp) = cur {
            if pp == 0 || guard > 64 {
                break;
            }
            match self.get(pp) {
                Some(p) => {
                    out.push(p);
                    cur = p.ppid;
                }
                None => break,
            }
            guard += 1;
        }
        out
    }

    pub fn descendants(&self, pid: u32) -> Vec<&Proc> {
        let mut out = Vec::new();
        let mut stack = vec![pid];
        let mut guard = 0usize;
        while let Some(cur) = stack.pop() {
            for c in self.children(cur) {
                out.push(c);
                stack.push(c.pid);
            }
            guard += 1;
            if guard > 100_000 {
                break;
            }
        }
        out
    }
}
