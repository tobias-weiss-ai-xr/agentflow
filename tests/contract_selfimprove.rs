//! Per-contract acceptance demo for `verify: true` tasks.
//!
//! Drives `af run` (the `run::run_loop` entry point the `af run` CLI invokes)
//! on a scratch git repo with the bundled `example_agent`, declaring ONE
//! `verify: true` task whose acceptance gate emits an `accept_report.json`
//! with a per-contract verdict map. The contract for a verify task is:
//!
//! - the gate is the SOLE deliverable; passing it records `outcome: "verified"`,
//! - the branch is NEVER merged (the base branch is untouched; the work is
//!   archived under `<branch>-rejected-<now>`),
//! - the per-contract verdicts from `accept_report.json` ride the receipt
//!   (sorted by contract key), and
//! - `af status --json` surfaces `verify: true`, `verdict: "verified"` and the
//!   `contracts` map.
//!
//! This mirrors the `fixture()` / `ENV_GUARD` conventions of `tests/e2e.rs`
//! and `tests/gate_replay.rs`: a scratch git repo, the real `example_agent`
//! binary as the worker `cli`, and a serialized env-guard so the
//! process-global `FAKE_AGENT_*` knobs do not race parallel tests.

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
    // ride AF_AGENT_ENV_PASSTHROUGH (tests that override it set their own).
    std::env::set_var(
        "AF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT,FAKE_AGENT_ENV,FAKE_AGENT_ENV_NAMES",
    );
    let dir = std::env::temp_dir().join(format!("af-si-verify-{}-{n}", std::process::id()));
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

/// Every `tf/<id>-rejected-*` branch in `repo`, sorted. The archived form
/// every preserved failure/verify path uses (`<branch>-rejected-<now>`).
fn archived_refs(repo: &Path, id: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["branch", "--list", &format!("tf/{id}-rejected-*")])
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

/// A `verify: true` task whose gate passes AND emits an `accept_report.json`
/// with a per-contract verdict map reaches `verified` (NOT merged), surfaces
/// the per-contract verdicts on the receipt (sorted by key), and reports
/// `verdict: "verified"` with the contracts map through `af status --json`.
// spec: lifecycle/verify-tasks
// spec: lifecycle/verify-tasks#verify-task-passes-and-archives
#[test]
fn verify_task_emits_accept_report_reaches_verified_and_surfaces_contracts() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
    // The gate passes (DONE.txt exists, written by the example_agent) AND
    // writes accept_report.json with a per-contract map. The inner quotes are
    // JSON-escaped (`\"`) so the whole gate embeds cleanly in the tasks.json
    // `accept` string; the shell then receives a plain object. The contract
    // keys are intentionally out of file order (C-P2 before C-M1) so the
    // sorted-by-key invariant is exercised.
    let gate = format!(
        "{} && printf '%s' '{{\\\"C-P2\\\":\\\"fail\\\",\\\"C-M1\\\":\\\"pass\\\"}}' > accept_report.json",
        gate_cmd("DONE.txt")
    );
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"verify contracts","verify":true,"scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);

    // 1) The task ends Done with a `verified` verdict on its receipt (NOT
    //    merged). The base branch is untouched: DONE.txt must NOT exist in
    //    the repo because a verify task never merges.
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done, "verify task ends done");
    assert!(!f.repo.join("DONE.txt").exists(), "verify never merges");

    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1, "exactly one receipt: {receipts:?}");
    assert_eq!(
        receipts[0].outcome, "verified",
        "receipt records verified (not merged)"
    );
    let contracts = receipts[0]
        .contracts
        .as_ref()
        .expect("accept_report.json surfaces per-contract verdicts");
    assert_eq!(contracts.len(), 2, "both contracts recorded");
    // Sorted by contract key (BTreeMap): C-M1 then C-P2.
    assert_eq!(contracts[0].contract, "C-M1");
    assert_eq!(contracts[0].verdict, "pass");
    assert_eq!(contracts[1].contract, "C-P2");
    assert_eq!(contracts[1].verdict, "fail");

    // 2) The attempt branch is archived (not merged, not deleted): exactly
    //    one `tf/A-rejected-*` ref carries the agent's committed work.
    let archived = archived_refs(&f.repo, "A");
    assert_eq!(
        archived.len(),
        1,
        "exactly one archived ref (the verify work): {archived:?}"
    );

    // 3) `af status --json` reports `verdict: "verified"`, `verify: true`,
    //    and the per-contract map (sorted by key).
    let json = run::status_json(&f.cfg, &f.st);
    let v: serde_json::Value = serde_json::from_str(&json).expect("status --json is valid JSON");
    let task = v
        .get("A")
        .and_then(|t| t.as_object())
        .unwrap_or_else(|| panic!("task A present as object: {json}"));
    assert_eq!(
        task.get("verify").and_then(|x| x.as_bool()),
        Some(true),
        "status surfaces verify flag"
    );
    assert_eq!(
        task.get("verdict").and_then(|x| x.as_str()),
        Some("verified"),
        "status reports verdict verified: {json}"
    );
    let cs = task
        .get("contracts")
        .and_then(|c| c.as_array())
        .unwrap_or_else(|| panic!("contracts is an array: {json}"));
    assert_eq!(cs.len(), 2, "status surfaces both contracts");
    assert_eq!(cs[0]["contract"], "C-M1");
    assert_eq!(cs[0]["verdict"], "pass");
    assert_eq!(cs[1]["contract"], "C-P2");
    assert_eq!(cs[1]["verdict"], "fail");
}
