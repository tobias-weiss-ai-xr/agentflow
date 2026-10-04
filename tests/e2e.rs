//! E2E tests: full `af run` against a scratch git repository and the
//! bundled `example_agent` binary. No network, no LLM — deterministic CI.
//!
//! These tests mutate process-global env vars used by example_agent, so they
//! share one mutex and run logically serialized.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::Store;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static ENV_GUARD: Mutex<()> = Mutex::new(());
const AGENT: &str = env!("CARGO_BIN_EXE_example_agent");

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git must be available");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
    cfg: config::Config,
    st: Settings,
}

fn fixture(tasks_json: &str, workers_json: &str) -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-e2e-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    // Identity for commits/merges (inherited by child git processes).
    std::env::set_var("GIT_AUTHOR_NAME", "af test");
    std::env::set_var("GIT_AUTHOR_EMAIL", "af@test");
    std::env::set_var("GIT_COMMITTER_NAME", "af test");
    std::env::set_var("GIT_COMMITTER_EMAIL", "af@test");
    std::fs::write(repo.join("README.md"), "# scratch\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);

    let config_dir = dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("tasks.json"), tasks_json).unwrap();
    std::fs::write(config_dir.join("workers.json"), workers_json).unwrap();

    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .unwrap();
    let st = Settings {
        repo_dir: repo.clone(),
        state_dir: dir.join("state"),
        worktree_root: dir.join("wt"),
        max_parallel: 2,
        branch_prefix: "tf".into(),
        poll_secs: 1,
        gate_env: vec![],
        tasks_file: config_dir.join("tasks.json"),
        workers_file: config_dir.join("workers.json"),
        prompt_file: dir.join("no-template.md"),
        agent_timeout_s: 60,
    };
    Fixture { dir, repo, cfg, st }
}

fn gate_cmd(file: &str) -> String {
    #[cfg(windows)]
    {
        format!("if exist {file} (exit 0) else (exit 1)")
    }
    #[cfg(not(windows))]
    {
        format!("test -f {file}")
    }
}

fn worker_json(max_attempts: u32) -> String {
    // Windows paths contain backslashes — escape them for JSON.
    let agent_escaped = AGENT.replace('\\', "\\\\");
    format!(
        r#"{{ "defaults": {{ "max_attempts": {max_attempts}, "accept_timeout_s": 10 }},
            "workers": [ {{ "name": "w1", "provider": "openai", "model": "gpt-4o",
                            "enabled": true, "cli": "{agent_escaped}" }} ] }}"#
    )
}

/// Gate exit-0 after agent writes DONE.txt → merged to main, all done.
#[test]
fn happy_path_dependency_and_merge() {
    let _g = ENV_GUARD.lock().unwrap();
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"create done","scope":["DONE.txt"],"accept":"{g}"}},
                {{"id":"B","title":"follow on","deps":["A"],"scope":["DONE.txt"],"accept":"{g}"}}
            ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "run should exit 0 (all done)");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(st["B"].state, TaskState::Done);
    // Merged artifact exists on the base repo.
    assert!(f.repo.join("DONE.txt").exists(), "DONE.txt merged to main");
    // No leftover worktree.
    assert!(!f.st.worktree_root.join("A").exists());
    // Receipts were appended (one per merged task).
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert!(receipts.len() >= 2, "one receipt per merged task");
}

/// Agent exits non-zero → task fails after max_attempts, nothing merged.
#[test]
fn agent_failure_fails_task() {
    let _g = ENV_GUARD.lock().unwrap();
    std::env::set_var("FAKE_AGENT_EXIT", "7");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"boom","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1), // max_attempts=1 → no retry loop
    );
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "deadlock/failure → exit 2");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    assert!(!f.repo.join("DONE.txt").exists(), "nothing merged");
}

/// Agent succeeds but gate fails → task fails, nothing merged.
#[test]
fn gate_failure_fails_task() {
    let _g = ENV_GUARD.lock().unwrap();
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"gate blocks","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("NEVER.txt") // gate demands a file the agent won't create
        ),
        &worker_json(1),
    );
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    assert!(st["A"].last_error.as_deref().unwrap_or("").contains("gate"));
    assert!(!f.repo.join("DONE.txt").exists());
}

/// --dry-run changes nothing.
#[test]
fn dry_run_changes_nothing() {
    let _g = ENV_GUARD.lock().unwrap();
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions { dry_run: true, ..Default::default() });
    assert_eq!(code, 0);
    assert!(!f.st.worktree_root.exists(), "no worktrees created");
    assert!(!f.repo.join("DONE.txt").exists(), "no merge happened");
}

/// Status board is machine-readable as JSON after a run.
#[test]
fn status_json_after_run() {
    let _g = ENV_GUARD.lock().unwrap();
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let json = run::status_json(&f.cfg, &f.st);
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(v["A"]["state"], "done");
}
