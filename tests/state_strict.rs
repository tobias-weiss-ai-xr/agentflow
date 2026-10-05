//! A corrupt `run-state.json` must be an explicit error, never a silent fresh
//! campaign.
//!
//! The old `Store::load` was `from_str(..).unwrap_or_default()`: a torn or
//! truncated ledger became an empty map, so every `done` task looked `ready`
//! and `af run` re-bought the whole campaign. `load_checked` makes the read
//! fallible and the orchestrator refuses to start on it.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::{Store, TaskStatus};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("af-strict-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn done_status() -> TaskStatus {
    TaskStatus {
        state: TaskState::Done,
        attempts: 1,
        last_error: None,
        phase: None,
    }
}

/// Minimal valid config + settings built through `config::load`. The state
/// dir is NOT created here; callers populate it.
fn fixture(dir: &Path) -> (config::Config, Settings) {
    let cfg_dir = dir.join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let tasks = cfg_dir.join("tasks.json");
    let workers = cfg_dir.join("workers.json");
    std::fs::write(
        &tasks,
        r#"{ "tasks": [ {"id":"T1","title":"x","accept":"true"} ] }"#,
    )
    .unwrap();
    std::fs::write(
        &workers,
        r#"{ "workers": [ {"name":"w1","provider":"openai","model":"gpt-4o","enabled":true} ] }"#,
    )
    .unwrap();
    let cfg = config::load(&tasks, &workers).expect("minimal config loads");
    let st = Settings {
        repo_dir: dir.join("repo"),
        state_dir: dir.join("state"),
        worktree_root: dir.join("wt"),
        max_parallel: 1,
        branch_prefix: "tf".into(),
        poll_secs: 1,
        gate_env: vec![],
        tasks_file: tasks,
        workers_file: workers,
        prompt_file: dir.join("no-template.md"),
        agent_timeout_s: 60,
        sandbox_cmd: vec![],
    };
    (cfg, st)
}

/// `run-state.json.tmp*` files left in `dir` (they must never survive a save).
fn temp_leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("run-state.json.tmp"))
        .collect()
}

#[test]
fn corrupt_state_is_an_error_not_an_empty_campaign() {
    let dir = tmpdir("corrupt");
    let (cfg, st) = fixture(&dir);
    std::fs::create_dir_all(&st.state_dir).unwrap();
    let store = Store::new(st.state_dir.clone());
    let path = store.status_file();

    // (a) A missing state file is a legitimate fresh campaign, not an error.
    assert!(
        store.load_checked().expect("missing file is Ok").is_empty(),
        "a fresh campaign has no state yet"
    );

    // (b) A valid file round-trips, including a Done task.
    let mut good = HashMap::new();
    good.insert("T1".to_string(), done_status());
    store.save(&good).unwrap();
    assert_eq!(store.load_checked().unwrap(), good);

    // (c) A truncated/garbage state file returns Err that names the path,
    // NOT Ok(empty). The read-only display path stays loud but empty.
    let valid = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, &valid[..valid.len() / 2]).unwrap();
    let err = store
        .load_checked()
        .expect_err("a truncated ledger must be an error, not Ok(empty)");
    let msg = err.to_string();
    assert!(
        msg.contains("run-state.json"),
        "the error names the offending file: {msg}"
    );
    assert!(
        store.load().is_empty(),
        "read-only load degrades to empty for display — but only after warning"
    );

    // A syntactically valid document that is not a task map is also an error.
    std::fs::write(&path, "[1, 2, 3]").unwrap();
    assert!(
        store.load_checked().is_err(),
        "a non-map document is not an empty campaign"
    );

    // A state path that exists but cannot be read (here: a directory) is an
    // error too, not a fresh campaign.
    let unreadable = tmpdir("unreadable");
    std::fs::create_dir_all(unreadable.join("run-state.json")).unwrap();
    let s_unreadable = Store::new(unreadable.clone());
    assert!(s_unreadable.load_checked().is_err());
    let _ = std::fs::remove_dir_all(&unreadable);

    // (d) The decisive one: the orchestrator refuses to re-dispatch and exits
    // 2 over a corrupt ledger, so it can never silently re-buy the campaign.
    std::fs::write(&path, "{\"T1\": {\"state\":").unwrap();
    assert_eq!(
        run::run_loop(&cfg, &st, &RunOptions::default()),
        2,
        "run_loop must refuse to start on a corrupt ledger"
    );
    assert_eq!(
        run::dry_run(&cfg, &st),
        2,
        "the displayed plan must agree with the real run"
    );
    // The corrupt file is evidence and must be left exactly as it was.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"T1\": {\"state\":",
        "the unreadable ledger is preserved for repair"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A failed atomic install returns Err and cleans up its temp file rather than
/// leaking a partial ledger. Pointing the status path at a directory makes the
/// final rename fail.
#[test]
fn failed_save_returns_err_and_removes_its_temp_file() {
    let dir = tmpdir("save-fail");
    std::fs::create_dir_all(dir.join("run-state.json")).unwrap();
    let store = Store::new(dir.clone());
    let mut m = HashMap::new();
    m.insert("T1".to_string(), done_status());

    assert!(
        store.save(&m).is_err(),
        "installing over a directory must fail"
    );
    assert!(
        temp_leftovers(&dir).is_empty(),
        "a failed save must not leak its temp file: {:?}",
        temp_leftovers(&dir)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
