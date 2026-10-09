//! E2E tests: full `af run` against a scratch git repository and the
//! bundled `example_agent` binary. No network, no LLM — deterministic CI.
//!
//! These tests mutate process-global env vars used by example_agent, so they
//! share one mutex and run logically serialized.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::{Receipt, Store};
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
        agent_max_turns: 0,
        tasks_file: config_dir.join("tasks.json"),
        workers_file: config_dir.join("workers.json"),
        prompt_file: dir.join("no-template.md"),
        agent_timeout_s: 60,
        agent_stall_s: 0,
        max_wall_clock_s: 0,
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

/// Same worker, dispatched through the `command` shell-template path
/// (GOWORKER parity): `{prompt}` → absolute prompt path, run via the
/// platform shell inside the worktree.
fn worker_json_command(max_attempts: u32) -> String {
    let agent_escaped = AGENT.replace('\\', "\\\\");
    format!(
        r#"{{ "defaults": {{ "max_attempts": {max_attempts}, "accept_timeout_s": 10 }},
            "workers": [ {{ "name": "w1", "provider": "openai", "model": "gpt-4o",
                            "enabled": true, "command": "\"{agent_escaped}\" {{prompt}}" }} ] }}"#
    )
}

/// Write a shell "worker" that first runs the real stub to WRITE AND COMMIT
/// its work, then re-execs the stub with `FAKE_AGENT_HANG_MS` armed so the
/// attempt commits real work and THEN stalls silently. `exec` replaces the
/// shell with the hanging stub, so the watchdog's kill reaches the process it
/// measured (no orphaned grandchild still editing the worktree). The stub's
/// env knobs must ride `TF_AGENT_ENV_PASSTHROUGH` — the agent child's env is
/// stripped otherwise and the test would silently measure nothing.
#[cfg(unix)]
fn work_then_hang_agent(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("work-then-hang.sh");
    let body = format!(
        "#!/bin/sh\n\"{real}\" \"$@\"\nexport FAKE_AGENT_HANG_MS=2000\nexec \"{real}\" \"$@\"\n",
        real = AGENT
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// Every `tf/A-rejected-*` branch in `repo`, sorted. The archived form every
/// preserved failure path uses (`<branch>-rejected-<now>`).
fn archived_refs(repo: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["branch", "--list", "tf/A-rejected-*"])
        .current_dir(repo)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// The archived branch's committed tree contains `file` with `content`,
/// proving the preserved ref really carries the agent's work.
fn archived_holds(repo: &Path, branch: &str, file: &str, content: &str) {
    let out = std::process::Command::new("git")
        .args(["show", &format!("{branch}:{file}")])
        .current_dir(repo)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "archived branch {branch} must contain {file}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), content);
}

/// The original attempt branch is gone, so a retry starts clean from base.
fn original_branch_gone(repo: &Path) {
    let gone = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/tf/A"])
        .current_dir(repo)
        .output()
        .expect("git runs");
    assert!(
        !gone.status.success(),
        "original branch tf/A must be removed so a retry starts clean"
    );
}

/// Gate exit-0 after agent writes DONE.txt → merged to main, all done.
// spec: lifecycle/execute-pipeline
// spec: lifecycle/execute-pipeline#happy-path
// spec: cli/run-commands
// spec: cli/run-commands#full-run-completes
// spec: scheduling/dependency-dag
// spec: scheduling/dependency-dag#dependency-ordering
// spec: scheduling/deadlock-detection#no-deadlock-while-progress-possible
// spec: lifecycle/prompt-rendering#first-attempt-has-no-history-block
// spec: state/cost-receipts#receipt-appended-per-attempt
// spec: worktree/worktree-lifecycle
// spec: worktree/worktree-lifecycle#create-and-remove
#[test]
fn happy_path_dependency_and_merge() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"create a","scope":["A.txt"],"accept":"{ga}"}},
                {{"id":"B","title":"follow on","deps":["A"],"scope":["B.txt"],"accept":"{gb}"}}
            ] }}"#,
            ga = gate_cmd("A.txt"),
            gb = gate_cmd("B.txt")
        ),
        &worker_json(1),
    );
    // Give each task its OWN artifact (A.txt / B.txt) so the dependent task B
    // produces a genuine committed change instead of re-creating A's file (a
    // no-change attempt must not reach Done). Ride the sandbox passthrough.
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_TOUCH_FROM_MODEL,FAKE_AGENT_TOUCH_FROM_TASK,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    std::env::set_var("FAKE_AGENT_TOUCH_FROM_TASK", "1");
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "run should exit 0 (all done)");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(st["B"].state, TaskState::Done);
    // Each task's artifact is merged to main.
    assert!(f.repo.join("A.txt").exists(), "A.txt merged to main");
    assert!(f.repo.join("B.txt").exists(), "B.txt merged to main");
    // No leftover worktree.
    assert!(!f.st.worktree_root.join("A").exists());
    assert!(!f.st.worktree_root.join("B").exists());
    // Receipts were appended (one per merged task), outcome "merged".
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert!(receipts.len() >= 2, "one receipt per merged task");
    assert!(receipts.iter().all(|r| r.outcome == "merged"));
    // First-attempt prompts carry no retry history (ADR-13).
    let prompt = std::fs::read_to_string(f.st.state_dir.join("prompts").join("B.md"))
        .expect("prompt file exists");
    assert!(!prompt.contains("Previous attempts"));
}

/// The `command` shell-template dispatch reaches Done: the agent child is
/// spawned via the platform shell with `{prompt}` substituted, the work
/// merges, the gate passes. (GOWORKER parity — legacy argv dispatch is
/// covered by every other e2e.)
// spec: config/worker-command-template
#[test]
fn happy_path_command_template_dispatch() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::set_var("FAKE_AGENT_TOUCH", "1");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"create a","scope":["A.txt"],"accept":"{ga}"}}
            ] }}"#,
            ga = gate_cmd("A.txt")
        ),
        &worker_json_command(1),
    );
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "run should exit 0 (all done)");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(f.repo.join("A.txt").exists(), "A.txt merged to main");
    assert!(!f.st.worktree_root.join("A").exists());
}

/// Failed attempts get receipts too — the routing substrate (ADR-12).
// spec: state/cost-receipts
// spec: lifecycle/prompt-rendering
// spec: lifecycle/prompt-rendering#retry-prompt-names-the-earlier-failure
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
// spec: lifecycle/execute-pipeline#agent-failure
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
// spec: lifecycle/execute-pipeline#gate-failure
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
// spec: cli/run-commands#dry-run-changes-nothing
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
// spec: scheduling/retry-reuses-verified-agent-work
// spec: scheduling/retry-reuses-verified-agent-work#attempts-are-bounded-by-max-attempts
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
/// The seeded state file predates the attempt-phase journal (no `phase`
/// key), so the legacy resume path must re-run the agent from scratch.
// spec: state/resume-and-self-heal
// spec: state/attempt-phase-journal#legacy-state-resumes-by-re-running-the-agent
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
        &worker_json(2),
    );
    // Pre-seed the state file as if a previous process died mid-run. With
    // the interrupted attempt now consuming budget on heal, the seeded
    // entry has spent 1 of 2 attempts — one spare slot lets the re-run
    // complete instead of healing straight to Failed.
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
// spec: sandbox/agent-environment-allowlist#foreign-secrets-are-not-leaked
// spec: sandbox/agent-environment-allowlist#passthrough-escape-hatch
// spec: sandbox/git-hygiene-for-agent-children
// spec: sandbox/git-hygiene-for-agent-children#git-hygiene-pairs-present
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
// spec: worktree/worktrees-target-the-task-s-repository#cross-repo-campaign-lands-in-the-right-repos
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
// spec: config/per-task-repo-resolution-with-compat-fallback#unknown-repo-warns-and-falls-back
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
// spec: scheduling/scope-contention-avoidance#disjoint-scope-dispatches-in-parallel
// spec: worktree/merge-serialization
// spec: worktree/merge-serialization#serialized-merges
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
// spec: lifecycle/subprocess-execution-contract#timeout-kills
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
// spec: sandbox/sandbox-wrapper-hook#wrapper-prepended
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
// spec: state/resume-and-self-heal#orphan-worktrees-cleaned
// spec: worktree/self-heal-spans-all-repositories
// spec: worktree/self-heal-spans-all-repositories#stale-worktree-is-cleaned-regardless-of-its-repo
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

/// The mirror of `sandbox_wrapper_cmd_is_prepended_to_agent_argv`: with
/// `TF_SANDBOX_CMD` UNSET the agent argv is unchanged — the child is
/// invoked as `<agent-cli> --provider … --model … -p @prompt` with no
/// wrapper prefix. The "agent" is a recording script that logs `$0` and
/// `$@` so the no-op is observed from the child's own argv.
// spec: sandbox/sandbox-wrapper-hook#unset-wrapper-is-a-no-op
#[cfg(unix)]
#[test]
fn unset_sandbox_wrapper_leaves_the_agent_argv_unchanged() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let mut f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"unwrapped","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    assert!(
        f.st.sandbox_cmd.is_empty(),
        "fixture must not set a sandbox wrapper for the no-op arm"
    );

    // The worker's cli is a recording script: it logs $0 and $@, then exits
    // 0 WITHOUT creating DONE.txt, so the gate fails the attempt — this
    // test asserts the argv, not a merge.
    let recorder = f.dir.join("record-argv.sh");
    let argv_file = f.dir.join("argv.txt");
    std::fs::write(
        &recorder,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > {}\nexit 0\n",
            argv_file.to_string_lossy()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&recorder, std::fs::Permissions::from_mode(0o755)).unwrap();
    f.cfg.workers[0].cli = recorder.to_string_lossy().to_string();

    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 2);

    let argv = std::fs::read_to_string(&argv_file).expect("recorder logged its argv");
    let mut lines = argv.lines();
    let zeroth = lines.next().unwrap_or_default();
    assert_eq!(
        zeroth,
        recorder.to_string_lossy(),
        "with no wrapper, argv[0] is the agent CLI itself: {argv}"
    );
    assert!(
        argv.contains("--provider") && argv.contains("-p"),
        "agent args follow unchanged: {argv}"
    );
}

/// Retry after a GATE failure (scheduling spec): the gate passes only from
/// the third attempt on (a counter file in the fixture dir counts gate
/// runs). Attempt 2 attaches to the kept branch as a gate-only retry; when
/// the gate fails again the branch is dropped, so attempt 3 is a fresh
/// agent run that passes and merges — and the receipts record the exact
/// spec sequence `failed, failed, merged` (state spec).
// spec: scheduling/retry-reuses-verified-agent-work
// spec: scheduling/retry-reuses-verified-agent-work#a-second-gate-failure-falls-back-to-a-fresh-attempt
// spec: state/cost-receipts#failed-attempts-are-recorded
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#a-second-gate-failure-archives-the-reused-work
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#the-work-survives-every-failure-path
#[test]
fn retry_after_gate_failure_runs_fresh_attempts_until_it_passes() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
    let dir = std::env::temp_dir().join(format!("af-e2e-retry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let count = dir.join("GATE_COUNT");
    let count = count.to_string_lossy().to_string();
    // The gate fails the first two times it runs (counter < 3), then passes.
    let flaky_gate = format!(
        "n=$(cat {count} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {count}; test $n -ge 3"
    );
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"flaky gate","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = flaky_gate
        ),
        &worker_json(3),
    );

    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(
        st["A"].attempts, 3,
        "gate failed twice, passed on attempt 3"
    );
    assert!(f.repo.join("DONE.txt").exists(), "attempt 3 merged");

    // Receipts: one per attempt, outcomes failed/failed/merged in attempt
    // order (load order is by timestamp, so group by attempt number).
    let mut receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    receipts.sort_by_key(|r| r.attempt);
    let outcomes: Vec<&str> = receipts.iter().map(|r| r.outcome.as_str()).collect();
    assert_eq!(
        outcomes,
        vec!["failed", "failed", "merged"],
        "spec sequence"
    );

    // The gate-only retry's SECOND failure archives the very work the retry
    // was built to preserve: the receipt names the archived branch and the
    // ref survives the fresh attempt 3. `cleanup()` used to delete it.
    let second = receipts.iter().find(|r| r.attempt == 2).unwrap();
    assert!(
        second
            .error
            .as_deref()
            .unwrap_or("")
            .contains("work kept on branch tf/A-rejected-"),
        "the reused-gate failure names the archived branch: {second:?}"
    );
    let archived = archived_refs(&f.repo);
    assert_eq!(
        archived.len(),
        1,
        "the reused work was archived: {archived:?}"
    );
    archived_holds(
        &f.repo,
        &archived[0],
        "DONE.txt",
        "example-agent: task complete\n",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// THE decisive cost test: a gate FLAKE (fails the first time it runs,
/// passes the second) must not re-pay the agent. The agent's committed
/// work is durable on the attempt branch (journal `AgentDone`), so the
/// retry re-runs ONLY the gate — the agent is never invoked again.
/// Proof from the task log: `execute_attempt` appends the marker line
/// `-- agent --` immediately before every agent spawn, and a gate-only
/// retry never reaches that code — so the count must be exactly 1.
/// (Under the old always-fresh behavior the count was 2: attempt 1's
/// `cleanup()` ran `git worktree remove --force` + `git branch -D`,
/// discarding the paid work and re-running the agent for it.)
// spec: scheduling/retry-reuses-verified-agent-work#gate-failure-retries-only-the-gate
#[test]
fn gate_flake_retry_does_not_rerun_the_agent() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    // Counter file OUTSIDE the repo (absolute path under a temp dir): it
    // must survive worktree removal/recreation between attempts.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-e2e-flake-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let count = dir.join("COUNT");
    let count = count.to_string_lossy().to_string();
    // The gate fails the first time it runs (n=1), passes from the second on.
    let flaky_gate = format!(
        "n=$(cat {count} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {count}; test $n -ge 2"
    );
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"flake then pass","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = flaky_gate
        ),
        &worker_json(3),
    );

    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(
        st["A"].attempts, 2,
        "gate flaked on attempt 1; attempt 2 was a gate-only retry that passed"
    );
    assert!(f.repo.join("DONE.txt").exists(), "agent artifact merged");

    // The agent ran EXACTLY once across both attempts (the gate ran twice).
    let log = std::fs::read_to_string(f.st.state_dir.join("logs").join("A.log"))
        .expect("attempt log exists");
    assert_eq!(
        log.matches("-- agent --").count(),
        1,
        "agent must not re-run on a gate-only retry:\n{log}"
    );
    // One receipt per attempt with the real outcomes (routing substrate).
    let mut receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    receipts.sort_by_key(|r| r.attempt);
    let outcomes: Vec<&str> = receipts.iter().map(|r| r.outcome.as_str()).collect();
    assert_eq!(
        outcomes,
        vec!["failed", "merged"],
        "one receipt per attempt"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The data-loss half of the gate-failure bug: an ALWAYS-failing gate
/// (max_attempts=1) must fail the task (exit 2) but KEEP the agent's
/// committed attempt branch — the paid-for work is durable evidence, not
/// garbage to `git branch -D`. Today's `cleanup()` destroyed both the
/// worktree and the branch; only the worktree dir may go.
// spec: scheduling/retry-reuses-verified-agent-work#gate-failure-keeps-the-verified-branch
// spec: lifecycle/execute-pipeline#gate-failure
#[test]
fn gate_failure_keeps_the_agents_committed_branch() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"gate always blocks","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("NEVER.txt") // demands a file the agent will not create
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 2);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    assert!(st["A"].last_error.as_deref().unwrap_or("").contains("gate"));
    assert!(!f.repo.join("DONE.txt").exists(), "nothing merged");

    // The attempt branch survives, carrying the agent's commit(s) beyond
    // the base branch (worktree::branch_exists-equivalent evidence).
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "refs/heads/tf/A"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "branch tf/A must survive a gate failure: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ahead = std::process::Command::new("git")
        .args(["rev-list", "--count", "main..tf/A"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert!(
        ahead.status.success()
            && String::from_utf8_lossy(&ahead.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
                > 0,
        "branch carries the agent's committed work beyond main: {}{}",
        String::from_utf8_lossy(&ahead.stdout),
        String::from_utf8_lossy(&ahead.stderr)
    );
    // Only the worktree DIRECTORY is dropped; the branch (the work) stays.
    assert!(!f.st.worktree_root.join("A").exists());
}

/// A failed attempt's receipt carries the failure's first line (state
/// spec): an agent exiting 7 produces `error: Some("agent exited …7…")`.
// spec: state/cost-receipts#failed-receipt-carries-the-reason
#[test]
fn failed_receipt_carries_the_failure_reason() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("FAKE_AGENT_EXIT", "7");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"boom","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 2);
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1);
    let err = receipts[0]
        .error
        .as_deref()
        .unwrap_or_else(|| panic!("failed receipt must carry error: {receipts:?}"));
    assert!(
        err.contains("agent exited") && err.contains('7'),
        "error is the failure's first line: {err}"
    );
}

/// Restart after a partial run (state spec): task A is already `done` from
/// a previous process; the restart must NOT re-run A (no new attempt, no
/// new receipt) and must continue with the remaining task B.
// spec: state/resume-and-self-heal#restart-continues
#[test]
fn restart_after_partial_run_skips_done_and_continues_remaining() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [
                {{"id":"A","title":"finished earlier","scope":["DONE.txt"],"accept":"{g}"}},
                {{"id":"B","title":"still to do","deps":["A"],"scope":["B.txt"],"accept":"{gb}"}}
            ] }}"#,
            g = gate_cmd("DONE.txt"),
            gb = gate_cmd("B.txt")
        ),
        &worker_json(1),
    );
    // Seed the state file as if a previous run finished A and died.
    std::fs::create_dir_all(&f.st.state_dir).unwrap();
    std::fs::write(
        f.st.state_dir.join("run-state.json"),
        r#"{ "A": { "state": "done", "attempts": 1, "last_error": null } }"#,
    )
    .unwrap();
    // B's agent writes B.txt (the default touch would write DONE.txt).
    std::env::set_var("FAKE_AGENT_TOUCH", "B.txt");

    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(st["A"].attempts, 1, "A must not be re-run on restart");
    assert_eq!(st["B"].state, TaskState::Done, "remaining task continues");
    // A was never re-dispatched: exactly one receipt exists, and it is B's.
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1, "only B ran: {receipts:?}");
    assert_eq!(receipts[0].task, "B");
    assert!(f.repo.join("B.txt").exists(), "B merged");

    std::env::remove_var("FAKE_AGENT_TOUCH");
}

/// A merge conflict fails the task branch cleanly (worktree spec): merging
/// a branch that conflicts with the base branch returns an error naming the
/// branch, aborts the merge (working tree left clean), and NEVER force-pushes
/// — the base branch keeps its own change. The failed-task/retry arm is the
/// same failure path proven by `gate_failure_fails_task`.
// spec: worktree/merge-serialization#conflict-fails-task
#[test]
fn merge_conflict_fails_the_task_branch_cleanly() {
    use agentflow::worktree::{self, MergeLocks};

    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-e2e-conflict-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    // Commits need an identity; keep it local to this scratch repo.
    let commit_all = |repo: &Path, msg: &str| {
        git(repo, &["add", "."]);
        git(
            repo,
            &[
                "-c",
                "user.name=af test",
                "-c",
                "user.email=af@test",
                "commit",
                "-m",
                msg,
            ],
        );
    };

    std::fs::write(repo.join("shared.txt"), "base\n").unwrap();
    commit_all(&repo, "init");

    // The task branch diverges: same file, different content.
    let wt_root = dir.join("wt");
    let wt = worktree::create(&repo, &wt_root, "T", "tf").expect("worktree");
    std::fs::write(wt.path.join("shared.txt"), "branch change\n").unwrap();
    commit_all(&wt.path, "task work");

    // The base branch moves the same lines the other way.
    std::fs::write(repo.join("shared.txt"), "main change\n").unwrap();
    commit_all(&repo, "base moves on");

    let locks = MergeLocks::new();
    let err = worktree::merge(&repo, &wt.branch, &locks, "merge T").unwrap_err();
    assert!(
        err.contains("merge of tf/T failed"),
        "error names the conflicting branch: {err}"
    );
    // Never force-pushed: main keeps its own version of the file.
    assert_eq!(
        std::fs::read_to_string(repo.join("shared.txt")).unwrap(),
        "main change\n"
    );
    // The conflict was aborted, not staged: the working tree is clean.
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "",
        "merge --abort left no conflicted files staged"
    );

    worktree::remove(&repo, &wt);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The merge-conflict arm of the preserve-work contract: when an attempt's
/// committed branch conflicts with the base branch at merge time, the run
/// must keep the agent's paid-for work under an archived branch name
/// (`<branch>-rejected-<now>`) BEFORE removing the original branch, and the
/// failure message (→ receipt → `af api results`) must name that branch.
/// The conflict is engineered: this thread advances `main` on the seeded
/// file while the agent sleeps mid-attempt, so the only way the merge can
/// go is a genuine conflict (both sides changed the same line differently).
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#merge-conflict-archives-the-committed-work
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#archiving-never-fails-the-attempt
#[test]
fn a_merge_conflict_keeps_the_agents_work_on_an_archived_branch() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"conflict","scope":["work.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("work.txt")
        ),
        &worker_json(1),
    );
    // Seed work.txt on the base so the branch AND a concurrent base advance
    // both edit it (a genuine conflict, not an add-on-both-sides).
    std::fs::write(f.repo.join("work.txt"), "base\n").unwrap();
    git(&f.repo, &["add", "work.txt"]);
    git(&f.repo, &["commit", "-m", "seed work.txt"]);
    // The agent writes a DIFFERENT value to work.txt and is slowed so this
    // thread can advance main between the branch-off and the merge.
    std::env::set_var("FAKE_AGENT_TOUCH", "work.txt");
    // The stub writes `{out}\n`, so the file content is the single value.
    std::env::set_var("FAKE_AGENT_OUT", "agent version");
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES,FAKE_AGENT_SLEEP_MS",
    );
    std::env::set_var("FAKE_AGENT_SLEEP_MS", "1500");

    // Drive the orchestrator on a background thread so this thread can move
    // main while the attempt is mid-flight (exactly like the parallel E2E
    // tests drive run_loop on a background thread).
    let cfg = f.cfg.clone();
    let st = f.st.clone();
    let (tx, rx) = std::sync::mpsc::channel::<i32>();
    std::thread::spawn(move || {
        let code = run::run_loop(&cfg, &st, &RunOptions::default());
        let _ = tx.send(code);
    });

    // Wait until the worktree exists (tf/A is at the seeded base), then
    // advance main on the SAME file the agent will edit. The later merge
    // then conflicts: both sides changed work.txt differently from base.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if f.st.worktree_root.join("A").join(".git").exists() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worktree never created within 15s"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::fs::write(f.repo.join("work.txt"), "main version\n").unwrap();
    git(&f.repo, &["add", "work.txt"]);
    git(&f.repo, &["commit", "-m", "advance main"]);

    let code = rx
        .recv_timeout(std::time::Duration::from_secs(40))
        .expect("run completed");
    assert_eq!(code, 2, "conflicting merge must fail the run");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.clone().unwrap_or_default();
    assert!(
        err.contains("work kept on branch") && err.contains("tf/A-rejected-"),
        "error names the archived branch: {err}"
    );
    // Nothing merged: main keeps its own advancing change (never force-pushed).
    assert_eq!(
        std::fs::read_to_string(f.repo.join("work.txt")).unwrap(),
        "main version\n"
    );
    // The archived branch is discoverable and really contains the agent work.
    let list = std::process::Command::new("git")
        .args(["branch", "--list", "tf/A-rejected-*"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    let names: Vec<String> = String::from_utf8_lossy(&list.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(names.len(), 1, "exactly one archived branch: {names:?}");
    let name = &names[0];
    let show = std::process::Command::new("git")
        .args(["show", &format!("{name}:work.txt")])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert!(
        show.status.success() && String::from_utf8_lossy(&show.stdout) == "agent version\n",
        "archived branch preserves the agent's conflicting edit, got: {}",
        String::from_utf8_lossy(&show.stdout)
    );
    // The ORIGINAL branch is gone so the retry starts clean from base.
    let gone = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/tf/A"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert!(
        !gone.status.success(),
        "original branch tf/A must be removed so a retry starts clean"
    );

    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
    std::env::remove_var("FAKE_AGENT_SLEEP_MS");
}

/// The FALSE GREEN this round exists to kill: an agent that exits 0 while
/// producing no change at all must NOT be reported as merged. Before the
/// fix, the gate passed in a worktree the merge did not carry (the branch
/// tip IS the base, so `git merge --no-ff` answered "Already up to date")
/// and the task was recorded Done with nothing in the base.
// spec: lifecycle/an-attempt-that-produces-no-change-is-not-merged
// spec: lifecycle/an-attempt-that-produces-no-change-is-not-merged#zero-commit-attempt-fails-never-merged
// spec: worktree/the-attempt-s-work-is-committed-before-it-is-merged#a-no-op-merge-is-not-reported
#[test]
fn an_attempt_that_produces_no_change_never_reports_merged() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_NO_COMMIT");
    // The stub writes `{out}\n`; the content matches the seeded README.md
    // byte-for-byte, so it produces NO git change (a clean worktree, zero
    // commits ahead) while still exiting 0 with a passing gate.
    std::env::set_var("FAKE_AGENT_TOUCH", "README.md");
    std::env::set_var("FAKE_AGENT_OUT", "# scratch");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"no change","scope":["README.md"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("README.md")
        ),
        &worker_json(1),
    );
    // Count commits on main before the run: the run must not add any.
    let before = std::process::Command::new("git")
        .args(["rev-list", "--count", "main"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    let before = String::from_utf8_lossy(&before.stdout).trim().to_string();

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "a no-change attempt must fail the run");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_ne!(st["A"].state, TaskState::Done, "must not reach done");
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.clone().unwrap_or_default();
    assert!(
        err.contains("produced no change") && err.contains("0 commits ahead"),
        "reason names the zero-commit condition: {err}"
    );

    // The receipt is the routing substrate and must not claim a merge.
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].outcome, "failed",
        "no-change attempt must not report merged: {receipts:?}"
    );
    assert!(receipts[0]
        .error
        .as_deref()
        .unwrap_or("")
        .contains("0 commits ahead"));

    // The base is untouched: no new commit.
    let after = std::process::Command::new("git")
        .args(["rev-list", "--count", "main"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert_eq!(
        String::from_utf8_lossy(&after.stdout).trim(),
        before,
        "base must gain no commit from a no-change attempt"
    );

    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
}

/// DURABLE FIRST: an agent that exits 0 leaving its edit UNCOMMITTED must
/// still land the work. The orchestrator commits the dirty worktree on the
/// attempt branch before judging it, so the gate sees the committed tree and
/// the merge carries it into the base — instead of the pre-fix false green
/// where the gate passed on a dirty file that the merge never carried and
/// cleanup then destroyed.
// spec: lifecycle/attempt-work-is-durable-before-it-is-judged
// spec: lifecycle/attempt-work-is-durable-before-it-is-judged#dirty-worktree-is-committed-before-judging
// spec: worktree/the-attempt-s-work-is-committed-before-it-is-merged
// spec: worktree/the-attempt-s-work-is-committed-before-it-is-merged#merge-carries-the-attempt-s-real-content
#[test]
fn a_dirty_worktree_is_committed_before_the_attempt_is_judged() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"dirty done","scope":["TOUCHED.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("TOUCHED.txt")
        ),
        &worker_json(1),
    );
    // The no-commit knob must ride the sandbox passthrough, else the agent
    // child never sees it (and the test measures nothing).
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES,FAKE_AGENT_NO_COMMIT",
    );
    std::env::set_var("FAKE_AGENT_TOUCH", "TOUCHED.txt");
    std::env::set_var("FAKE_AGENT_OUT", "dirty but real");
    std::env::set_var("FAKE_AGENT_NO_COMMIT", "1");

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "the dirty work must be committed and merged");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);

    // The work is IN THE BASE'S COMMITTED TREE — not merely on a dirty
    // checkout that cleanup destroyed.
    let show = std::process::Command::new("git")
        .args(["show", "main:TOUCHED.txt"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert!(
        show.status.success(),
        "base's committed tree must contain the agent's file: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&show.stdout), "dirty but real\n");

    // A real merge happened, so the receipt honestly says merged.
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "merged", "{receipts:?}");

    // No leftover worktree; the base repo's working tree is clean.
    assert!(!f.st.worktree_root.join("A").exists());
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");

    // The harness committed the dirty work with its own message (the agent
    // never committed), and the branch was consumed by the merge.
    let log = std::process::Command::new("git")
        .args(["log", "--format=%s", "main"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    let log = String::from_utf8_lossy(&log.stdout);
    assert!(
        log.contains("af: A attempt 1 — agent left uncommitted work"),
        "log names the durability commit: {log}"
    );

    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
    std::env::remove_var("FAKE_AGENT_NO_COMMIT");
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
}

/// DURABLE FIRST, uncommittable arm: if the agent's uncommitted work cannot
/// be committed, the attempt must fail naming the commit error — a tree that
/// cannot be made durable must not be judged or merged. A rejecting
/// pre-commit hook (the operator's git hooks are honored; no `--no-verify`)
/// makes the durability commit fail even though the file is otherwise fine.
// spec: lifecycle/attempt-work-is-durable-before-it-is-judged#an-uncommittable-tree-fails-the-attempt
#[test]
fn an_uncommittable_dirty_tree_fails_the_attempt_cleanly() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"blocked dirty","scope":["B.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("B.txt")
        ),
        &worker_json(1),
    );
    // Leave the work uncommitted and ride the passthrough so the orchestrator
    // must commit it for us.
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES,FAKE_AGENT_NO_COMMIT",
    );
    std::env::set_var("FAKE_AGENT_TOUCH", "B.txt");
    std::env::set_var("FAKE_AGENT_OUT", "can't land");
    std::env::set_var("FAKE_AGENT_NO_COMMIT", "1");
    // The operator's pre-commit hook rejects every commit in this repo.
    // (Unix-only: POSIX exec bits; the same failure shape on Windows is
    // covered by the dirty-tree path without a hook.)
    #[cfg(unix)]
    let code = {
        use std::os::unix::fs::PermissionsExt;
        let hooks = f.repo.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(
            hooks.join("pre-commit"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        run::run_loop(&f.cfg, &f.st, &RunOptions::default())
    };
    #[cfg(not(unix))]
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "an uncommittable dirty tree must fail the run");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.clone().unwrap_or_default();
    assert!(
        err.contains("cannot commit the agent's uncommitted work"),
        "failure names the commit error: {err}"
    );

    // Nothing merged: the base has no new commit and no B.txt.
    assert!(!f.repo.join("B.txt").exists());
    let count = std::process::Command::new("git")
        .args(["rev-list", "--count", "main"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert_eq!(
        String::from_utf8_lossy(&count.stdout).trim(),
        "1",
        "only the seeded init commit remains"
    );

    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
    std::env::remove_var("FAKE_AGENT_NO_COMMIT");
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
}

/// THE stall failure path keeps the work. A worker that commits real work and
/// THEN stops producing output is killed by the stall watchdog — before the
/// fix the `!agent_out.passed()` arm ran `cleanup()`, whose `git branch -D`
/// destroyed the most expensive failure the harness has. Now the committed
/// branch is archived first (`<branch>-rejected-<now>`), the receipt names it,
/// and the work is recoverable even though the task is NOT `done`.
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#a-stalled-agent-archives-the-committed-work
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#the-work-survives-every-failure-path
#[cfg(unix)]
#[test]
fn a_stalled_attempt_keeps_its_work_under_an_archived_branch() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_HANG_MS");
    // The stub knobs must ride the sandbox passthrough: the agent child's env
    // is stripped otherwise and the test would silently measure nothing.
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    std::env::set_var("FAKE_AGENT_TOUCH", "DONE.txt");
    std::env::set_var("FAKE_AGENT_OUT", "committed before the stall");

    let mut f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"stall","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1), // max_attempts=1 → a single stalled attempt fails the task
    );
    f.cfg.workers[0].cli = work_then_hang_agent(&f.dir).to_string_lossy().to_string();
    f.st.agent_stall_s = 1; // the stall window (the stub hangs 2000ms)
    f.st.agent_timeout_s = 60;

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "a stalled attempt must fail the campaign");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_ne!(st["A"].state, TaskState::Done, "task must NOT be Done");
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.clone().unwrap_or_default();
    assert!(
        err.contains("work kept on branch") && err.contains("tf/A-rejected-"),
        "error names the archived branch: {err}"
    );
    // The receipt (what `af api results` reads) names the same branch.
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1, "one receipt for the stalled attempt");
    assert!(
        receipts[0]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("work kept on branch tf/A-rejected-"),
        "receipt names the archived branch: {receipts:?}"
    );

    // The archived branch exists, its tip really contains the committed work,
    // and the original attempt branch is gone so a retry starts clean.
    let names = archived_refs(&f.repo);
    assert_eq!(names.len(), 1, "exactly one archived branch: {names:?}");
    archived_holds(
        &f.repo,
        &names[0],
        "DONE.txt",
        "committed before the stall\n",
    );
    original_branch_gone(&f.repo);

    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
}

/// THE total-timeout failure path keeps the work too: with the stall watchdog
/// DISABLED (`agent_stall_s=0`), a worker that commits and then runs past
/// `agent_timeout_s` is killed by the total timeout — the same
/// `!agent_out.passed()` arm, and the same preservation. The receipt names the
/// archived branch and the work is recoverable.
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#a-timed-out-agent-archives-the-committed-work
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#the-work-survives-every-failure-path
#[cfg(unix)]
#[test]
fn a_timed_out_attempt_keeps_its_work_under_an_archived_branch() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_HANG_MS");
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    std::env::set_var("FAKE_AGENT_TOUCH", "DONE.txt");
    std::env::set_var("FAKE_AGENT_OUT", "committed before the timeout");

    let mut f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"timeout","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    f.cfg.workers[0].cli = work_then_hang_agent(&f.dir).to_string_lossy().to_string();
    f.st.agent_stall_s = 0; // watchdog off: the TOTAL timeout is the bound under test
    f.st.agent_timeout_s = 1; // the stub hangs 2000ms, so the total timeout fires first

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "a timed-out attempt must fail the campaign");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_ne!(st["A"].state, TaskState::Done, "task must NOT be Done");
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.clone().unwrap_or_default();
    assert!(
        err.contains("work kept on branch") && err.contains("tf/A-rejected-"),
        "error names the archived branch: {err}"
    );
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1, "one receipt for the timed-out attempt");
    assert!(
        receipts[0]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("work kept on branch tf/A-rejected-"),
        "receipt names the archived branch: {receipts:?}"
    );
    let names = archived_refs(&f.repo);
    assert_eq!(names.len(), 1, "exactly one archived branch: {names:?}");
    archived_holds(
        &f.repo,
        &names[0],
        "DONE.txt",
        "committed before the timeout\n",
    );
    original_branch_gone(&f.repo);

    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
}

/// Seed an archived rejected branch `tf/<id>-rejected-<ts>` off the fixture
/// repo's current base, carrying a committed `file` = `content` and leaving
/// the checkout back on `main`. Models exactly what round 11's failure paths
/// leave on disk when an attempt is rejected.
fn seed_archived_branch(repo: &Path, id: &str, ts: u64, file: &str, content: &str) {
    let branch = format!("tf/{id}");
    git(repo, &["checkout", "-b", &branch]);
    std::fs::write(repo.join(file), content).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", "agent work"]);
    git(repo, &["branch", &format!("tf/{id}-rejected-{ts}")]);
    git(repo, &["checkout", "main"]);
    // Production's failure cleanup deletes the original attempt branch after
    // archiving; only the rejected copy survives.
    git(repo, &["branch", "-D", &branch]);
}

/// The r14 archive form `<prefix>/<id>-rejected-<attempt>-<ts>`, which is
/// what `af` writes now. Recovery parses the attempt back out to pair the
/// marker with the failed receipt.
fn seed_archived_branch_at_attempt(
    repo: &Path,
    id: &str,
    attempt: u32,
    ts: u64,
    file: &str,
    content: &str,
) {
    let branch = format!("tf/{id}");
    git(repo, &["checkout", "-b", &branch]);
    std::fs::write(repo.join(file), content).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", "agent work"]);
    git(
        repo,
        &["branch", &format!("tf/{id}-rejected-{attempt}-{ts}")],
    );
    git(repo, &["checkout", "main"]);
    git(repo, &["branch", "-D", &branch]);
}

/// THE core cost test for r13-auto-recover: `af run` must find an archived
/// rejected branch that satisfies the task's CURRENT scope and gate, re-run
/// the gate on it, merge it, and NEVER invoke the agent — the money for that
/// work was already spent by the attempt that archived it. The gate remains
/// the sole arbiter, so the merged content is exactly what the gate passed.
// spec: lifecycle/pre-dispatch-reuse-of-an-archived-branch
// spec: lifecycle/pre-dispatch-reuse-of-an-archived-branch#an-in-scope-archive-is-reused-without-an-agent
// spec: cli/run-commands#pre-dispatch-reuse-never-re-pays-for-archived-work
#[test]
fn a_run_reuses_a_scope_compatible_archived_branch_without_paying_an_agent() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
    std::env::remove_var("TF_NO_REUSE");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"reuse me","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    // The archive already carries work the CURRENT scope allows and the
    // CURRENT gate accepts.
    seed_archived_branch(
        &f.repo,
        "A",
        1791259017,
        "DONE.txt",
        "example-agent: task complete\n",
    );

    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);

    // The task reached Done from the ARCHIVE, not from a fresh attempt.
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done, "archived work landed");
    // The base contains the change …
    assert!(
        f.repo.join("DONE.txt").exists(),
        "base contains the archived change"
    );
    assert_eq!(
        std::fs::read_to_string(f.repo.join("DONE.txt")).unwrap(),
        "example-agent: task complete\n"
    );
    // … and the archived branch was consumed (its content now lives in base).
    assert!(
        archived_refs(&f.repo).is_empty(),
        "archived branch removed after reuse: {:?}",
        archived_refs(&f.repo)
    );
    // THE point: the agent was NEVER invoked. `execute_attempt` appends the
    // marker `-- agent --` immediately before every spawn and reuse never
    // reaches that code, so the attempt log carries zero markers (it may not
    // even exist).
    let log =
        std::fs::read_to_string(f.st.state_dir.join("logs").join("A.log")).unwrap_or_default();
    assert_eq!(
        log.matches("-- agent --").count(),
        0,
        "reuse must not pay an agent again:\n{log}"
    );
    // Reuse is not a NEW agent attempt: the archived work's failed receipt
    // already exists and reuse adds no second failed/merged receipt — it adds
    // exactly one NON-VERDICT `recovered` marker (r14-recovery-ledger),
    // measured 0.0s because no agent ran.
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(
        receipts.len(),
        1,
        "reuse writes exactly one recovered receipt: {receipts:?}"
    );
    assert_eq!(receipts[0].outcome, "recovered");
    assert_eq!(receipts[0].wall_clock_s, 0.0, "no agent ran");
    assert!(
        !receipts[0].counts_as_verdict(),
        "recovery is not a verdict"
    );
}

/// Pre-dispatch reuse is a recovery too: it writes the same non-verdict
/// `recovered` receipt `af recover` writes, naming the ATTEMPT parsed from
/// the archived branch and the failed receipt's worker/model, with
/// wall-clock 0.0 and no agent spawn.
// spec: state/recovered-receipts
// spec: state/recovered-receipts#a-recovered-receipt-pairs-with-the-failed-attempt
#[test]
fn reuse_writes_a_recovered_receipt() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
    std::env::remove_var("TF_NO_REUSE");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"reuse me","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    seed_archived_branch_at_attempt(
        &f.repo,
        "A",
        2,
        1791259017,
        "DONE.txt",
        "example-agent: task complete\n",
    );
    // The failed attempt that produced the archive, on disk before the run.
    Store::new(f.st.state_dir.clone())
        .append_receipt(&Receipt {
            task: "A".into(),
            attempt: 2,
            worker: "w1".into(),
            model: "gpt-4o".into(),
            wall_clock_s: 100.0,
            tokens: None,
            ts: 1,
            outcome: "failed".into(),
            error: Some("acceptance gate failed (exit 1): boom".into()),
            cost_micros: None,
        })
        .unwrap();

    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);

    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    let recovered = receipts
        .iter()
        .find(|r| r.outcome == "recovered")
        .unwrap_or_else(|| panic!("reuse writes a recovered receipt: {receipts:?}"));
    assert_eq!(recovered.task, "A");
    assert_eq!(recovered.attempt, 2, "paired to the archived attempt");
    assert_eq!(recovered.worker, "w1", "names the failed attempt's worker");
    assert_eq!(recovered.model, "gpt-4o");
    assert_eq!(recovered.wall_clock_s, 0.0, "no agent ran");
    assert!(!recovered.counts_as_verdict());
    // The agent was NEVER invoked (reuse reached no spawn marker).
    let log =
        std::fs::read_to_string(f.st.state_dir.join("logs").join("A.log")).unwrap_or_default();
    assert_eq!(log.matches("-- agent --").count(), 0, "no agent: {log}");
}
