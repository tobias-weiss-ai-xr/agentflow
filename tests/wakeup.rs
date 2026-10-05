//! Dispatcher wake-up latency: a finished attempt must wake the run loop
//! IMMEDIATELY instead of the loop sleeping through its poll interval.
//!
//! The harness is modeled on tests/e2e.rs's `fixture()` (copied rather
//! than imported): a scratch git repo under a uniquely-named temp dir, a
//! tasks.json + workers.json pair, `config::load`, and a hand-built
//! `Settings` pointing the worker's `cli` at the bundled `example_agent`
//! stub. No network, no LLM — deterministic CI.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::Store;
use std::path::Path;
use std::time::{Duration, Instant};

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

/// poll_secs = 30, max_parallel = 1, two tasks chained A -> B: a serialized
/// chain, so there are exactly two dispatches and two post-dispatch waits.
/// Sleeping through the poll interval on each wait costs ≥ 60s (measured
/// 60.4s before the fix); waking on completion finishes in ~1-2s. The
/// asserted bound (20s) is impossible to reach while sleeping through even
/// ONE 30s poll interval, so it can only pass when the completion wakes
/// the dispatcher.
#[test]
fn a_finished_task_wakes_the_dispatcher_immediately() {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-wakeup-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Hermetic fixture identity: env pairs for commits/merges made by this
    // process and its children, plus repo-local config for anything that
    // does not inherit the environment.
    std::env::set_var("GIT_AUTHOR_NAME", "af test");
    std::env::set_var("GIT_AUTHOR_EMAIL", "af@test");
    std::env::set_var("GIT_COMMITTER_NAME", "af test");
    std::env::set_var("GIT_COMMITTER_EMAIL", "af@test");

    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "af test"]);
    git(&repo, &["config", "user.email", "af@test"]);
    std::fs::write(repo.join("README.md"), "# scratch\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);

    // A's gate: the stub writes DONE.txt by default, so `test -f DONE.txt`
    // passes. B's gate cannot fail — its branch may carry no new commit
    // (the stub rewrites the identical DONE.txt), and only the wake-up
    // latency is under test here.
    let tasks = format!(
        r#"{{ "tasks": [
            {{"id":"A","title":"create done","scope":["DONE.txt"],"accept":"{ga}"}},
            {{"id":"B","title":"follow on","deps":["A"],"scope":["DONE.txt"],"accept":"true"}}
        ] }}"#,
        ga = gate_cmd("DONE.txt")
    );
    // Windows paths contain backslashes — escape them for JSON.
    let workers = format!(
        r#"{{ "defaults": {{ "max_attempts": 1, "accept_timeout_s": 10 }},
            "workers": [ {{ "name": "w1", "provider": "p", "model": "m",
                            "enabled": true, "cli": "{agent}" }} ] }}"#,
        agent = AGENT.replace('\\', "\\\\")
    );
    let config_dir = dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("tasks.json"), tasks).unwrap();
    std::fs::write(config_dir.join("workers.json"), workers).unwrap();
    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .unwrap();
    let st = Settings {
        repo_dir: repo.clone(),
        state_dir: dir.join("state"),
        worktree_root: dir.join("wt"),
        max_parallel: 1,
        branch_prefix: "tf".into(),
        poll_secs: 30,
        gate_env: vec![],
        tasks_file: config_dir.join("tasks.json"),
        workers_file: config_dir.join("workers.json"),
        prompt_file: dir.join("no-template.md"),
        agent_timeout_s: 60,
        agent_stall_s: 0,
        sandbox_cmd: vec![],
    };

    // Measured: 60.4s before the wake-on-completion fix (two full 30s
    // sleeps, one per dispatch, wasted on ~0s of agent work), ~1-2s after.
    let start = Instant::now();
    let code = run::run_loop(&cfg, &st, &RunOptions::default());
    let elapsed = start.elapsed();

    assert_eq!(code, 0, "run should exit 0 (all done)");
    let final_state = Store::new(st.state_dir.clone()).load();
    assert_eq!(final_state["A"].state, TaskState::Done, "A done");
    assert_eq!(final_state["B"].state, TaskState::Done, "B done");
    assert!(
        elapsed < Duration::from_secs(20),
        "dispatcher slept through a poll interval (poll_secs=30, two waits): \
         {elapsed:?} elapsed — measured 60.4s before the fix, ~1-2s after"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
