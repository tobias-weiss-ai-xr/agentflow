//! Stall-watchdog E2E tests: an agent that stops producing output is
//! killed by the stall window long before the total `agent_timeout_s`
//! would expire — and a healthy agent is never disturbed by the watchdog.
//!
//! Same substrate as tests/e2e.rs (scratch git repo + the bundled
//! `example_agent` stub, no network, no LLM). Mutates process-global env
//! vars used by example_agent, so the tests share one mutex and run
//! serialized.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::Store;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// Build the full `af run` harness (scratch repo + config + settings) the
/// same way tests/e2e.rs::fixture does, with the two knobs under test —
/// the total `agent_timeout_s` and the stall window `agent_stall_s` —
/// passed in explicitly.
fn fixture(
    tasks_json: &str,
    workers_json: &str,
    agent_timeout_s: u64,
    agent_stall_s: u64,
) -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The sandbox strips the agent child's env; the example_agent knobs
    // must ride TF_AGENT_ENV_PASSTHROUGH (FAKE_AGENT_HANG_MS is the stall
    // producer these tests rely on).
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_HANG_MS",
    );
    let dir = std::env::temp_dir().join(format!("af-watchdog-{}-{n}", std::process::id()));
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
        agent_timeout_s,
        agent_stall_s,
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

fn one_task_json(gate: &str) -> String {
    format!(
        r#"{{ "tasks": [ {{"id":"A","title":"stall probe","scope":["DONE.txt"],"accept":"{gate}"}} ] }}"#
    )
}

/// A stalled agent is killed by the stall window, NOT by the total
/// `agent_timeout_s`. The stub is told to produce NO output far longer
/// than the 1s window (FAKE_AGENT_HANG_MS is capped at 2000ms inside the
/// stub — still 2x the window); the watchdog fires at ~1s while the total
/// timeout is 60s. The failure is diagnosable and distinct: the receipt
/// error names the stall and the configured window.
#[test]
fn a_stalled_agent_is_killed_before_the_total_timeout() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::set_var("FAKE_AGENT_HANG_MS", "30000"); // stub caps it to 2000ms

    let f = fixture(
        &one_task_json(&gate_cmd("DONE.txt")),
        &worker_json(1), // max_attempts=1 → a single stalled attempt fails the task
        60,              // agent_timeout_s — the total bound (an hour of paid tokens by default)
        1,               // agent_stall_s — the stall window under test
    );
    let start = Instant::now();
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    let elapsed = start.elapsed();

    // The attempt fails: task NOT Done (terminal Failed), exit code 2.
    assert_ne!(code, 0, "a stalled attempt must fail the campaign");
    assert_eq!(code, 2, "deadlock/failure → exit 2");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_ne!(st["A"].state, TaskState::Done, "task must NOT be Done");
    assert_eq!(st["A"].state, TaskState::Failed);
    assert!(!f.repo.join("DONE.txt").exists(), "nothing merged");

    // DIAGNOSABLE + DISTINCT: the receipt error says the agent produced no
    // output and names the configured stall window — not "agent exited".
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1, "one receipt for the failed attempt");
    let err = receipts[0]
        .error
        .as_deref()
        .unwrap_or_else(|| panic!("failed receipt must carry error: {receipts:?}"));
    assert!(
        err.contains("stalled") && err.contains("no output"),
        "error names the stall: {err}"
    );
    assert!(
        err.contains("1s"),
        "error names the configured stall window (1s): {err}"
    );
    assert!(
        !err.contains("agent exited"),
        "a stall is not an exit-code failure: {err}"
    );
    // The task's last_error carries the same diagnosis.
    assert!(
        st["A"]
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("stalled"),
        "last_error names the stall: {:?}",
        st["A"].last_error
    );

    // THE POINT of the watchdog — the measured contrast:
    //   legacy (stall disabled): the silent agent would sit out the FULL
    //     agent_timeout_s = 60s before Timeout, burning paid tokens.
    //   watchdog (window = 1s): the kill lands at ~1.0-1.2s (window +
    //     50ms poll granularity + 200ms post-kill snapshot grace), the
    //     whole run_loop returns in ~1.5-3s once worktree cleanup and the
    //     reap poll are counted — 20-40x below the total timeout.
    // The 20s bound leaves generous CI slack while still proving the run
    // never approached the 60s total timeout.
    assert!(
        elapsed < Duration::from_secs(20),
        "stall kill at the 1s window must not burn agent_timeout_s=60s (took {elapsed:?})"
    );

    std::env::remove_var("FAKE_AGENT_HANG_MS");
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// The mirror contract: an armed stall window never disturbs a healthy
/// agent (it speaks immediately and exits → Done, exit 0), and the default
/// `agent_stall_s = 0` is genuinely DISABLED — an agent silent for far
/// longer than the window still completes under the legacy behaviour.
#[test]
fn a_stall_window_does_not_disturb_a_healthy_agent() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");

    // Arm the watchdog (1s window) and run a healthy agent: output arrives
    // immediately, the agent exits on its own — the window must never fire.
    std::env::remove_var("FAKE_AGENT_HANG_MS");
    let f = fixture(
        &one_task_json(&gate_cmd("DONE.txt")),
        &worker_json(1),
        60, // agent_timeout_s
        1,  // agent_stall_s — armed
    );
    assert_eq!(
        run::run_loop(&f.cfg, &f.st, &RunOptions::default()),
        0,
        "healthy agent + armed watchdog → run succeeds"
    );
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(f.repo.join("DONE.txt").exists(), "agent work merged");
    assert!(!f.st.worktree_root.join("A").exists(), "worktree cleaned");
    let _ = std::fs::remove_dir_all(&f.dir);

    // Default (agent_stall_s = 0) is DISABLED, not a tiny window: the stub
    // is silent for 2s — twice the window that killed the agent in the
    // test above — and still runs to completion untouched.
    std::env::set_var("FAKE_AGENT_HANG_MS", "2000");
    let f = fixture(
        &one_task_json(&gate_cmd("DONE.txt")),
        &worker_json(1),
        60, // agent_timeout_s
        0,  // agent_stall_s — the documented default: watchdog off
    );
    assert_eq!(
        run::run_loop(&f.cfg, &f.st, &RunOptions::default()),
        0,
        "stall_s=0 must never kill a (temporarily) silent agent"
    );
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(
        st["A"].state,
        TaskState::Done,
        "legacy behaviour: bounded only by the total timeout"
    );
    assert!(f.repo.join("DONE.txt").exists());

    std::env::remove_var("FAKE_AGENT_HANG_MS");
    let _ = std::fs::remove_dir_all(&f.dir);
}
