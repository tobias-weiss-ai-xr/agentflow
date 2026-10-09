//! E2E: a worker's `args` are passed through to the agent CLI argv
//! (config spec: Worker schema loading).
//!
//! End-to-end means END TO END: `workers.json` → `config::load` →
//! `run_loop` → `execute` → the REAL agent child argv — not a unit test of
//! `spawn_argv`. The agent is the bundled `example_agent` stub
//! (`env!("CARGO_BIN_EXE_example_agent")`) and the run's observable effect
//! is asserted from the merged artifact on the base repo, exactly like
//! tests/e2e.rs.
//!
//! # How receipt is proven (and why a recording wrapper rides along)
//!
//! `src/bin/example_agent.rs` reads its argv through exactly one channel:
//! a FIRST-wins scan for `--model` (used when `FAKE_AGENT_TOUCH_FROM_MODEL`
//! is set, writing `{model}.txt`). Worker `args` are appended AFTER the
//! worker's own `--model M` (their stable, documented position), so by
//! construction no appended arg can ever win that scan — the stub cannot
//! itself react to an extra arg it was handed, short of changing the stub
//! (e.g. to last-wins), which is out of this task's file scope. NO change
//! to the stub was made or needed. Instead, receipt is observed at the
//! process boundary by a recording wrapper riding the documented
//! `TF_SANDBOX_CMD` hook (the same argv-observation point
//! tests/e2e.rs::sandbox_wrapper_cmd_is_prepended_to_agent_argv uses):
//! the wrapper appends its `$@` to a file and then `exec`s through to the
//! REAL stub, so the full pipeline — spawn → agent → gate → merge — still
//! completes end-to-end. The stub's first-wins scan still pulls its weight:
//! with `args: ["--model", "from-worker-args"]` and worker model
//! `base-model`, a merged `base-model.txt` proves from the CHILD's own
//! behavior that the extra `--model` arrived AFTER the built-in one (had
//! the args been placed before it, the stub would have written
//! `from-worker-args.txt` and the gate would have failed).
//!
//! Unix-only, like the wrapper-based e2e tests (the recording wrapper is a
//! /bin/sh script; CI is Linux).

#![cfg(unix)]

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::Store;
use std::path::{Path, PathBuf};

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

/// Mirrors tests/e2e.rs::fixture: scratch git repo, temp tasks/workers
/// files, `agentflow::config::load`, hand-built `Settings`. The FAKE_AGENT
/// knobs must ride `TF_AGENT_ENV_PASSTHROUGH` because the sandbox strips
/// the agent child's env.
fn fixture(tasks_json: &str, workers_json: &str, tag: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("af-worker-args-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_TOUCH_FROM_MODEL",
    );
    // Deterministic stub knobs: default touch/exit, no env surprises.
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_TOUCH_FROM_MODEL");
    // Identity for commits/merges (inherited by child git processes).
    std::env::set_var("GIT_AUTHOR_NAME", "af test");
    std::env::set_var("GIT_AUTHOR_EMAIL", "af@test");
    std::env::set_var("GIT_COMMITTER_NAME", "af test");
    std::env::set_var("GIT_COMMITTER_EMAIL", "af@test");

    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
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
        max_parallel: 1,
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

/// A `/bin/sh` wrapper for `TF_SANDBOX_CMD` that records its `$@` (one
/// argument per line) and then `exec`s through to the real child, so the
/// run still completes end-to-end while the exact argv is captured.
fn recording_wrapper(dir: &Path) -> PathBuf {
    let wrapper = dir.join("record-and-exec.sh");
    let record = dir.join("argv.txt");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nexec \"$@\"\n",
            record.to_string_lossy()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    wrapper
}

/// Worker `args` reach the agent CLI argv END TO END: a campaign whose
/// worker declares `args: ["--model", "from-worker-args"]` dispatches a
/// child whose argv carries that pair AFTER the built-in `--model M` and
/// BEFORE `-p @file`, the run completes (gate passes, artifact merges), and
/// a worker with NO `args` dispatches exactly as before. Plus the config
/// contract: an absent `args` field loads as an empty vector, and an empty
/// string entry is rejected at load time naming the worker.
// spec: config/worker-schema-loading#worker-args-are-passed-through-to-the-agent-cli
// spec: config/worker-schema-loading#empty-arg-entries-are-rejected
#[test]
fn worker_args_are_passed_through_to_the_agent_cli() {
    // ------------------------------------------------------------------
    // Config contract first (cheap, and it fails fastest):
    // ------------------------------------------------------------------
    let d = std::env::temp_dir().join(format!("af-worker-args-cfg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("tasks.json"),
        r#"{ "tasks": [{"id":"A","title":"a","accept":"true"}] }"#,
    )
    .unwrap();

    // Absent `args` ⇒ empty vector (existing workers.json keeps working).
    std::fs::write(
        d.join("workers-no-args.json"),
        r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}] }"#,
    )
    .unwrap();
    let cfg = config::load(&d.join("tasks.json"), &d.join("workers-no-args.json")).unwrap();
    assert!(
        cfg.workers[0].args.is_empty(),
        "absent args field must deserialize as an empty vector"
    );

    // An empty string entry is rejected at load time, naming the worker.
    std::fs::write(
        d.join("workers-bad-args.json"),
        r#"{ "workers": [{"name":"w1","provider":"p","model":"m","args":["--flag",""]}] }"#,
    )
    .unwrap();
    let err = config::load(&d.join("tasks.json"), &d.join("workers-bad-args.json")).unwrap_err();
    assert!(
        err.contains("worker 'w1'") && err.contains("non-empty"),
        "empty args entry must be rejected naming the worker: {err}"
    );

    // ------------------------------------------------------------------
    // Arm 1: worker WITH args — the pair must reach the child argv in the
    // documented position, and the run must complete through the stub.
    // ------------------------------------------------------------------
    let mut f = fixture(
        r#"{ "tasks": [
            {"id":"A","title":"touch model file","scope":["base-model.txt"],"accept":"test -f base-model.txt"}
        ] }"#,
        &format!(
            r#"{{ "workers": [{{"name":"w1","provider":"p","model":"base-model","enabled":true,
                "cli":"{agent}","args":["--model","from-worker-args"]}}] }}"#,
            agent = AGENT.replace('\\', "\\\\")
        ),
        "args",
    );
    // The stub's model-derived touch (see module docs) is armed for THIS
    // arm only — it is the stub-side proof of the appended args' position.
    std::env::set_var("FAKE_AGENT_TOUCH_FROM_MODEL", "1");
    // The argv observer rides the sandbox hook (see module docs); it
    // records, then execs through to the real stub.
    let wrapper = recording_wrapper(&f.dir);
    f.st.sandbox_cmd = vec![wrapper.to_string_lossy().to_string()];

    assert_eq!(
        run::run_loop(&f.cfg, &f.st, &RunOptions::default()),
        0,
        "the args campaign must complete"
    );
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);

    // (a) The run's observable effect, from the stub's OWN behavior: the
    // first `--model` the child saw was the worker's `base-model` (the
    // stub writes `{model}.txt`), so the extra `--model from-worker-args`
    // must have arrived AFTER it — and the artifact merged to main.
    assert!(
        f.repo.join("base-model.txt").exists(),
        "stub wrote (and the run merged) the FIRST --model's file"
    );
    assert!(
        !f.repo.join("from-worker-args.txt").exists(),
        "the appended --model must not have won the stub's first-wins scan"
    );

    // (b) Receipt at the process boundary: the recorded argv carries the
    // pair, positioned after the built-in `--model M` and before the
    // `-p @file` handoff, which stays LAST.
    let argv = std::fs::read_to_string(f.dir.join("argv.txt")).expect("wrapper recorded argv");
    let lines: Vec<&str> = argv.lines().collect();
    // The wrapper records "$@", which excludes the wrapper itself ($0) —
    // its presence at the FRONT is proven by this file existing at all
    // (only the wrapper writes it): what follows is exactly the agent
    // command line the orchestrator built.
    assert_eq!(
        lines[0], AGENT,
        "the agent CLI is the first entry after the wrapper"
    );
    let model_positions: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == "--model")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        model_positions.len(),
        2,
        "exactly two --model occurrences (built-in + worker arg): {argv}"
    );
    let (first, second) = (model_positions[0], model_positions[1]);
    assert_eq!(lines[first + 1], "base-model", "built-in --model M first");
    assert_eq!(
        lines[second + 1],
        "from-worker-args",
        "the worker arg pair rides after it: {argv}"
    );
    assert!(second > first, "appended args follow the built-in flags");
    let p_idx = lines
        .iter()
        .position(|l| *l == "-p")
        .expect("prompt handoff present");
    assert!(
        p_idx > second,
        "-p @file stays LAST, after the appended args: {argv}"
    );
    assert_eq!(p_idx, lines.len() - 2, "-p is the final flag: {argv}");
    assert!(
        lines[lines.len() - 1].starts_with('@'),
        "the @prompt path is the very last argv entry: {argv}"
    );

    // ------------------------------------------------------------------
    // Arm 2: worker with NO args — the legacy path, byte-for-byte.
    // ------------------------------------------------------------------
    let f2 = fixture(
        r#"{ "tasks": [
            {"id":"A","title":"plain worker","scope":["DONE.txt"],"accept":"test -f DONE.txt"}
        ] }"#,
        &format!(
            r#"{{ "workers": [{{"name":"w1","provider":"p","model":"m","enabled":true,"cli":"{agent}"}}] }}"#,
            agent = AGENT.replace('\\', "\\\\")
        ),
        "no-args",
    );
    // No sandbox wrapper, no args: exactly the pre-feature dispatch shape.
    assert_eq!(
        run::run_loop(&f2.cfg, &f2.st, &RunOptions::default()),
        0,
        "a worker with no args must keep working exactly as before"
    );
    let st2 = Store::new(f2.st.state_dir.clone()).load();
    assert_eq!(st2["A"].state, TaskState::Done);
    assert!(
        f2.repo.join("DONE.txt").exists(),
        "the no-args worker's artifact still merges to main"
    );

    let _ = std::fs::remove_dir_all(&f.dir);
    let _ = std::fs::remove_dir_all(&f2.dir);
    let _ = std::fs::remove_dir_all(&d);
}
