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
    #[allow(dead_code)] // kept for debugging failed assertions
    dir: PathBuf,
    repo: PathBuf,
    cfg: config::Config,
    st: Settings,
}

fn fixture(tasks_json: &str, workers_json: &str) -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The sandbox strips the agent child's env; the example_agent knobs must
    // ride TF_AGENT_ENV_PASSTHROUGH (tests that override it set their own).
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
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
        sandbox_cmd: vec![],
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
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
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
    // Receipts were appended (one per merged task), outcome "merged".
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert!(receipts.len() >= 2, "one receipt per merged task");
    assert!(receipts.iter().all(|r| r.outcome == "merged"));
    // First-attempt prompts carry no retry history (ADR-13).
    let prompt = std::fs::read_to_string(f.st.state_dir.join("prompts").join("B.md"))
        .expect("prompt file exists");
    assert!(!prompt.contains("Previous attempts"));
}

/// Failed attempts get receipts too — the routing substrate (ADR-12).
#[test]
fn failed_attempts_get_failed_receipts() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("FAKE_AGENT_EXIT", "7");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(3),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 2);
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 3, "one receipt per failed attempt");
    assert!(receipts.iter().all(|r| r.outcome == "failed" && r.worker == "w1"));
    // Retry memory (ADR-13): the final (attempt-3) prompt render lists the
    // earlier failures.
    let prompt = std::fs::read_to_string(f.st.state_dir.join("prompts").join("A.md"))
        .expect("prompt file exists");
    assert!(prompt.contains("Previous attempts on this task"));
    assert!(prompt.contains("attempt 1:"));
    assert!(prompt.contains("attempt 2:"));
}

/// Agent exits non-zero → task fails after max_attempts, nothing merged.
#[test]
fn agent_failure_fails_task() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
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
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
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

/// Retry: agent always fails, max_attempts=2 → exactly 2 attempts, then Failed.
#[test]
fn retry_runs_up_to_max_attempts() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("FAKE_AGENT_EXIT", "7");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(2),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 2);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    assert_eq!(st["A"].attempts, 2, "must have retried on a fresh attempt");
}

/// Self-heal: a stale `running` entry from a dead process is reset and run.
#[test]
fn self_heals_stale_running_state() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    // Pre-seed the state file as if a previous process died mid-run.
    std::fs::create_dir_all(&f.st.state_dir).unwrap();
    std::fs::write(
        f.st.state_dir.join("run-state.json"),
        r#"{ "A": { "state": "running", "attempts": 0, "last_error": null } }"#,
    )
    .unwrap();
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done, "stale running healed + completed");
    assert!(f.repo.join("DONE.txt").exists());
}

/// --task filter: only the named task runs; out-of-scope work never blocks.
#[test]
fn task_filter_completes_without_running_others() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"run me","scope":["DONE.txt"],"accept":"{g}"}},
                {{"id":"B","title":"not in scope","scope":["DONE.txt"],"accept":"{g}"}}
            ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    let code = run::run_loop(
        &f.cfg,
        &f.st,
        &RunOptions { task_filter: Some("A".into()), ..Default::default() },
    );
    assert_eq!(code, 0, "in-scope completion must not hang on out-of-scope B");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(!st.contains_key("B"), "B must not have been dispatched");
}

/// --task naming a task that waits on out-of-scope deps → clean deadlock, no hang.
#[test]
fn task_filter_on_dependent_task_deadlocks_cleanly() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"base","scope":["DONE.txt"],"accept":"{g}"}},
                {{"id":"B","title":"depends on A","deps":["A"],"scope":["DONE.txt"],"accept":"{g}"}}
            ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    let code = run::run_loop(
        &f.cfg,
        &f.st,
        &RunOptions { task_filter: Some("B".into()), ..Default::default() },
    );
    assert_eq!(code, 2, "B waits on out-of-scope A → deadlock exit, no hang");
}

/// --worker naming no enabled worker fails fast instead of hanging.
#[test]
fn unknown_worker_filter_fails_fast() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    let code = run::run_loop(
        &f.cfg,
        &f.st,
        &RunOptions { worker_filter: Some("nope".into()), ..Default::default() },
    );
    assert_eq!(code, 2);
    assert!(!f.st.worktree_root.join("A").exists(), "nothing ran");
}

/// Sandbox layer 1: the agent child sees ONLY the dispatched worker's
/// api key (+ passthrough) — not other secrets from the orchestrator env.
#[test]
fn agent_env_is_allowlisted() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("AF_TEST_KEY", "keyvalue-123");
    std::env::set_var("AF_TEST_LEAK", "leak-456");
    std::env::set_var("TF_AGENT_ENV_PASSTHROUGH", "AF_TEST_EXTRA");
    std::env::set_var("AF_TEST_EXTRA", "extra-789");
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &format!(
            r#"{{ "workers": [{{"name":"w1","provider":"p","model":"m","api_key_env":"AF_TEST_KEY","cli":"{agent}"}}]}}"#,
            agent = AGENT.replace('\\', "\\\\")
        ),
    );
    let probe = f.dir.join("env.txt");
    std::env::set_var("FAKE_AGENT_ENV", &probe);
    std::env::set_var(
        "FAKE_AGENT_ENV_NAMES",
        "AF_TEST_KEY,AF_TEST_LEAK,AF_TEST_EXTRA",
    );
    // The probe vars must ride the passthrough — they are not in the default
    // allowlist (which is exactly the behavior under test).
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "AF_TEST_EXTRA,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let env_txt = std::fs::read_to_string(&probe).unwrap();
    assert!(env_txt.contains("AF_TEST_KEY=keyvalue-123"), "worker key visible: {env_txt}");
    assert!(env_txt.contains("AF_TEST_EXTRA=extra-789"), "passthrough visible");
    assert!(env_txt.contains("AF_TEST_LEAK=<unset>"), "foreign secret stripped: {env_txt}");
    for k in [
        "AF_TEST_KEY",
        "AF_TEST_LEAK",
        "TF_AGENT_ENV_PASSTHROUGH",
        "AF_TEST_EXTRA",
        "FAKE_AGENT_ENV",
        "FAKE_AGENT_ENV_NAMES",
    ] {
        std::env::remove_var(k);
    }
}

/// Multi-repo (ADR-11): A on repo `main`, B on repo `auxrepo` (deps: A) — each
/// task's worktree/branch/merge lands in its own repo.
#[test]
fn multi_repo_campaign_merges_into_each_repo() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::set_var("FAKE_AGENT_TOUCH", "DONE.txt");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"main repo","repo":"main","scope":["DONE.txt"],"accept":"{g}"}},
                {{"id":"B","title":"auxrepo change","repo":"auxrepo","deps":["A"],"scope":["DONE.txt"],"accept":"{g}"}}
            ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &format!(r#"{{ "workers": [{{"name":"w1","provider":"p","model":"m","cli":"{}"}}]}}"#, AGENT.replace('\\', "\\\\")),
    );
    // Second scratch repo + repos.json next to tasks.json.
    let auxrepo = f.dir.join("auxrepo");
    std::fs::create_dir_all(&auxrepo).unwrap();
    git(&auxrepo, &["init", "-b", "main"]);
    // af worktrees branch off HEAD — an unborn repo has none.
    git(&auxrepo, &["commit", "--allow-empty", "-m", "init"]);
    let config_dir = f.dir.join("config");
    std::fs::write(
        config_dir.join("repos.json"),
        format!(
            r#"{{"repos": {{"main": "{}", "auxrepo": "{}"}}}}"#,
            f.dir.join("repo").to_string_lossy().replace('\\', "\\\\"),
            auxrepo.to_string_lossy().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    // Re-load so cfg picks up repos.json (fixture loaded before it existed).
    let cfg = config::load(&config_dir.join("tasks.json"), &config_dir.join("workers.json")).unwrap();
    let cfg = config::Config {
        repos: agentflow::config::load_repos(&config_dir.join("repos.json")).unwrap(),
        ..cfg
    };
    assert_eq!(run::run_loop(&cfg, &f.st, &RunOptions::default()), 0);
    assert!(f.repo.join("DONE.txt").exists(), "A merged into main repo");
    assert!(auxrepo.join("DONE.txt").exists(), "B merged into aux repo");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(st["B"].state, TaskState::Done);
}

/// Unknown repo name: warn + fall back to the default repo, run completes.
#[test]
fn unknown_repo_warns_and_falls_back() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","repo":"nowhere","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    assert!(f.repo.join("DONE.txt").exists(), "fell back to default repo");
}

/// Task on a repo that exists on disk but is not a git repo: the attempt
/// fails cleanly (worktree error arm), task goes Failed with the reason.
#[test]
fn task_on_broken_repo_fails_cleanly() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"x","repo":"broken","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    // "broken" exists but was never git-inited.
    std::fs::create_dir_all(f.dir.join("broken")).unwrap();
    let config_dir = f.dir.join("config");
    let bp = f.dir.join("broken").to_string_lossy().replace('\\', "/");
    std::fs::write(
        config_dir.join("repos.json"),
        format!(r#"{{"repos": {{"broken": "{}"}}}}"#, bp),
    )
    .unwrap();
    let cfg = config::load(&config_dir.join("tasks.json"), &config_dir.join("workers.json")).unwrap();
    let cfg = config::Config {
        repos: agentflow::config::load_repos(&config_dir.join("repos.json")).unwrap(),
        ..cfg
    };
    assert_eq!(run::run_loop(&cfg, &f.st, &RunOptions::default()), 2);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.as_deref().unwrap_or("");
    assert!(err.contains("not a git repository"), "reason surfaced: {err}");
}

