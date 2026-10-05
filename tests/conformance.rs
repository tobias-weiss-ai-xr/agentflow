//! Reusable storage conformance suite.
//!
//! Every persistence backend agentflow may grow (today only the JSON
//! [`Store`], tomorrow sqlite/redb/…) must pass the same contract:
//!
//! 1. a torn write never corrupts the readable status file;
//! 2. receipts are append-only — equal keys never overwrite one another;
//! 3. `load_receipts()` is ordered by timestamp;
//! 4. a saved status map round-trips through a fresh handle.
//!
//! `conformance_suite` is generic over [`StateStore`], so the same checks can
//! be pointed at any backend. The `#[test]`s below run it against `Store`, the
//! backend shipped today; each test gets its own temp dir so they stay
//! independent under cargo's parallel test runner.

use agentflow::state::{Receipt, StateStore, Store, TaskStatus};
use agentflow::TaskState;
use std::collections::HashMap;
use std::path::PathBuf;

fn tmpdir() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("af-conformance-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sample_map() -> HashMap<String, TaskStatus> {
    let mut m = HashMap::new();
    m.insert(
        "A".to_string(),
        TaskStatus {
            state: TaskState::Done,
            attempts: 1,
            last_error: None,
        },
    );
    m
}

fn receipt(task: &str, attempt: u32, ts: u64) -> Receipt {
    Receipt {
        task: task.to_string(),
        attempt,
        worker: "w1".to_string(),
        model: "m".to_string(),
        wall_clock_s: 1.0,
        tokens: None,
        ts,
        outcome: "merged".to_string(),
        error: None,
    }
}

/// A leftover partial temp file must never override the real status file.
fn check_torn_write_never_corrupts<S: StateStore>(make: &impl Fn() -> S) {
    let store = make();
    store.save(&sample_map()).expect("save status");

    let dir = store
        .status_file()
        .parent()
        .expect("status file lives in a directory")
        .to_path_buf();
    std::fs::write(
        dir.join("run-state.json.tmp12345"),
        r#"{ "A": { "state": "running", "#,
    )
    .expect("simulate an interrupted write");

    let loaded = store.load();
    assert_eq!(loaded.len(), 1, "partial temp file must not add entries");
    assert_eq!(loaded["A"].state, TaskState::Done);
    let raw = std::fs::read_to_string(store.status_file()).expect("read status file");
    assert!(
        raw.contains("done"),
        "the real status file must win over the partial temp file"
    );
}

/// Two receipts with the same (task, attempt, ts) must both survive.
fn check_append_only_receipts_never_overwrite<S: StateStore>(make: &impl Fn() -> S) {
    let store = make();
    store
        .append_receipt(&receipt("T", 1, 10))
        .expect("first receipt");
    store
        .append_receipt(&receipt("T", 1, 10))
        .expect("second receipt");

    let matching = store
        .load_receipts()
        .into_iter()
        .filter(|r| r.task == "T" && r.attempt == 1 && r.ts == 10)
        .count();
    assert_eq!(
        matching, 2,
        "append-only storage must not overwrite a receipt with an equal key"
    );
}

/// `load_receipts()` must be ascending by timestamp.
fn check_receipts_order_by_timestamp<S: StateStore>(make: &impl Fn() -> S) {
    let store = make();
    for ts in [30u64, 10, 20] {
        store
            .append_receipt(&receipt("O", 1, ts))
            .expect("append receipt");
    }

    let loaded = store.load_receipts();
    assert!(
        loaded.windows(2).all(|w| w[0].ts <= w[1].ts),
        "receipts must load in ascending timestamp order"
    );
    let ordered: Vec<u64> = loaded
        .iter()
        .filter(|r| r.task == "O")
        .map(|r| r.ts)
        .collect();
    assert_eq!(
        ordered,
        vec![10, 20, 30],
        "receipts must be sorted by ts, not insertion order"
    );
}

/// Saving then loading from a fresh handle must yield an equal map.
fn check_save_roundtrips_across_reopen<S: StateStore>(make: &impl Fn() -> S) {
    let store = make();
    let map = sample_map();
    store.save(&map).expect("save status");

    let reopened = make();
    assert_eq!(
        reopened.load(),
        map,
        "a fresh handle over the same location must see the saved map"
    );
}

/// Run every storage conformance check against a backend factory.
///
/// `make` must return handles to the *same* storage location so the reopen
/// check is meaningful; callers give each backend invocation its own temp dir.
pub fn conformance_suite<S: StateStore>(make: impl Fn() -> S) {
    check_torn_write_never_corrupts(&make);
    check_append_only_receipts_never_overwrite(&make);
    check_receipts_order_by_timestamp(&make);
    check_save_roundtrips_across_reopen(&make);
}

#[test]
fn conformance_torn_write_never_corrupts() {
    let dir = tmpdir();
    conformance_suite(move || Store::new(dir.clone()));
}

#[test]
fn conformance_append_only_receipts_never_overwrite() {
    let dir = tmpdir();
    conformance_suite(move || Store::new(dir.clone()));
}

#[test]
fn conformance_receipts_order_by_timestamp() {
    let dir = tmpdir();
    conformance_suite(move || Store::new(dir.clone()));
}

#[test]
fn conformance_save_roundtrips_across_reopen() {
    let dir = tmpdir();
    conformance_suite(move || Store::new(dir.clone()));
}
