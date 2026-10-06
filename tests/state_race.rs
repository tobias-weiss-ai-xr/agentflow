//! Concurrency safety of the JSON status ledger: many threads saving at once
//! must never tear `run-state.json`.
//!
//! Regression for the shared-`tmp{pid}` write race: every thread used to
//! truncate and interleave the SAME temp file, then rename it — installing a
//! byte-mixed document while the other rejects failed with ENOENT. The fix
//! gives every save its own temp path (pid + atomic sequence) and keeps the
//! rename as the single atomic install step.

use agentflow::config::TaskState;
use agentflow::state::{Store, TaskStatus};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("af-race-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-entry map whose JSON length varies with `len`, so interleaved
/// writers produce visibly different documents (identical tiny payloads can
/// hide the race).
fn payload(len: usize) -> HashMap<String, TaskStatus> {
    let mut m = HashMap::new();
    m.insert(
        "A".to_string(),
        TaskStatus {
            state: TaskState::Running,
            attempts: 1,
            last_error: Some("x".repeat(len)),
            phase: None,
            attempt_started_ts: None,
            attempt_worker: None,
        },
    );
    m
}

#[test]
fn concurrent_saves_never_tear_the_state_file() {
    let dir = tmpdir("save");
    let store = Store::new(dir.clone());
    // Seed the ledger so the reader always has a document to parse while the
    // writers race (a missing file is legitimate and skipped below).
    store.save(&payload(8)).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let save_errors = Arc::new(AtomicUsize::new(0));
    let torn_reads = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));

    const THREADS: usize = 4;
    const SAVES_PER_THREAD: usize = 100;

    let mut writers = Vec::new();
    for t in 0..THREADS {
        let store = store.clone();
        let save_errors = Arc::clone(&save_errors);
        writers.push(std::thread::spawn(move || {
            // Alternate a huge payload with a tiny one between threads so an
            // interleaved write is actually detectable.
            let len = if t % 2 == 0 { 12_000 } else { 4 };
            let m = payload(len);
            for _ in 0..SAVES_PER_THREAD {
                if store.save(&m).is_err() {
                    save_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    let reader = {
        let status_file = store.status_file();
        let stop = Arc::clone(&stop);
        let torn_reads = Arc::clone(&torn_reads);
        let reads = Arc::clone(&reads);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // Missing is a legitimate state (nothing installed yet);
                // only a present-but-unparseable file is a tear.
                if let Ok(s) = std::fs::read_to_string(&status_file) {
                    reads.fetch_add(1, Ordering::Relaxed);
                    if serde_json::from_str::<HashMap<String, TaskStatus>>(&s).is_err() {
                        torn_reads.fetch_add(1, Ordering::Relaxed);
                    }
                }
                // Don't starve the writers; the rename is atomic either way.
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        })
    };

    for w in writers {
        w.join().unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    reader.join().unwrap();

    assert_eq!(
        save_errors.load(Ordering::Relaxed),
        0,
        "every concurrent save must succeed (the old shared temp path made some fail ENOENT)"
    );
    assert_eq!(
        torn_reads.load(Ordering::Relaxed),
        0,
        "no reader may observe a torn/partial state file"
    );
    assert!(
        reads.load(Ordering::Relaxed) > 0,
        "the reader must actually have exercised the file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
