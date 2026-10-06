//! Receipt durability: writes are atomic (unique temp file + fsync + rename)
//! and the loader can never be fooled by a half-written or torn receipt.
//!
//! Regression for two cost-integrity bugs:
//!
//! * `append_receipt` wrote straight to the final path, so a crash (SIGKILL,
//!   OOM, power loss) could leave a TRUNCATED receipt on disk that the old
//!   `load_receipts` then silently dropped — undercounting both spend and
//!   worker trust;
//! * `load_receipts` parsed EVERY file in the receipts directory, so a
//!   leftover temp file holding valid JSON was counted as a phantom receipt —
//!   inflating spend and trust.
//!
//! The fix writes through a unique non-`.json` temp file and renames it into
//! place, and the loader only ever considers `*.json`.

use agentflow::state::{Receipt, Store};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A unique per-test temp dir under its own subdir; removed by the caller.
fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("af-receipt-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn receipt(task: &str, attempt: u32, wall: f64, ts: u64) -> Receipt {
    Receipt {
        task: task.to_string(),
        attempt,
        worker: "w1".to_string(),
        model: "m".to_string(),
        wall_clock_s: wall,
        tokens: None,
        ts,
        outcome: "merged".to_string(),
        error: None,
        cost_micros: None,
    }
}

#[test]
fn receipt_writes_are_atomic_and_temp_files_are_never_counted() {
    let dir = tmpdir("atomic");
    let store = Store::new(dir.clone());

    const THREADS: usize = 4;
    const PER_THREAD: usize = 50;

    let mut handles = Vec::new();
    for t in 0..THREADS {
        let store = store.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..PER_THREAD {
                // Distinct attempt number per thread, so the final filenames
                // cannot collide — the count is deterministic WITHOUT relying
                // on the timestamp-nanos component for uniqueness.
                let attempt = (t * PER_THREAD + i) as u32 + 1;
                store
                    .append_receipt(&receipt("T", attempt, 1.0, i as u64))
                    .expect("every concurrent append must succeed");
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let receipts_dir = store.receipt_dir();
    let entries: Vec<String> = std::fs::read_dir(&receipts_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    // Exactly one `.json` receipt per append, and every one parses.
    let json: Vec<&String> = entries.iter().filter(|n| n.ends_with(".json")).collect();
    assert_eq!(
        json.len(),
        THREADS * PER_THREAD,
        "one .json receipt per append: {entries:?}"
    );
    for name in &json {
        let raw = std::fs::read_to_string(receipts_dir.join(name.as_str())).unwrap();
        serde_json::from_str::<Receipt>(&raw)
            .unwrap_or_else(|e| panic!("receipt {name} must parse: {e}"));
    }
    assert_eq!(
        store.load_receipts().len(),
        THREADS * PER_THREAD,
        "the loader must see every appended receipt"
    );

    // A successful append never leaves its temp file behind.
    let leftover_temps: Vec<&String> = entries.iter().filter(|n| n.contains(".tmp-")).collect();
    assert!(
        leftover_temps.is_empty(),
        "no temp files may survive a successful append: {leftover_temps:?}"
    );

    // A leftover temp file matching the write convention, holding VALID
    // receipt JSON, must NOT be counted as a receipt. The old loader parsed
    // every file and double-counted it as phantom 100.0s of spend + trust.
    let phantom = receipt("T", 9999, 100.0, 0);
    std::fs::write(
        receipts_dir.join("T-9999-0-0x.json.tmp-999"),
        serde_json::to_vec(&phantom).unwrap(),
    )
    .unwrap();
    let loaded = store.load_receipts();
    assert_eq!(
        loaded.len(),
        THREADS * PER_THREAD,
        "a temp file with valid JSON must never be counted as a receipt"
    );
    assert!(
        !loaded.iter().any(|r| r.attempt == 9999),
        "the phantom attempt must not appear in cost history"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_torn_receipt_is_reported_not_silently_dropped() {
    let dir = tmpdir("torn");
    let store = Store::new(dir.clone());
    let receipts = store.receipt_dir();

    // Valid history: two good receipts.
    store.append_receipt(&receipt("A", 1, 10.0, 1)).unwrap();
    store.append_receipt(&receipt("A", 2, 5.0, 2)).unwrap();
    // A truncated receipt, exactly what a crash mid-write used to leave.
    std::fs::write(
        receipts.join("A-3-3-0x.json"),
        r#"{"task":"A","attempt":3,"worker"#,
    )
    .unwrap();

    // History stays readable, and the torn file is not fatal.
    let loaded = store.load_receipts();
    assert_eq!(
        loaded.len(),
        2,
        "the two valid receipts still load: {loaded:?}"
    );

    // …but the loss is visible, naming the torn file.
    let (valid, problems) = store.load_receipts_checked();
    assert_eq!(valid.len(), 2);
    assert_eq!(
        problems.len(),
        1,
        "one unreadable file is reported: {problems:?}"
    );
    assert!(
        problems[0].contains("A-3-3-0x.json"),
        "the message names the torn file: {}",
        problems[0]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `.json` entry that cannot even be read (e.g. a directory left in the
/// receipts dir) is reported by name too — the same visible-loss contract as
/// a torn file, and it must never panic the cost report.
#[test]
fn unreadable_receipt_entry_is_reported_by_name() {
    let dir = tmpdir("unreadable");
    let store = Store::new(dir.clone());
    let receipts = store.receipt_dir();
    store.append_receipt(&receipt("A", 1, 1.0, 1)).unwrap();
    // A name that ends in `.json` but is not a readable file.
    std::fs::create_dir_all(receipts.join("broken.json")).unwrap();

    assert_eq!(store.load_receipts().len(), 1, "the readable receipt loads");
    let (valid, problems) = store.load_receipts_checked();
    assert_eq!(valid.len(), 1);
    assert_eq!(problems.len(), 1, "the unreadable entry is reported");
    assert!(
        problems[0].contains("broken.json"),
        "the message names the unreadable entry: {}",
        problems[0]
    );

    let _ = std::fs::remove_dir_all(&dir);
}
