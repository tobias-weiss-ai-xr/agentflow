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
// spec: lifecycle/execute-pipeline
// spec: scheduling/dependency-dag
// spec: worktree/worktree-lifecycle
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
// spec: state/cost-receipts
// spec: lifecycle/prompt-rendering
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
    assert!(receipts
        .iter()
        .all(|r| r.outcome == "failed" && r.worker == "w1"));
    // Retry memory (ADR-13): the final (attempt-3) prompt render lists the
    // earlier failures.
    let prompt = std::fs::read_to_string(f.st.state_dir.join("prompts").join("A.md"))
        .expect("prompt file exists");
    assert!(prompt.contains("Previous attempts on this task"));
    assert!(prompt.contains("attempt 1:"));
    assert!(prompt.contains("attempt 2:"));
}

/// Agent exits non-zero → task fails after max_attempts, nothing merged.
// spec: lifecycle/execute-pipeline
// spec: scheduling/deadlock-detection
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
// spec: lifecycle/execute-pipeline
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
// spec: cli/run-commands
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
    let code = run::run_loop(
        &f.cfg,
        &f.st,
        &RunOptions {
            dry_run: true,
            ..Default::default()
        },
    );
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
// spec: scheduling/retry-with-fresh-branch
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
// spec: state/resume-and-self-heal
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
    assert_eq!(
        st["A"].state,
        TaskState::Done,
        "stale running healed + completed"
    );
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
        &RunOptions {
            task_filter: Some("A".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        code, 0,
        "in-scope completion must not hang on out-of-scope B"
    );
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(!st.contains_key("B"), "B must not have been dispatched");
}

/// --task naming a task that waits on out-of-scope deps → clean deadlock, no hang.
// spec: scheduling/deadlock-detection
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
        &RunOptions {
            task_filter: Some("B".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        code, 2,
        "B waits on out-of-scope A → deadlock exit, no hang"
    );
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
        &RunOptions {
            worker_filter: Some("nope".into()),
            ..Default::default()
        },
    );
    assert_eq!(code, 2);
    assert!(!f.st.worktree_root.join("A").exists(), "nothing ran");
}

/// Sandbox layer 1: the agent child sees ONLY the dispatched worker's
/// api key (+ passthrough) — not other secrets from the orchestrator env.
/// The same probe also pins the git hygiene pairs every agent child gets.
// spec: sandbox/agent-environment-allowlist
// spec: sandbox/git-hygiene-for-agent-children
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
        "AF_TEST_KEY,AF_TEST_LEAK,AF_TEST_EXTRA,\
         GIT_TERMINAL_PROMPT,GIT_CONFIG_COUNT,GIT_CONFIG_KEY_0,GIT_CONFIG_VALUE_0",
    );
    // The probe vars must ride the passthrough — they are not in the default
    // allowlist (which is exactly the behavior under test).
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "AF_TEST_EXTRA,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let env_txt = std::fs::read_to_string(&probe).unwrap();
    assert!(
        env_txt.contains("AF_TEST_KEY=keyvalue-123"),
        "worker key visible: {env_txt}"
    );
    assert!(
        env_txt.contains("AF_TEST_EXTRA=extra-789"),
        "passthrough visible"
    );
    assert!(
        env_txt.contains("AF_TEST_LEAK=<unset>"),
        "foreign secret stripped: {env_txt}"
    );
    // Git hygiene (sandbox spec): no credential prompts, no stored helpers.
    assert!(
        env_txt.contains("GIT_TERMINAL_PROMPT=0"),
        "terminal prompt disabled: {env_txt}"
    );
    assert!(
        env_txt.contains("GIT_CONFIG_COUNT=1")
            && env_txt.contains("GIT_CONFIG_KEY_0=credential.helper")
            && env_txt.contains("GIT_CONFIG_VALUE_0=\n"),
        "empty credential.helper injected via GIT_CONFIG_*: {env_txt}"
    );
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
// spec: worktree/worktrees-target-the-task-s-repository
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
        &format!(
            r#"{{ "workers": [{{"name":"w1","provider":"p","model":"m","cli":"{}"}}]}}"#,
            AGENT.replace('\\', "\\\\")
        ),
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
    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .unwrap();
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
// spec: config/per-task-repo-resolution-with-compat-fallback
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
    assert!(
        f.repo.join("DONE.txt").exists(),
        "fell back to default repo"
    );
}

/// Parallel multi-worker dispatch (scheduling spec: disjoint scope dispatches
/// in parallel): two INDEPENDENT tasks run CONCURRENTLY on two DISTINCT
/// workers and both merge to main.
///
/// Concurrency is proven from the orchestrator's OWN persisted state: while
/// `run_loop` runs on a background thread we poll `run-state.json` and record
/// that task A and task B are BOTH in the `running` state at the same instant.
/// Only the real parallel dispatcher (2 free workers, max_parallel=2, disjoint
/// scopes) can ever hold two tasks running simultaneously — a one-slot
/// dispatcher would finish A before ever dispatching B. The agents sleep a fixed
/// `FAKE_AGENT_SLEEP_MS` so the concurrent-running window is comfortably long
/// enough to observe. Each worker then merges a distinct `{model}.txt` artifact.
// spec: scheduling/scope-contention-avoidance
// spec: worktree/merge-serialization
#[test]
fn parallel_multi_worker_dispatch_runs_concurrently_and_merges() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"alpha","scope":["alpha.txt"],"accept":"{ga}"}},
                {{"id":"B","title":"beta","scope":["beta.txt"],"accept":"{gb}"}}
            ] }}"#,
            ga = gate_cmd("alpha.txt"),
            gb = gate_cmd("beta.txt"),
        ),
        // Two ENABLED workers with distinct models (distinct output files).
        &format!(
            r#"{{ "defaults": {{ "max_attempts": 1, "accept_timeout_s": 10 }},
                 "workers": [
                    {{"name":"w1","provider":"p","model":"alpha","enabled":true,"cli":"{agent}"}},
                    {{"name":"w2","provider":"p","model":"beta","enabled":true,"cli":"{agent}"}}
                 ] }}"#,
            agent = AGENT.replace('\\', "\\\\")
        ),
    );
    // The stub knobs must ride the sandbox env allowlist (ADR-10); the
    // fixture's default passthrough does not include them, so override it.
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_TOUCH_FROM_MODEL,FAKE_AGENT_SLEEP_MS",
    );
    std::env::set_var("FAKE_AGENT_TOUCH_FROM_MODEL", "1");
    std::env::set_var("FAKE_AGENT_SLEEP_MS", "1200");

    // Drive the orchestrator on a background thread so we can watch the
    // persisted state from this thread while it runs.
    let cfg = f.cfg.clone();
    let st = f.st.clone();
    let (tx, rx) = std::sync::mpsc::channel::<i32>();
    std::thread::spawn(move || {
        let code = run::run_loop(&cfg, &st, &RunOptions::default());
        let _ = tx.send(code);
    });

    // CONCURRENT: poll the persisted state until BOTH tasks are running at the
    // same instant. A serial dispatcher can never satisfy this.
    let store = Store::new(f.st.state_dir.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut saw_concurrent = false;
    while std::time::Instant::now() < deadline {
        let m = store.load();
        let a_running = m
            .get("A")
            .map(|s| s.state == TaskState::Running)
            .unwrap_or(false);
        let b_running = m
            .get("B")
            .map(|s| s.state == TaskState::Running)
            .unwrap_or(false);
        if a_running && b_running {
            saw_concurrent = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let code = rx
        .recv_timeout(std::time::Duration::from_secs(40))
        .expect("run completed");
    assert_eq!(code, 0, "run_loop must exit 0 (all done)");
    assert!(
        saw_concurrent,
        "A and B were never observed running simultaneously (parallel dispatch required)"
    );

    // Both tasks Done; both distinct artifacts merged to main.
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(st["B"].state, TaskState::Done);
    assert!(f.repo.join("alpha.txt").exists(), "A merged (alpha.txt)");
    assert!(f.repo.join("beta.txt").exists(), "B merged (beta.txt)");

    // DISTINCT workers: one receipt per merged task, spread across w1 and w2.
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 2, "one receipt per merged task");
    let wa = receipts
        .iter()
        .find(|r| r.task == "A")
        .map(|r| r.worker.as_str())
        .unwrap();
    let wb = receipts
        .iter()
        .find(|r| r.task == "B")
        .map(|r| r.worker.as_str())
        .unwrap();
    assert_ne!(wa, wb, "A and B ran on different workers (got {wa} / {wb})");
    assert!(
        (wa == "w1" && wb == "w2") || (wa == "w2" && wb == "w1"),
        "unexpected workers: {wa} / {wb}"
    );
    // No leftover worktrees for either task.
    assert!(!f.st.worktree_root.join("A").exists());
    assert!(!f.st.worktree_root.join("B").exists());
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
    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .unwrap();
    let cfg = config::Config {
        repos: agentflow::config::load_repos(&config_dir.join("repos.json")).unwrap(),
        ..cfg
    };
    assert_eq!(run::run_loop(&cfg, &f.st, &RunOptions::default()), 2);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.as_deref().unwrap_or("");
    assert!(
        err.contains("not a git repository"),
        "reason surfaced: {err}"
    );
}

// ---------------------------------------------------------------------------
// Spec-coverage gap tests (see docs/spec-traceability.md).
// ---------------------------------------------------------------------------

/// The single subprocess helper every child runs through: output captured,
/// hard timeout enforced (the child is killed, not awaited), and results
/// classified by exit code — success / non-zero / timeout / missing binary.
// spec: lifecycle/subprocess-execution-contract
#[test]
fn subprocess_helper_times_out_and_classifies() {
    use agentflow::subprocess::{self, CmdKind, EnvMode};
    use std::time::Duration;

    let args = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };

    // Success: exit 0, stdout captured.
    let out = subprocess::run(
        "git",
        &args(&["--version"]),
        None,
        &[],
        EnvMode::Inherit,
        Duration::from_secs(30),
    );
    assert!(out.passed(), "git --version must succeed");
    assert!(
        out.stdout.contains("git version"),
        "stdout captured: {}",
        out.stdout
    );

    #[cfg(unix)]
    {
        // Non-zero exit classified by code.
        let out = subprocess::run(
            "sh",
            &args(&["-c", "exit 7"]),
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(30),
        );
        assert_eq!(out.kind, CmdKind::NonZero);
        assert_eq!(out.code, Some(7));

        // Hard timeout: a child exceeding it is killed promptly — the
        // helper returns Timeout instead of waiting out the sleep.
        let start = std::time::Instant::now();
        let out = subprocess::run(
            "sh",
            &args(&["-c", "sleep 30"]),
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_millis(300),
        );
        assert_eq!(out.kind, CmdKind::Timeout, "timeout kills the child");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "must not wait out the child's full 30s sleep"
        );
    }

    // Missing binary classified Missing (never a hang or a panic).
    let out = subprocess::run(
        "af-no-such-binary-xyz",
        &[],
        None,
        &[],
        EnvMode::Inherit,
        Duration::from_secs(5),
    );
    assert_eq!(out.kind, CmdKind::Missing);
}

/// Sandbox wrapper hook (sandbox spec): `TF_SANDBOX_CMD` is a command prefix
/// prepended to the agent argv. A wrapper script records the argv it was
/// invoked with and exits 0 without creating the gate's file — the attempt
/// fails the gate, but the recording proves the wrapper ran first. (The
/// unset-wrapper case is a no-op, proven by every other test in this file:
/// agents are invoked directly and their work merges.)
// spec: sandbox/sandbox-wrapper-hook
#[cfg(unix)]
#[test]
fn sandbox_wrapper_cmd_is_prepended_to_agent_argv() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let mut f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"wrapped","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );

    let wrapper = f.dir.join("wrap.sh");
    let argv_file = f.dir.join("argv.txt");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nexit 0\n",
            argv_file.to_string_lossy()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    f.st.sandbox_cmd = vec![wrapper.to_string_lossy().to_string()];

    // The wrapper exits 0 but never creates DONE.txt → the gate fails the
    // attempt (max_attempts=1) → run exits 2. That is expected: this test
    // asserts the argv, not a merge.
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 2);

    let argv = std::fs::read_to_string(&argv_file).expect("wrapper recorded its argv");
    let mut lines = argv.lines();
    let first = lines.next().unwrap_or_default();
    assert_eq!(
        first, AGENT,
        "child argv starts with the wrapper, then the agent CLI: {argv}"
    );
    assert!(
        argv.contains("-p"),
        "agent args ride along after the wrapper: {argv}"
    );
}

/// Startup self-heal spans all repositories (worktree spec): a stale
/// worktree dir + branch belonging to repo `aux` is removed even though the
/// default repo is a different one, and the run continues cleanly.
// spec: worktree/self-heal-spans-all-repositories
#[test]
fn self_heal_removes_stale_worktrees_across_repos() {
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

    // Second scratch repo `aux` with a REAL stale worktree (branch tf/OLD)
    // at the orchestrator's worktree root — residue of a dead attempt.
    let auxrepo = f.dir.join("auxrepo");
    std::fs::create_dir_all(&auxrepo).unwrap();
    git(&auxrepo, &["init", "-b", "main"]);
    git(&auxrepo, &["commit", "--allow-empty", "-m", "init"]);
    let stale_wt = f.st.worktree_root.join("OLD");
    git(
        &auxrepo,
        &[
            "worktree",
            "add",
            stale_wt.to_string_lossy().as_ref(),
            "-b",
            "tf/OLD",
        ],
    );
    assert!(stale_wt.exists(), "stale worktree seeded");

    // repos.json registers both repos; reload so cfg picks it up.
    let config_dir = f.dir.join("config");
    std::fs::write(
        config_dir.join("repos.json"),
        format!(
            r#"{{"repos": {{"main": "{}", "aux": "{}"}}}}"#,
            f.repo.to_string_lossy().replace('\\', "\\\\"),
            auxrepo.to_string_lossy().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .unwrap();
    let cfg = config::Config {
        repos: agentflow::config::load_repos(&config_dir.join("repos.json")).unwrap(),
        ..cfg
    };

    // Task A already finished in a previous run: the next startup only
    // self-heals (remove the orphan, delete its branch) and exits 0.
    std::fs::create_dir_all(&f.st.state_dir).unwrap();
    std::fs::write(
        f.st.state_dir.join("run-state.json"),
        r#"{ "A": { "state": "done", "attempts": 1, "last_error": null } }"#,
    )
    .unwrap();
    assert_eq!(run::run_loop(&cfg, &f.st, &RunOptions::default()), 0);

    assert!(
        !stale_wt.exists(),
        "stale worktree removed even though it belongs to repo `aux`"
    );
    let out = std::process::Command::new("git")
        .args(["branch", "--list", "tf/OLD"])
        .current_dir(&auxrepo)
        .output()
        .expect("git must be available");
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "aux's stale branch tf/OLD deleted"
    );
}
