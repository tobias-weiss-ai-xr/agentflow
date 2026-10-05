//! Journal (effect sandwich) tests: an attempt's phase boundaries are
//! persisted (commit intent → perform effect → commit outcome, pi-durable)
//! so a crashed orchestrator resumes WITHOUT re-running the expensive,
//! non-replayable agent step.
//!
//! Same substrate as tests/e2e.rs (scratch git repo + `example_agent`):
//! no network, no LLM. Mutates process-global env vars used by
//! example_agent, so the tests share one mutex and run serialized.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::{AttemptPhase, Store};
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

fn commit_count(repo: &Path) -> usize {
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", "--first-parent", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("git must be available");
    assert!(out.status.success(), "rev-list failed");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
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
    // ride TF_AGENT_ENV_PASSTHROUGH (includes FAKE_AGENT_EXIT, which these
    // tests set to make an accidental agent re-dispatch observable).
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    let dir = std::env::temp_dir().join(format!("af-journal-{}-{n}", std::process::id()));
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

/// One task, one attempt, gate wants DONE.txt.
fn one_task_fixture() -> Fixture {
    fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"create done","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    )
}

/// Commit a file on branch `<prefix>/A` (the dead attempt's durable work),
/// then return the main checkout to its base branch.
fn seed_attempt_branch(f: &Fixture, branch: &str, file: &str, content: &str) {
    git(&f.repo, &["checkout", "-b", branch]);
    std::fs::write(f.repo.join(file), content).unwrap();
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-m", "dead attempt: agent work"]);
    git(&f.repo, &["checkout", "main"]);
}

/// Crash residue: `run-state.json` with task A stuck at `running` and the
/// journal at `phase`.
fn seed_running_state(f: &Fixture, phase: &str) {
    std::fs::create_dir_all(&f.st.state_dir).unwrap();
    std::fs::write(
        f.st.state_dir.join("run-state.json"),
        format!(r#"{{ "A": {{ "state": "running", "attempts": 1, "last_error": null, "phase": "{phase}" }} }}"#),
    )
    .unwrap();
}

/// A successful attempt leaves a journal: the persisted TaskStatus shows the
/// attempt reached at least AgentDone (and the task is Done).
// spec: state/attempt-phase-journal
// spec: state/attempt-phase-journal#phase-boundaries-persist-at-every-step
#[test]
fn journal_records_phase_after_successful_attempt() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = one_task_fixture();
    assert_eq!(
        run::run_loop(&f.cfg, &f.st, &RunOptions::default()),
        0,
        "run should exit 0 (all done)"
    );
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(
        matches!(
            st["A"].phase,
            Some(AttemptPhase::AgentDone) | Some(AttemptPhase::GatePassed)
        ),
        "journal must show the attempt reached at least AgentDone, got {:?}",
        st["A"].phase
    );
}

/// THE gate test: a crash after the agent finished must resume by re-running
/// only the gate + merge — the agent itself is never re-invoked. Proof:
/// FAKE_AGENT_EXIT=7 makes ANY agent invocation fail the attempt (max_attempts
/// is 1, so an accidental re-dispatch ends the run in exit 2 / Failed), yet
/// the resumed run exits 0 with the branch's work merged to main.
// spec: state/resume-and-self-heal
// spec: state/attempt-phase-journal#crash-after-agent-done-resumes-at-the-gate
#[test]
fn resume_from_agent_done_skips_agent() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("FAKE_AGENT_EXIT", "7");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    // FAKE_AGENT_EXIT rides the fixture's TF_AGENT_ENV_PASSTHROUGH into the
    // sandboxed agent child.
    let f = one_task_fixture();
    seed_attempt_branch(&f, "tf/A", "DONE.txt", "work from the dead attempt\n");
    seed_running_state(&f, "agent_done");

    assert_eq!(
        run::run_loop(&f.cfg, &f.st, &RunOptions::default()),
        0,
        "resume must complete the task without the agent"
    );
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(
        f.repo.join("DONE.txt").exists(),
        "gate re-ran on the branch and the merge landed on main"
    );
    assert!(
        !f.st.worktree_root.join("A").exists(),
        "resume worktree cleaned up"
    );
    assert!(
        Store::new(f.st.state_dir.clone())
            .load_receipts()
            .is_empty(),
        "no receipts — execute_task (and thus the agent) never ran"
    );
}

/// GatePassed resume: only the merge re-runs — exactly one new commit (the
/// merge), even though the agent would fail the attempt if invoked.
// spec: state/attempt-phase-journal#crash-after-gate-passed-merges-only
#[test]
fn resume_from_gate_passed_merges_only() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("FAKE_AGENT_EXIT", "7");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = one_task_fixture();
    seed_attempt_branch(&f, "tf/A", "DONE.txt", "work from the dead attempt\n");
    seed_running_state(&f, "gate_passed");

    let before = commit_count(&f.repo);
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(f.repo.join("DONE.txt").exists(), "branch merged to main");
    assert_eq!(
        commit_count(&f.repo),
        before + 1,
        "exactly one new first-parent commit on main: the resume merge \
         (no agent, no gate commit)"
    );
}

/// MergeOnly resume is idempotent when the merge already landed: the crash
/// happened after the merge (and its cleanup deleted the branch) but before
/// Done was persisted — record merged, no second merge commit.
// spec: state/status-persistence
// spec: state/status-persistence#crash-between-gate-and-merge-recording
#[test]
fn merge_only_resume_is_idempotent_when_already_merged() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("FAKE_AGENT_EXIT", "7");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = one_task_fixture();
    seed_attempt_branch(&f, "tf/A", "DONE.txt", "merged work\n");
    // The dead attempt got as far as merging (af's merge message format),
    // then its cleanup deleted the branch before Done was persisted.
    git(
        &f.repo,
        &["merge", "--no-ff", "tf/A", "-m", "af: A — create done"],
    );
    git(&f.repo, &["branch", "-D", "tf/A"]);
    seed_running_state(&f, "gate_passed");

    let before = commit_count(&f.repo);
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(f.repo.join("DONE.txt").exists());
    assert_eq!(
        commit_count(&f.repo),
        before,
        "already merged → record merged without a second merge"
    );
}

/// The journal's serialization half of `phase boundaries persist at every
/// step` (state spec): the persisted status file spells the phase in
/// `snake_case` (`"agent_done"`, `"gate_passed"`) — the exact format the
/// resume path parses back — and the value survives a save/load round-trip.
// spec: state/attempt-phase-journal#phase-boundaries-persist-at-every-step
#[test]
fn phase_serializes_snake_case_in_persisted_state() {
    use agentflow::config::TaskState as TS;
    use std::collections::HashMap;

    let dir = std::env::temp_dir().join(format!("af-journal-ser-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::new(dir.clone());
    let mut m: HashMap<String, agentflow::state::TaskStatus> = HashMap::new();
    m.insert(
        "A".to_string(),
        agentflow::state::TaskStatus {
            state: TS::Running,
            attempts: 1,
            last_error: None,
            phase: Some(AttemptPhase::AgentDone),
        },
    );
    store.save(&m).expect("save journaled state");

    let raw = std::fs::read_to_string(store.status_file()).unwrap();
    assert!(
        raw.contains("\"agent_done\""),
        "phase must serialize snake_case: {raw}"
    );
    assert_eq!(store.load()["A"].phase, Some(AttemptPhase::AgentDone));

    // Same for the later boundary.
    m.insert(
        "A".to_string(),
        agentflow::state::TaskStatus {
            phase: Some(AttemptPhase::GatePassed),
            ..m["A"].clone()
        },
    );
    store.save(&m).unwrap();
    let raw = std::fs::read_to_string(store.status_file()).unwrap();
    assert!(
        raw.contains("\"gate_passed\""),
        "gate boundary serializes snake_case: {raw}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
