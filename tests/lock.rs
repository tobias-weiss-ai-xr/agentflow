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
