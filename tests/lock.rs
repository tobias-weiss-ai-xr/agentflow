//! Single-writer lock tests: two `af run` processes must never share one
//! TF_STATE_DIR (pi-durable's "one process owns a storage at a time").

use agentflow::state::Store;
use std::path::PathBuf;

fn tmpdir() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("af-lock-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// spec: state/single-writer-state-lock
// spec: state/single-writer-state-lock#lock-is-released-on-drop
// spec: state/single-writer-state-lock#stale-lock-is-reclaimed
#[test]
fn single_writer_lock_is_exclusive() {
    let dir = tmpdir();
    let a = Store::new(dir.clone());
    let b = Store::new(dir.clone());

    // First writer owns the state dir.
    let guard = a.acquire_lock().expect("first acquire must succeed");

    // A second writer over the same dir is rejected while the first is alive.
    assert!(
        b.acquire_lock().is_err(),
        "second acquire must fail while the lock is held"
    );

    // Releasing (Drop) frees the dir for the next writer.
    drop(guard);
    let guard2 = b.acquire_lock().expect("lock must be released on Drop");
    drop(guard2);

    // Stale lock: a pid that has already been reaped must be reclaimed.
    let mut child = std::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .expect("spawn short-lived child");
    let pid = child.id();
    child.wait().expect("reap child");
    std::fs::write(dir.join(".lock"), format!("{pid}\n")).unwrap();

    let guard3 = a
        .acquire_lock()
        .expect("stale lock (dead pid) must be reclaimed");
    drop(guard3);
}

/// The spec scenario: a second writer over a locked state dir is rejected
/// with an error NAMING THE OWNING PID (so the user can go kill it), and
/// `af run` surfaces that as exit 2 (pinned by the config-error path in
/// `run_loop`; here we pin the error text itself).
// spec: state/single-writer-state-lock#second-writer-is-rejected-naming-the-owner
#[test]
fn second_lock_rejection_names_the_owning_pid() {
    let dir = tmpdir();
    let a = Store::new(dir.clone());
    let b = Store::new(dir.clone());
    let _guard = a.acquire_lock().expect("first acquire must succeed");

    let err = b
        .acquire_lock()
        .expect_err("second acquire must fail while the first is live");
    let msg = err.to_string();
    assert!(
        msg.contains(&std::process::id().to_string()),
        "error names the owning pid ({}): {msg}",
        std::process::id()
    );
    assert!(
        msg.contains("af owns this state dir"),
        "error says another af owns the dir: {msg}"
    );
}
