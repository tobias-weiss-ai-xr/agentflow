//! State: atomic JSON persistence of task status + cost receipts.
//!
//! Writes go to a unique temp file (per-process id + a process-global atomic
//! sequence), are fsync'd, then atomically renamed over `run-state.json` — so
//! even several attempt threads saving at once can never corrupt the readable
//! state. A state file that exists but does not parse is reported, never
//! silently treated as an empty campaign (ADR-3, ch. 6.2 of arc42).

use crate::config::TaskState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskStatus {
    pub state: TaskState,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// Journal: the furthest phase this task's current attempt reached
    /// (effect sandwich, pi-durable). `None` on legacy state files and
    /// between attempts — `resume_action` maps it to `RerunAgent`.
    #[serde(default)]
    pub phase: Option<AttemptPhase>,
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus {
            state: TaskState::Ready,
            attempts: 0,
            last_error: None,
            phase: None,
        }
    }
}

/// One attempt's phase in the effect sandwich (pi-durable journal):
/// commit intent → perform effect → commit outcome. Persisted at every
/// boundary so a crashed orchestrator can resume an attempt without
/// re-running the expensive, non-replayable agent step.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    /// Worktree created; the agent may be running but nothing durable
    /// proves it finished. Resume must re-run the agent (safe).
    #[default]
    Spawned,
    /// The agent exited 0 and its change is committed on the attempt
    /// branch. Never re-invoke the agent after this point.
    AgentDone,
    /// The acceptance gate passed. Only the (idempotent) merge remains.
    GatePassed,
}

/// How a stale `running` attempt resumes after a crash, derived from its
/// journaled [`AttemptPhase`] (startup heal in `run.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAction {
    /// No durable agent outcome → fresh attempt (agent, gate, merge).
    RerunAgent,
    /// Agent outcome is committed → re-run only the acceptance gate, merge.
    RerunGate,
    /// Gate already passed → re-run only the merge, idempotently.
    MergeOnly,
}

/// Map a journaled phase to the resume plan. `None` (legacy state files,
/// inter-attempt gap) and `Spawned` both mean "the agent's outcome was
/// never committed" → the always-safe `RerunAgent`.
pub fn resume_action(phase: Option<AttemptPhase>) -> ResumeAction {
    match phase {
        None | Some(AttemptPhase::Spawned) => ResumeAction::RerunAgent,
        Some(AttemptPhase::AgentDone) => ResumeAction::RerunGate,
        Some(AttemptPhase::GatePassed) => ResumeAction::MergeOnly,
    }
}

/// Outcome recorded on a receipt. Pre-routing receipts (only ever written
/// on success) deserialize as "merged".
fn default_outcome() -> String {
    "merged".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub task: String,
    pub attempt: u32,
    pub worker: String,
    pub model: String,
    pub wall_clock_s: f64,
    pub tokens: Option<u64>,
    pub ts: u64,
    #[serde(default = "default_outcome")]
    pub outcome: String,
    /// First line of the failure reason (≤200 chars); None on merged
    /// attempts and for legacy receipts.
    #[serde(default)]
    pub error: Option<String>,
}

/// The persistence interface every agentflow backend must provide.
///
/// A `StateStore` owns one directory and is responsible for two things:
/// atomic status persistence (a torn write must never corrupt the readable
/// state) and append-only receipts (no two receipts may overwrite one
/// another). The generic conformance suite in `tests/conformance.rs`
/// encodes those guarantees so every backend is held to the same contract.
pub trait StateStore {
    fn status_file(&self) -> PathBuf;
    fn load(&self) -> HashMap<String, TaskStatus>;
    fn save(&self, m: &HashMap<String, TaskStatus>) -> io::Result<()>;
    fn append_receipt(&self, r: &Receipt) -> io::Result<()>;
    fn load_receipts(&self) -> Vec<Receipt>;
}

#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    pub fn status_file(&self) -> PathBuf {
        self.dir.join("run-state.json")
    }

    /// Read the status map for read-only display paths.
    ///
    /// LOUD on corruption: a state file that exists but cannot be parsed is
    /// reported to stderr (naming the file) and only then treated as empty —
    /// it is never silently presented as a fresh campaign. Any path that
    /// would ACT on the result must use [`Store::load_checked`] instead.
    pub fn load(&self) -> HashMap<String, TaskStatus> {
        match self.load_checked() {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "warning: {e}; showing an empty campaign (the unreadable file was left in place)"
                );
                HashMap::new()
            }
        }
    }

    /// Fallible read of the status map.
    ///
    /// * `Ok(empty)` when the state file does not exist — a fresh campaign is
    ///   legitimate, not an error;
    /// * `Err` (naming the path and the parse problem) when the file exists
    ///   but cannot be read or is not a task map — the file is never
    ///   modified or overwritten, it is evidence;
    /// * `Ok(map)` otherwise.
    pub fn load_checked(&self) -> io::Result<HashMap<String, TaskStatus>> {
        let path = self.status_file();
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("read state file {}: {e}", path.display()),
                ));
            }
        };
        serde_json::from_str::<HashMap<String, TaskStatus>>(&raw).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse state file {}: {e}", path.display()),
            )
        })
    }

    /// Atomically persist `m` to `run-state.json`.
    ///
    /// Each call writes its own temp file, fsyncs it, then renames it over
    /// the final path. Concurrent callers never share or truncate one
    /// another's temp file, and the rename is the single atomic install step.
    /// The temp file is removed if the write or the rename fails.
    pub fn save(&self, m: &HashMap<String, TaskStatus>) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let data = serde_json::to_vec_pretty(m).map_err(io::Error::other)?;
        atomic_write(&self.dir, "run-state.json", &data)
    }

    pub fn log_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }

    pub fn prompt_dir(&self) -> PathBuf {
        self.dir.join("prompts")
    }

    pub fn receipt_dir(&self) -> PathBuf {
        self.dir.join("receipts")
    }

    pub fn append_receipt(&self, r: &Receipt) -> io::Result<()> {
        std::fs::create_dir_all(self.receipt_dir())?;
        let data = serde_json::to_vec_pretty(r).map_err(io::Error::other)?;
        // Nanos in the name: fast retries can land in the same second
        // (task-attempt-ts would otherwise overwrite, losing receipts). The
        // `.json` suffix marks a COMPLETE receipt; `atomic_write` installs it
        // through a non-`.json` temp file while the bytes are in flight.
        let name = format!(
            "{}-{}-{}-{}x.json",
            r.task,
            r.attempt,
            r.ts,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        atomic_write(&self.receipt_dir(), &name, &data)
    }

    /// All receipts across runs (sorted by timestamp), for `af cost`.
    ///
    /// Read-only path: unreadable `.json` receipts are dropped from the
    /// displayed history. Use [`Store::load_receipts_checked`] to see them.
    pub fn load_receipts(&self) -> Vec<Receipt> {
        self.load_receipts_checked().0
    }

    /// Read every `.json` receipt, returning the valid receipts (sorted by
    /// `ts`) and one human-readable message per unreadable `.json` file.
    ///
    /// Only `.json` files are considered at all: an interrupted atomic write
    /// leaves `<final-name>.tmp-<pid>-<counter>`, which is structurally
    /// incapable of being counted as a phantom receipt. A corrupt receipt is
    /// history, not a fatal error — it is reported by name, never allowed to
    /// block a campaign.
    pub fn load_receipts_checked(&self) -> (Vec<Receipt>, Vec<String>) {
        let mut out = Vec::new();
        let mut problems = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.receipt_dir()) {
            for e in rd.flatten() {
                let path = e.path();
                // A leftover temp file (`<name>.json.tmp-...`) is not `.json`
                // and can never be mistaken for a completed receipt.
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                match std::fs::read_to_string(&path) {
                    Ok(s) => match serde_json::from_str::<Receipt>(&s) {
                        Ok(r) => out.push(r),
                        Err(err) => problems.push(format!("unreadable receipt {name}: {err}")),
                    },
                    Err(err) => problems.push(format!("unreadable receipt {name}: {err}")),
                }
            }
        }
        out.sort_by_key(|r| r.ts);
        (out, problems)
    }

    pub fn lock_file(&self) -> PathBuf {
        self.dir.join(".lock")
    }

    /// Acquire the single-writer lock for this state dir.
    ///
    /// Unlinks `run-state.json` from concurrent `af run` processes: creates
    /// `<state_dir>/.lock` with O_CREAT|O_EXCL and records this pid. A second
    /// live owner gets an error; a lock whose pid is dead is reclaimed.
    pub fn acquire_lock(&self) -> io::Result<LockGuard> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.lock_file();
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                writeln!(f, "{}", std::process::id())?;
                Ok(LockGuard {
                    state_dir: self.dir.clone(),
                })
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let existing = std::fs::read_to_string(&path).unwrap_or_default();
                let pid: u32 = existing.trim().parse().unwrap_or(0);
                if pid != 0 && pid_alive(pid) {
                    Err(io::Error::other(format!(
                        "another af owns this state dir (pid {pid})"
                    )))
                } else {
                    // Stale lock: the owner is gone. Reclaim it.
                    std::fs::write(&path, format!("{}\n", std::process::id()))?;
                    Ok(LockGuard {
                        state_dir: self.dir.clone(),
                    })
                }
            }
            Err(e) => Err(e),
        }
    }
}

impl StateStore for Store {
    fn status_file(&self) -> PathBuf {
        Store::status_file(self)
    }

    fn load(&self) -> HashMap<String, TaskStatus> {
        Store::load(self)
    }

    fn save(&self, m: &HashMap<String, TaskStatus>) -> io::Result<()> {
        Store::save(self, m)
    }

    fn append_receipt(&self, r: &Receipt) -> io::Result<()> {
        Store::append_receipt(self, r)
    }

    fn load_receipts(&self) -> Vec<Receipt> {
        Store::load_receipts(self)
    }
}

/// Held for the lifetime of a run; releasing it removes `<state_dir>/.lock`.
#[derive(Debug)]
pub struct LockGuard {
    state_dir: PathBuf,
}

impl LockGuard {
    pub fn state_dir(&self) -> &PathBuf {
        &self.state_dir
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.state_dir.join(".lock"));
    }
}

/// Liveness check with no extra dependency: `kill -0 <pid>` succeeds only
/// while the process exists (and we may signal it).
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("kill -0 {pid}")])
        // The probe is expected to fail for stale pids; don't spam stderr.
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Process-global sequence: distinguishes every temp path this process ever
/// creates, so concurrent writers (threads) never share one.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Atomically install `data` at `<dir>/<name>`.
///
/// Writes a UNIQUE temp file in the SAME directory (so the final rename is
/// atomic), fsyncs it, then renames it over the final path. The temp name is
/// deliberately NOT `*.json` — it is `<name>.tmp-<pid>-<counter>` — so an
/// interrupted write can never be mistaken for a receipt. The temp file is
/// removed if the write or the rename fails.
fn atomic_write(dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    let final_path = dir.join(name);
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{name}.tmp-{}-{seq}", std::process::id()));
    let installed = std::fs::File::create(&tmp)
        .and_then(|mut f| {
            // fsync before rename: a power loss cannot install a truncated
            // or empty document.
            f.write_all(data)?;
            f.sync_all()
        })
        .and_then(|_| std::fs::rename(&tmp, &final_path));
    if installed.is_err() {
        // Never leak a partial temp file for the next process to guess at.
        let _ = std::fs::remove_file(&tmp);
    }
    installed
}

pub fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("af-state-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn torn_write_never_corrupts() {
        let dir = tmpdir();
        let store = Store::new(dir.clone());
        let mut m = HashMap::new();
        m.insert(
            "A".to_string(),
            TaskStatus {
                state: TaskState::Done,
                attempts: 1,
                last_error: None,
                phase: None,
            },
        );
        store.save(&m).unwrap();
        // Simulate an interrupted write: a temp file left behind (no rename to final).
        std::fs::write(
            dir.join("run-state.json.tmp12345"),
            r#"{ "A": { "state": "running", "#,
        )
        .unwrap();
        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["A"].state, TaskState::Done);
        // Stale temp files never override the real state file.
        assert!(std::fs::read_to_string(store.status_file())
            .unwrap()
            .contains("done"));
    }

    #[test]
    fn resume_action_maps_every_phase_to_a_resume_plan() {
        assert_eq!(resume_action(None), ResumeAction::RerunAgent);
        assert_eq!(
            resume_action(Some(AttemptPhase::Spawned)),
            ResumeAction::RerunAgent,
            "Spawned means no durable agent outcome — safe re-run"
        );
        assert_eq!(
            resume_action(Some(AttemptPhase::AgentDone)),
            ResumeAction::RerunGate
        );
        assert_eq!(
            resume_action(Some(AttemptPhase::GatePassed)),
            ResumeAction::MergeOnly
        );
    }

    #[test]
    fn phase_persists_snake_case_and_legacy_files_load_without_it() {
        let dir = tmpdir();
        let store = Store::new(dir.clone());
        let mut m = HashMap::new();
        m.insert(
            "A".to_string(),
            TaskStatus {
                state: TaskState::Running,
                attempts: 1,
                last_error: None,
                phase: Some(AttemptPhase::AgentDone),
            },
        );
        store.save(&m).unwrap();
        let raw = std::fs::read_to_string(store.status_file()).unwrap();
        assert!(
            raw.contains("\"agent_done\""),
            "phase must serialize as snake_case: {raw}"
        );
        assert_eq!(store.load()["A"].phase, Some(AttemptPhase::AgentDone));

        // Legacy state file (written before the journal existed) has no
        // `phase` key — it must still load, as None (= RerunAgent).
        std::fs::write(
            store.status_file(),
            r#"{ "A": { "state": "running", "attempts": 1, "last_error": null } }"#,
        )
        .unwrap();
        let s = &store.load()["A"];
        assert_eq!(s.state, TaskState::Running);
        assert_eq!(s.phase, None, "missing phase key defaults to None");
        assert_eq!(resume_action(s.phase), ResumeAction::RerunAgent);
    }

    #[test]
    fn legacy_receipt_without_outcome_is_merged() {
        let d = tmpdir();
        let body = r#"{"task":"A","attempt":1,"worker":"w1","model":"m","wall_clock_s":1.0,"tokens":null,"ts":0}"#;
        let dir = d.join("receipts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("A-1-0.json"), body).unwrap();
        let rs = Store::new(d).load_receipts();
        assert_eq!(rs.len(), 1);
        assert_eq!(
            rs[0].outcome, "merged",
            "pre-routing receipts were success-only"
        );
    }

    #[test]
    fn receipts_append_and_aggregate() {
        let store = Store::new(tmpdir());
        store
            .append_receipt(&Receipt {
                task: "A".into(),
                attempt: 1,
                worker: "w1".into(),
                model: "m".into(),
                wall_clock_s: 12.5,
                tokens: Some(100),
                ts: 1,
                outcome: "merged".into(),
                error: None,
            })
            .unwrap();
        store
            .append_receipt(&Receipt {
                task: "B".into(),
                attempt: 1,
                worker: "w2".into(),
                model: "m".into(),
                wall_clock_s: 7.0,
                tokens: None,
                ts: 2,
                outcome: "failed".into(),
                error: Some("agent exited 7".into()),
            })
            .unwrap();
        let rs = store.load_receipts();
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].task, "A");
        assert_eq!(rs[0].wall_clock_s, 12.5);
        let total: f64 = rs.iter().map(|r| r.wall_clock_s).sum();
        assert!((total - 19.5).abs() < 1e-9);
    }
}
