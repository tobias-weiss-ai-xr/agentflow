//! Interrupted-attempt tests (r9-interrupted): an attempt lost when the
//! ORCHESTRATOR itself died must be RECORDED as `interrupted` — an honest
//! 0.0s duration plus a reason in `error`, because the true duration is
//! unknowable — and must stay OUT of worker trust: it is not a verdict on
//! the worker.
//!
//! Harness follows `tests/e2e.rs::fixture`: a scratch git repo in a uniquely
//! named temp dir, repo-local git identity, config files in temp, and a
//! hand-built `Settings`. Receipts are seeded through the real `Store`
//! append path so the report sees exactly the persisted ledger.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, CostFilter, RunOptions};
use agentflow::state::{AttemptPhase, Receipt, Store, TaskStatus};
use std::collections::HashMap;
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
    let dir = std::env::temp_dir().join(format!("af-interrupted-{}-{n}", std::process::id()));
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
        max_parallel: 1,
        branch_prefix: "tf".into(),
        poll_secs: 1,
        gate_env: vec![],
        tasks_file: config_dir.join("tasks.json"),
        workers_file: config_dir.join("workers.json"),
        prompt_file: dir.join("no-template.md"),
        agent_timeout_s: 10,
        agent_stall_s: 0,
        max_wall_clock_s: 0,
        sandbox_cmd: vec![],
    };
    Fixture { dir, cfg, st }
}

fn worker_json(max_attempts: u32) -> String {
    let agent_escaped = AGENT.replace('\\', "\\\\");
    format!(
        r#"{{ "defaults": {{ "max_attempts": {max_attempts}, "accept_timeout_s": 10 }},
            "workers": [ {{ "name": "w1", "provider": "openai", "model": "gpt-4o",
                            "enabled": true, "cli": "{agent_escaped}" }} ] }}"#
    )
}

/// One task only, so nothing is dispatchable after the heal marks it terminal.
fn tasks_json() -> String {
    r#"{ "tasks": [ {"id":"A","title":"do the thing","accept":"true"} ] }"#.to_string()
}

fn receipt(task: &str, attempt: u32, worker: &str, outcome: &str, ts: u64) -> Receipt {
    Receipt {
        task: task.into(),
        attempt,
        worker: worker.into(),
        model: "gpt-4o".into(),
        wall_clock_s: 0.0,
        tokens: None,
        ts,
        outcome: outcome.into(),
        error: None,
    }
}

/// (a) A task left `running` by a killed orchestrator gets an `interrupted`
/// receipt on the next startup heal, and (b) that receipt — even when it
/// names a real worker — never moves that worker's `WINS/TOTAL`: it is not
/// a verdict. The report also surfaces interrupted attempts distinctly.
// spec: state/cost-receipts
// spec: state/cost-receipts#interrupted-attempts-are-recorded-as-non-verdicts
// spec: cli/cost-report
// spec: cli/cost-report#interrupted-attempts-are-reported-distinctly
#[test]
fn a_killed_attempt_is_recorded_as_interrupted_and_excluded_from_trust() {
    let f = fixture(&tasks_json(), &worker_json(3));
    let store = Store::new(f.st.state_dir.clone());

    // A worker with a known track record on a DIFFERENT task: 2 merged,
    // 1 failed → trust 2/3. (Task B is intentionally absent from the config
    // so the heal run dispatches nothing.)
    for (attempt, outcome, ts) in [(1u32, "merged", 1u64), (2, "failed", 2)] {
        store
            .append_receipt(&receipt("B", attempt, "w1", outcome, ts))
            .unwrap();
    }
    store
        .append_receipt(&receipt("B", 3, "w1", "merged", 3))
        .unwrap();

    let before = run::cost(&f.cfg, &f.st, &CostFilter::default());
    assert!(
        before.contains(&format!("{:<14} {:<11} {:.2} {}", "w1", "2/3", 0.67, "-")),
        "seeded trust is 2/3 (COST is `-`: w1 declares no basis):\n{before}"
    );
    assert!(
        !before.contains("INTERRUPTED"),
        "no interrupted attempts yet — the report is unchanged:\n{before}"
    );

    // A task left mid-attempt by a killed orchestrator: no receipt exists,
    // and the persisted state still says `running`. `attempts: 2` plus the
    // running attempt is the third (and final) started attempt.
    let mut status = HashMap::new();
    status.insert(
        "A".to_string(),
        TaskStatus {
            state: TaskState::Running,
            attempts: 2,
            last_error: None,
            phase: Some(AttemptPhase::Spawned),
        },
    );
    store.save(&status).unwrap();

    // Startup heal through the PUBLIC run path. It records the lost attempt;
    // because the budget is exhausted it then goes terminal without dispatch,
    // so run_loop exits 2 (deadlock) — the receipt is what we assert on.
    let _ = run::run_loop(&f.cfg, &f.st, &RunOptions::default());

    let receipts = store.load_receipts();
    let interrupted: Vec<&Receipt> = receipts
        .iter()
        .filter(|r| r.outcome == "interrupted")
        .collect();
    assert_eq!(
        interrupted.len(),
        1,
        "the killed attempt is recorded exactly once: {receipts:?}"
    );
    assert_eq!(interrupted[0].task, "A", "the receipt names the task");
    assert_eq!(
        interrupted[0].attempt, 3,
        "the receipt names the dispatched attempt number"
    );
    assert_eq!(
        interrupted[0].wall_clock_s, 0.0,
        "the duration is honestly unknown, never invented"
    );
    let err = interrupted[0].error.as_deref().unwrap_or_default();
    assert!(
        err.contains("unknown"),
        "the reason says the duration is unknown: {err:?}"
    );

    // (b) Attribute an interrupted receipt to the real worker w1 directly
    // (heal cannot know the dead attempt's worker — it is not persisted) so
    // the trust exclusion is genuinely exercised, not just trivially true.
    store
        .append_receipt(&receipt("C", 1, "w1", "interrupted", 999))
        .unwrap();

    let after = run::cost(&f.cfg, &f.st, &CostFilter::default());
    assert!(
        after.contains(&format!("{:<14} {:<11} {:.2} {}", "w1", "2/3", 0.67, "-")),
        "interrupted receipts stay out of WINS/TOTAL (would be 2/4):\n{after}"
    );
    assert!(
        !after.contains("2/4"),
        "the interrupted attempt must not enter the trust denominator:\n{after}"
    );
    assert!(
        after.contains("INTERRUPTED"),
        "the report shows interrupted attempts distinctly:\n{after}"
    );
}

/// The heal writes the placeholder `unknown` for an attempt whose worker was
/// never persisted. That is agentflow admitting ignorance, NOT a worker that
/// went missing from the config, so the cost report must not footnote it as
/// one — the INTERRUPTED line already accounts for the attempt. A worker that
/// genuinely is gone from the config must still be footnoted.
// spec: cli/cost-report#the-interrupted-placeholder-is-not-a-missing-worker
// spec: cli/cost-report#receipts-naming-a-worker-absent-from-the-config-are-footnoted
// spec: cli/cost-report#interrupted-attempts-are-reported-distinctly
#[test]
fn the_interrupted_placeholder_worker_is_not_footnoted_as_missing() {
    let f = fixture(&tasks_json(), &worker_json(1));
    let store = Store::new(f.st.state_dir.clone());
    store
        .append_receipt(&receipt("A", 1, run::UNKNOWN_WORKER, "interrupted", 1))
        .unwrap();

    let out = run::cost(&f.cfg, &f.st, &CostFilter::default());
    assert!(
        out.contains("INTERRUPTED:"),
        "the attempt is still accounted for:\n{out}"
    );
    assert!(
        !out.contains("note:"),
        "the placeholder is not a worker that went missing:\n{out}"
    );

    // A worker that really is absent from the config still gets the note —
    // and the placeholder is never mixed into it.
    store
        .append_receipt(&receipt("A", 2, "ghost", "merged", 2))
        .unwrap();
    let out = run::cost(&f.cfg, &f.st, &CostFilter::default());
    let note = out
        .lines()
        .find(|l| l.starts_with("note:"))
        .unwrap_or_else(|| panic!("a note for the absent worker:\n{out}"));
    assert!(note.contains("ghost"), "the real stranger is named: {note}");
    assert!(
        !note.contains(run::UNKNOWN_WORKER),
        "the placeholder must never be named as missing: {note}"
    );
    assert_eq!(
        out.lines().filter(|l| l.starts_with("note:")).count(),
        1,
        "exactly one note line:\n{out}"
    );
}

/// The predicate is the single source of truth for the rule: only `merged`
/// and `failed` are verdicts on the worker.
// spec: state/cost-receipts#interrupted-attempts-are-recorded-as-non-verdicts
#[test]
fn interrupted_outcome_is_not_a_worker_verdict() {
    assert!(receipt("A", 1, "w1", "merged", 1).counts_as_verdict());
    assert!(receipt("A", 1, "w1", "failed", 1).counts_as_verdict());
    assert!(!receipt("A", 1, "w1", "interrupted", 1).counts_as_verdict());
}
