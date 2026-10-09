//! Budget-cap tests (r8-budget-cap): a campaign's `max_wall_clock_s` is a
//! spend ceiling measured from the receipts. Once the in-scope tasks have
//! already consumed at least that many wall-clock seconds, NO new attempt is
//! dispatched (in-flight attempts still finish) and the run exits 3 — while
//! an already-complete campaign still exits 0.
//!
//! Harness follows `tests/e2e.rs::fixture`: a scratch git repo in a uniquely
//! named temp dir, repo-local git identity, config files in temp, and a
//! hand-built `Settings` whose agent CLI is the bundled `example_agent`.
//! Receipts are pre-seeded via `Store::append_receipt` with exact
//! `wall_clock_s` values so the meter is deterministic.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::{Receipt, Store, TaskStatus};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// The agent child inherits only an allowlist; the default `example_agent`
/// behaviour (write DONE.txt, commit, exit 0) needs no `FAKE_AGENT_*` knobs.
/// This guard keeps the env-sensitive tests logically serialized anyway, so
/// a stray knob removed/set by one never races another.
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

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(tasks_json: &str, workers_json: &str) -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-budget-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    // Repo-local identity: af's merges and the agent's commit need one, and
    // linked worktrees share this config.
    git(&repo, &["config", "user.name", "af test"]);
    git(&repo, &["config", "user.email", "af@test"]);
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
        agent_timeout_s: 10,
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
    let agent_escaped = AGENT.replace('\\', "\\\\");
    format!(
        r#"{{ "defaults": {{ "max_attempts": {max_attempts}, "accept_timeout_s": 10 }},
            "workers": [ {{ "name": "w1", "provider": "openai", "model": "gpt-4o",
                            "enabled": true, "cli": "{agent_escaped}" }} ] }}"#
    )
}

fn tasks_json(id: &str) -> String {
    format!(
        r#"{{ "tasks": [ {{"id":"{id}","title":"do the thing","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
        g = gate_cmd("DONE.txt")
    )
}

/// Pre-seed the ledger through the real receipt path with an exact spend.
fn seed_receipt(st: &Settings, task: &str, wall_clock_s: f64) {
    let store = Store::new(st.state_dir.clone());
    store
        .append_receipt(&Receipt {
            task: task.into(),
            attempt: 1,
            worker: "w1".into(),
            model: "gpt-4o".into(),
            wall_clock_s,
            tokens: None,
            ts: 1,
            outcome: "merged".into(),
            error: None,
            cost_micros: None,
        })
        .unwrap();
}

fn status_of(st: &Settings, id: &str) -> Option<TaskState> {
    Store::new(st.state_dir.clone())
        .load()
        .get(id)
        .map(|s| s.state.clone())
}

/// A ready task with the ledger already over the ceiling: the run must stop
/// WITHOUT dispatching, exit 3, and leave the ready task untouched. The
/// `-- agent --` log marker is written immediately before each agent spawn,
/// so its absence proves the agent was never invoked.
#[test]
fn a_campaign_budget_stops_new_dispatches() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let f = fixture(&tasks_json("A"), &worker_json(1));
    seed_receipt(&f.st, "A", 600.0);
    let mut st = f.st.clone();
    st.max_wall_clock_s = 500;

    let start = Instant::now();
    let code = run::run_loop(&f.cfg, &st, &RunOptions::default());
    let elapsed = start.elapsed();

    assert_eq!(code, 3, "stopped early: budget exhausted");
    // Ready (or absent = the Ready default) — not Failed, not Done.
    assert!(
        matches!(status_of(&st, "A"), None | Some(TaskState::Ready)),
        "A must be left Ready, got {:?}",
        status_of(&st, "A")
    );
    // The base repo is untouched: nothing was merged and no worktree exists.
    assert!(!f.repo.join("DONE.txt").exists(), "no work merged");
    assert!(
        !st.worktree_root.join("A").exists(),
        "no worktree was created"
    );
    // The agent was NEVER invoked: the marker is appended right before spawn.
    let log = st.state_dir.join("logs").join("A.log");
    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !log_text.contains("-- agent --"),
        "agent must not be spawned; log was: {log_text:?}"
    );
    // Termination, not spin: with poll_secs=1 the stop must be immediate.
    assert!(
        elapsed.as_secs() < 20,
        "budget stop must return promptly, took {elapsed:?}"
    );
}

/// Spend strictly below the ceiling: the task is dispatched and completes.
#[test]
fn a_campaign_below_the_budget_dispatches_and_completes() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let f = fixture(&tasks_json("A"), &worker_json(1));
    seed_receipt(&f.st, "A", 100.0);
    let mut st = f.st.clone();
    st.max_wall_clock_s = 500;

    let code = run::run_loop(&f.cfg, &st, &RunOptions::default());

    assert_eq!(code, 0, "below the cap the campaign completes");
    assert_eq!(status_of(&st, "A"), Some(TaskState::Done));
    assert!(f.repo.join("DONE.txt").exists(), "the attempt merged");
    // The dispatched attempt appended its own receipt on top of the seed.
    let receipts = Store::new(st.state_dir.clone()).load_receipts();
    assert!(
        receipts.len() >= 2,
        "a new attempt must leave a receipt: {receipts:?}"
    );
}

/// `max_wall_clock_s = 0` means unlimited: identical to today's behaviour,
/// even when the ledger is absurdly large.
#[test]
fn zero_budget_is_unlimited_legacy_behaviour() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let f = fixture(&tasks_json("A"), &worker_json(1));
    seed_receipt(&f.st, "A", 100_000.0);
    let mut st = f.st.clone();
    st.max_wall_clock_s = 0;

    let code = run::run_loop(&f.cfg, &st, &RunOptions::default());

    assert_eq!(code, 0, "0 = unlimited: legacy guarantee preserved");
    assert_eq!(status_of(&st, "A"), Some(TaskState::Done));
    assert!(f.repo.join("DONE.txt").exists(), "the attempt merged");
}

/// Completion wins over the budget: an all-Done campaign exits 0 even when
/// the measured spend is far above the cap, and nothing is re-dispatched.
#[test]
fn completion_wins_over_an_exhausted_budget() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let f = fixture(&tasks_json("A"), &worker_json(1));

    let mut status = HashMap::new();
    status.insert(
        "A".to_string(),
        TaskStatus {
            state: TaskState::Done,
            attempts: 1,
            ..Default::default()
        },
    );
    Store::new(f.st.state_dir.clone()).save(&status).unwrap();
    seed_receipt(&f.st, "A", 600.0);
    let mut st = f.st.clone();
    st.max_wall_clock_s = 500;

    let before = Store::new(st.state_dir.clone()).load_receipts().len();
    let code = run::run_loop(&f.cfg, &st, &RunOptions::default());
    let after = Store::new(st.state_dir.clone()).load_receipts().len();

    assert_eq!(code, 0, "an all-Done campaign exits 0 despite the spend");
    assert_eq!(
        after, before,
        "nothing is re-dispatched: no new receipts for finished work"
    );
    assert!(!f.repo.join("DONE.txt").exists(), "no new work was merged");
}

/// The budget stop must not spin: it returns the same exit code promptly
/// (well under the 20s bound) instead of looping on the ready-but-blocked
/// task. `poll_secs` is low (1) so a spin would be fast but non-terminating.
#[test]
fn a_budget_stop_does_not_spin() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let f = fixture(&tasks_json("A"), &worker_json(1));
    // Spend exactly AT the cap: the boundary is inclusive ("at or above").
    seed_receipt(&f.st, "A", 500.0);
    let mut st = f.st.clone();
    st.max_wall_clock_s = 500;
    assert_eq!(st.poll_secs, 1);

    let start = Instant::now();
    let code = run::run_loop(&f.cfg, &st, &RunOptions::default());
    let elapsed = start.elapsed();

    assert_eq!(code, 3);
    assert!(
        elapsed.as_secs() < 20,
        "a spin would not terminate; returned in {elapsed:?}"
    );
}

/// Dry-run surfaces the exhausted ceiling too — and still creates nothing
/// and exits 0. The `af` binary is used so the line can be observed on
/// stdout; `TF_MAX_WALL_CLOCK_S` is the env wiring under test.
#[test]
fn dry_run_surfaces_an_exhausted_budget() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    let f = fixture(&tasks_json("A"), &worker_json(1));
    seed_receipt(&f.st, "A", 600.0);

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_af"))
        .args(["run", "--dry-run"])
        .env("TF_TASKS_JSON", f.st.tasks_file.clone())
        .env("TF_WORKERS_JSON", f.st.workers_file.clone())
        .env("TF_STATE_DIR", f.st.state_dir.clone())
        .env("TF_REPO_DIR", f.repo.clone())
        .env("TF_WORKTREE_ROOT", f.st.worktree_root.clone())
        .env("TF_MAX_WALL_CLOCK_S", "500")
        .output()
        .expect("spawn af");

    assert_eq!(out.status.code(), Some(0), "dry-run always exits 0");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("BUDGET"), "plan surfaces the budget: {text}");
    assert!(text.contains("600.0"), "plan names the spend: {text}");
    assert!(text.contains("500"), "plan names the cap: {text}");
    // Dry-run creates nothing: no worktree, nothing merged.
    assert!(!f.repo.join("DONE.txt").exists());
    assert!(!f.st.worktree_root.join("A").exists());
}
