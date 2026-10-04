//! State: atomic JSON persistence of task status + cost receipts.
//!
//! One writer (the orchestrator process), temp-file + rename so a torn write
//! never leaves corrupt JSON (ADR-3, ch. 6.2 of arc42).

use crate::config::TaskState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskStatus {
    pub state: TaskState,
    pub attempts: u32,
    pub last_error: Option<String>,
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus {
            state: TaskState::Ready,
            attempts: 0,
            last_error: None,
        }
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

    pub fn load(&self) -> HashMap<String, TaskStatus> {
        match std::fs::read_to_string(self.status_file()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    pub fn save(&self, m: &HashMap<String, TaskStatus>) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!("run-state.json.tmp{}", std::process::id()));
        let data = serde_json::to_vec_pretty(m).map_err(io::Error::other)?;
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, self.status_file())?;
        Ok(())
    }

    /// Load-modify-save in one atomic step; returns the modified status.
    pub fn update<F>(&self, id: &str, f: F) -> io::Result<TaskStatus>
    where
        F: FnOnce(&mut TaskStatus),
    {
        let mut m = self.load();
        let val = {
            let e = m.entry(id.to_string()).or_default();
            f(e);
            e.clone()
        };
        self.save(&m)?;
        Ok(val)
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
        let path = self
            .receipt_dir()
            // Nanos in the name: fast retries can land in the same second
            // (task-attempt-ts would otherwise overwrite, losing receipts).
            .join(format!(
                "{}-{}-{}-{}x.json",
                r.task,
                r.attempt,
                r.ts,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
        std::fs::write(path, data)
    }

    /// All receipts across runs (sorted by timestamp), for `af cost`.
    pub fn load_receipts(&self) -> Vec<Receipt> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.receipt_dir()) {
            for e in rd.flatten() {
                if let Ok(s) = std::fs::read_to_string(e.path()) {
                    if let Ok(r) = serde_json::from_str::<Receipt>(&s) {
                        out.push(r);
                    }
                }
            }
        }
        out.sort_by_key(|r| r.ts);
        out
    }
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
    fn transition_persisted_before_readable() {
        let store = Store::new(tmpdir());
        store
            .update("T1", |s| {
                s.state = TaskState::Running;
                s.attempts = 1;
            })
            .unwrap();
        let back = store.load();
        assert_eq!(back["T1"].state, TaskState::Running);
        assert_eq!(back["T1"].attempts, 1);
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
        assert_eq!(rs[0].outcome, "merged", "pre-routing receipts were success-only");
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
