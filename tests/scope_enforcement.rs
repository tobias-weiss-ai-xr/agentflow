//! Scope enforcement E2E: `task.scope` is an enforced contract, not just a
//! sentence in the prompt. An agent edit outside the declared scope fails the
//! attempt BEFORE merge; an in-scope edit still merges (happy path intact).
//!
//! Same substrate as tests/e2e.rs (scratch git repo + bundled `example_agent`
//! binary). No network, no LLM. These tests mutate process-global env vars
//! used by example_agent, so they share one mutex and run serialized.

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
    // ride TF_AGENT_ENV_PASSTHROUGH (FAKE_AGENT_TOUCH is included here).
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    let dir = std::env::temp_dir().join(format!("af-scope-{}-{n}", std::process::id()));
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

/// Agent edits `out_of_scope.txt` while the task only allows `in_scope.txt`:
/// the attempt fails with an "out of scope" reason naming the offending path,
/// and the change is never merged into the base repo.
// spec: scheduling/scope-enforcement-on-agent-edits
// spec: scheduling/scope-enforcement-on-agent-edits#out-of-scope-edit-fails-before-merge
#[test]
fn out_of_scope_edit_fails_the_attempt() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"scope","scope":["in_scope.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("in_scope.txt")
        ),
        &worker_json(1),
    );
    // FAKE_AGENT_TOUCH rides the fixture's TF_AGENT_ENV_PASSTHROUGH.
    std::env::set_var("FAKE_AGENT_TOUCH", "out_of_scope.txt");

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "out-of-scope attempt must fail the run");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.as_deref().unwrap_or("");
    assert!(err.contains("out of scope"), "reason surfaced: {err}");
    assert!(
        err.contains("out_of_scope.txt"),
        "offending path named: {err}"
    );
    assert!(
        !f.repo.join("out_of_scope.txt").exists(),
        "offending change must NOT reach the base branch"
    );

    std::env::remove_var("FAKE_AGENT_TOUCH");
}

/// A scope violation must NEVER discard the committed work the attempt paid
/// for. The attempt branch is kept as a COPY under `<branch>-rejected-<now>`
/// so the work is recoverable, the failure message (and therefore the
/// receipt → `af api results`) names that branch, and the retry still starts
/// clean because the ORIGINAL branch is removed.
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#scope-violation-archives-the-committed-work
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#archiving-never-fails-the-attempt
#[test]
fn a_scope_violation_keeps_the_agents_work_on_an_archived_branch() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"scope","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    // The agent commits a file OUTSIDE the allowed scope (rogue.txt).
    std::env::set_var("FAKE_AGENT_TOUCH", "rogue.txt");

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "out-of-scope attempt must fail the run");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let err = st["A"].last_error.clone().unwrap_or_default();
    assert!(err.contains("out of scope"), "violation surfaced: {err}");
    assert!(err.contains("rogue.txt"), "offending path named: {err}");
    assert!(
        err.contains("work kept on branch") && err.contains("tf/A-rejected-"),
        "error names the archived branch: {err}"
    );
    // The receipt (what `af api results` reads) names the same branch.
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1);
    assert!(
        receipts[0]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("work kept on branch tf/A-rejected-"),
        "receipt names the archived branch: {receipts:?}"
    );
    // The archived branch is discoverable and really contains the work.
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
    // The archived branch really contains the agent's commit (recoverable).
    let show = std::process::Command::new("git")
        .args(["show", &format!("{name}:rogue.txt")])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    assert!(
        show.status.success(),
        "agent's committed change is recoverable on the archived branch: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    // The ORIGINAL branch is gone, so the retry starts clean from base.
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
}

/// Happy path guard: an in-scope edit is still merged and the task is Done.
// spec: scheduling/scope-enforcement-on-agent-edits#in-scope-edit-still-merges
#[test]
fn in_scope_edit_is_merged() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"scope","scope":["in_scope.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("in_scope.txt")
        ),
        &worker_json(1),
    );
    std::env::set_var("FAKE_AGENT_TOUCH", "in_scope.txt");

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "in-scope attempt must succeed");

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(
        f.repo.join("in_scope.txt").exists(),
        "in-scope change must be merged into the base repo"
    );

    std::env::remove_var("FAKE_AGENT_TOUCH");
}

/// A task that declares NO scope accepts every changed file (scheduling
/// spec): the agent's edit — to a file no entry names — still passes scope
/// enforcement and merges.
// spec: scheduling/scope-enforcement-on-agent-edits#empty-scope-allows-any-file
#[test]
fn empty_scope_accepts_any_edited_file() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"no declared scope","accept":"{g}"}} ] }}"#,
            g = gate_cmd("any_file.txt")
        ),
        &worker_json(1),
    );
    std::env::set_var("FAKE_AGENT_TOUCH", "any_file.txt");

    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(
        code, 0,
        "no declared scope ⇒ every changed file is accepted"
    );

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(
        f.repo.join("any_file.txt").exists(),
        "the unscoped edit merged into the base repo"
    );

    std::env::remove_var("FAKE_AGENT_TOUCH");
}
