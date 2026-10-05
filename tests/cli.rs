//! CLI contract tests: spawn the real `af` binary and assert exit codes and
//! output shape (covers main() dispatch, load_cfg, and the command arms).
//!
//! The tail sections (`Spec-coverage gap tests`) hold small library-level
//! tests for spec requirements whose only existing coverage lives in src/
//! unit-test modules — kept here so every requirement in openspec/specs has
//! a `// spec:` marker in this file's allowed scope. See
//! docs/spec-traceability.md.

use agentflow::config;
use agentflow::config::{Config, Settings};
use agentflow::state::{Receipt, Store, TaskStatus};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

struct Cli {
    dir: PathBuf,
}

impl Cli {
    fn new() -> Cli {
        Cli::new_with_tasks(TASKS)
    }

    fn new_with_tasks(tasks: &str) -> Cli {
        Cli::new_with(tasks, WORKERS)
    }

    fn new_with(tasks: &str, workers: &str) -> Cli {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("af-cli-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config_dir = dir.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("tasks.json"), tasks).unwrap();
        std::fs::write(config_dir.join("workers.json"), workers).unwrap();
        Cli { dir }
    }

    fn af(&self, args: &[&str]) -> (i32, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_af"))
            .args(args)
            .env("TF_TASKS_JSON", self.dir.join("config").join("tasks.json"))
            .env(
                "TF_WORKERS_JSON",
                self.dir.join("config").join("workers.json"),
            )
            .env("TF_STATE_DIR", self.dir.join("state"))
            .env("TF_REPO_DIR", self.dir.join("repo"))
            .env("TF_WORKTREE_ROOT", self.dir.join("wt"))
            .output()
            .expect("spawn af");
        (
            out.status.code().unwrap_or(-1),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    }

    fn settings(&self) -> (Config, Settings) {
        let cfg = config::load(
            &self.dir.join("config").join("tasks.json"),
            &self.dir.join("config").join("workers.json"),
        )
        .unwrap();
        let st = Settings {
            repo_dir: self.dir.join("repo"),
            state_dir: self.dir.join("state"),
            worktree_root: self.dir.join("wt"),
            max_parallel: 2,
            branch_prefix: "tf".into(),
            poll_secs: 1,
            gate_env: vec![],
            tasks_file: self.dir.join("config").join("tasks.json"),
            workers_file: self.dir.join("config").join("workers.json"),
            prompt_file: self.dir.join("no-template.md"),
            agent_timeout_s: 60,
            sandbox_cmd: vec![],
        };
        (cfg, st)
    }

    fn seed_done_task(&self, id: &str) {
        let (_, st) = self.settings();
        let mut map = HashMap::new();
        let s = TaskStatus {
            state: agentflow::config::TaskState::Done,
            attempts: 1,
            ..Default::default()
        };
        map.insert(id.to_string(), s);
        Store::new(st.state_dir.clone()).save(&map).unwrap();
    }

    fn seed_receipt(&self, id: &str) {
        let (_, st) = self.settings();
        let store = Store::new(st.state_dir.clone());
        store
            .append_receipt(&Receipt {
                task: id.into(),
                attempt: 1,
                worker: "w1".into(),
                model: "m".into(),
                wall_clock_s: 1.5,
                tokens: None,
                ts: 1,
                outcome: "merged".into(),
                error: None,
            })
            .unwrap();
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const TASKS: &str = r#"{ "tasks": [ { "id": "A", "title": "t", "scope": ["DONE.txt"], "accept": "test -f DONE.txt" } ] }"#;
const WORKERS: &str = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
    "workers": [ { "name": "w1", "provider": "openai", "model": "gpt-4o", "enabled": true, "cli": "unused" } ] }"#;

#[test]
fn help_and_version_exit_zero() {
    let cli = Cli::new();
    let (code, out) = cli.af(&["--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("USAGE"));
    let (code, out) = cli.af(&["--version"]);
    assert_eq!(code, 0);
    assert!(out.contains("af "));
}

#[test]
fn unknown_command_and_flag_exit_two() {
    let cli = Cli::new();
    assert_eq!(cli.af(&["frobnicate"]).0, 2);
    assert_eq!(cli.af(&["run", "--nope"]).0, 2);
}

#[test]
fn missing_config_errors_exit_two() {
    let cli = Cli::new();
    let out = Command::new(env!("CARGO_BIN_EXE_af"))
        .args(["status"])
        .env("TF_TASKS_JSON", cli.dir.join("config").join("nope.json"))
        .env(
            "TF_WORKERS_JSON",
            cli.dir.join("config").join("workers.json"),
        )
        .env("TF_STATE_DIR", cli.dir.join("state"))
        .env("TF_REPO_DIR", cli.dir.join("repo"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("config error"));
}

// spec: cli/status-and-inspection-commands
// spec: cli/status-and-inspection-commands#json-status
#[test]
fn status_and_api_status_render_board() {
    let cli = Cli::new();
    cli.seed_done_task("A");
    let (code, out) = cli.af(&["status"]);
    assert_eq!(code, 0);
    assert!(out.contains("A") && out.contains("Done"));
    let (code, out) = cli.af(&["api", "status", "--json"]);
    assert_eq!(code, 0);
    assert!(out.contains("\"A\""));
}

#[test]
fn status_json_emits_parseable_board_with_task_fields() {
    let cli = Cli::new();
    cli.seed_done_task("A");
    // `af status --json` must emit machine-parseable JSON, not the human board.
    let (code, out) = cli.af(&["status", "--json"]);
    assert_eq!(code, 0, "status --json exits 0: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("status --json is valid JSON");
    let task = v
        .get("A")
        .and_then(|t| t.as_object())
        .unwrap_or_else(|| panic!("task A present as object: {out}"));
    assert_eq!(task.get("id").and_then(|x| x.as_str()), Some("A"));
    assert_eq!(task.get("state").and_then(|x| x.as_str()), Some("done"));
    assert_eq!(task.get("attempts").and_then(|x| x.as_u64()), Some(1));

    // Plain `af status` keeps printing the human board.
    let (code, out) = cli.af(&["status"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("TASK") && out.contains("ATTEMPTS"),
        "human board: {out}"
    );
    assert!(
        out.contains("A") && out.contains("Done"),
        "human board row: {out}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&out).is_err(),
        "plain status must not be JSON: {out}"
    );
}

#[test]
fn api_results_requires_task_flag() {
    let cli = Cli::new();
    assert_eq!(cli.af(&["api", "results"]).0, 2);
    cli.seed_done_task("A");
    cli.seed_receipt("A");
    let (code, out) = cli.af(&["api", "results", "--task", "A"]);
    assert_eq!(code, 0);
    assert!(out.contains("task A:"), "results header: {out}");
    assert!(!out.contains("no such task"));
    let (code, out) = cli.af(&["api", "results", "--task", "NOPE"]);
    assert_eq!(code, 0);
    assert!(out.contains("no such task"));
}

// spec: cli/status-and-inspection-commands
// spec: cli/status-and-inspection-commands#attach-tails-log
#[test]
fn attach_requires_id_and_tails_log_until_done() {
    let cli = Cli::new();
    assert_eq!(cli.af(&["attach"]).0, 2);
    cli.seed_done_task("A");
    let (_, st) = cli.settings();
    let logs = st.state_dir.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("A.log"), "== attempt 1 on w1 ==").unwrap();
    let (code, out) = cli.af(&["attach", "A"]);
    assert_eq!(code, 0);
    assert!(out.contains("== attempt 1 on w1 =="), "tails the log");
}

// spec: state/cost-receipts
#[test]
fn cost_prints_table_and_task_filter() {
    let cli = Cli::new();
    cli.seed_receipt("A");
    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0);
    assert!(out.contains("w1") && out.contains("1.5"), "task row: {out}");
    assert!(out.contains("TOTAL"));
    let (code, out) = cli.af(&["cost", "--task", "ZZ"]);
    assert_eq!(code, 0);
    assert!(!out.contains("1.5"), "filter excludes other tasks: {out}");
    assert!(out.contains("TOTAL: 0.0s"));
}

// spec: state/cost-receipts#cost-aggregates-receipts
#[test]
fn cost_last_shows_the_latest_attempt_per_task() {
    // Several attempts per task with distinct ts: `af cost` totals every
    // attempt, while `af cost --last` totals ONE attempt per task — the
    // receipt with the greatest ts — reporting ATTEMPTS 1, and the trust
    // block reflects only those selected receipts.
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "B", "title": "b", "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt =
        |task: &str, attempt: u32, ts: u64, wall: f64, worker: &str, outcome: &str| Receipt {
            task: task.into(),
            attempt,
            worker: worker.into(),
            model: "m".into(),
            wall_clock_s: wall,
            tokens: None,
            ts,
            outcome: outcome.into(),
            error: None,
        };
    for r in [
        receipt("A", 1, 1_000, 30.0, "w1", "failed"),
        receipt("A", 2, 2_000, 12.0, "w2", "merged"),
        receipt("B", 1, 3_000, 5.0, "w1", "merged"),
    ] {
        store.append_receipt(&r).unwrap();
    }

    // Plain `af cost`: every attempt is totaled and counted.
    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("TOTAL: 47.0s across 3 receipt(s)"), "{out}");
    assert!(
        out.contains(&format!("{:<12} {:<9}", "A", 2)),
        "A shows both attempts: {out}"
    );
    assert!(
        out.contains(&format!("{:<14} {:<11} {:.2}", "w1", "1/2", 0.50)),
        "trust over every receipt: {out}"
    );

    // `af cost --last`: one attempt per task, ATTEMPTS 1, retries not
    // double-counted.
    let (code, out) = cli.af(&["cost", "--last"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("TOTAL: 17.0s across 2 receipt(s)"), "{out}");
    assert!(
        out.contains(&format!("{:<12} {:<9} {:<10.1}", "A", 1, 12.0)),
        "A row: {out}"
    );
    assert!(
        out.contains(&format!("{:<12} {:<9} {:<10.1}", "B", 1, 5.0)),
        "B row: {out}"
    );
    assert!(
        !out.contains("30.0"),
        "the older failed attempt is not counted: {out}"
    );
    // Trust reflects ONLY the selected receipts: w1 now 1/1 (B's merge),
    // w2 1/1 (A's latest merge) — A's failed attempt on w1 is gone.
    assert!(
        out.contains(&format!("{:<14} {:<11} {:.2}", "w1", "1/1", 1.00)),
        "{out}"
    );
    assert!(
        out.contains(&format!("{:<14} {:<11} {:.2}", "w2", "1/1", 1.00)),
        "{out}"
    );

    // --last composes with --task: the table narrows to A's latest.
    let (code, out) = cli.af(&["cost", "--last", "--task", "A"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(&format!("{:<12} {:<9} {:<10.1}", "A", 1, 12.0)),
        "{out}"
    );
    assert!(
        !out.lines()
            .any(|l| l.split_whitespace().next() == Some("B")),
        "B has no row under --task A: {out}"
    );
}

#[test]
fn cost_since_filters_receipts_by_time() {
    let cli = Cli::new();
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt = |attempt: u32, ts: u64, wall: f64, worker: &str, outcome: &str| Receipt {
        task: "A".into(),
        attempt,
        worker: worker.into(),
        model: "m".into(),
        wall_clock_s: wall,
        tokens: None,
        ts,
        outcome: outcome.into(),
        error: None,
    };
    for r in [
        receipt(1, 1_600_000_000, 100.0, "w1", "failed"), // 2020-09-13
        receipt(2, 1_700_000_000, 20.0, "w1", "failed"),  // 2023-11-14
        receipt(3, 1_750_000_000, 30.0, "w2", "merged"),  // 2024-06-15
    ] {
        store.append_receipt(&r).unwrap();
    }

    // A bare unix timestamp excludes older receipts and keeps newer ones.
    let (code, out) = cli.af(&["cost", "--since", "1650000000"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("TOTAL: 50.0s across 2 receipt(s)"), "{out}");
    assert!(!out.contains("100.0"), "2020 receipt excluded: {out}");
    assert!(
        out.contains(&format!("{:<14} {:<11} {:.2}", "w1", "0/1", 0.00)),
        "trust reflects only the windowed receipts: {out}"
    );

    // The bound is inclusive: ts >= since keeps the boundary receipt.
    let (code, out) = cli.af(&["cost", "--since", "1600000000"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("TOTAL: 150.0s across 3 receipt(s)"), "{out}");

    // YYYY-MM-DD is UTC midnight: 2023-01-01 = 1672531200.
    let (code, out) = cli.af(&["cost", "--since", "2023-01-01"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("TOTAL: 50.0s across 2 receipt(s)"), "{out}");

    // --since composes with --last: the latest attempt per task WITHIN the
    // window; the trust block reflects only that selected receipt.
    let (code, out) = cli.af(&["cost", "--since", "1650000000", "--last"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("TOTAL: 30.0s across 1 receipt(s)"), "{out}");
    assert!(
        out.contains(&format!("{:<12} {:<9}", "A", 1)),
        "ATTEMPTS is 1 under --last: {out}"
    );
    assert!(
        out.contains(&format!("{:<14} {:<11} {:.2}", "w2", "1/1", 1.00)),
        "{out}"
    );
    assert!(
        !out.contains("w1"),
        "w1's windowed-out attempts leave no trust row: {out}"
    );

    // An unparseable value is a clear error with a non-zero exit — and an
    // impossible calendar date is rejected too.
    let (code, out) = cli.af(&["cost", "--since", "not-a-date"]);
    assert_ne!(code, 0, "bad --since must exit non-zero: {out}");
    assert!(out.contains("--since"), "error names the flag: {out}");
    assert!(out.contains("not-a-date"), "error names the value: {out}");
    let (code, out) = cli.af(&["cost", "--since", "2024-13-01"]);
    assert_ne!(code, 0, "impossible month must exit non-zero: {out}");
}

// spec: state/cost-receipts
#[test]
fn cost_report_surfaces_wasted_spend() {
    // Failed attempts must not be invisible: the report totals their
    // wall-clock, breaks it down by reason, and the whole waste section is
    // computed over the SAME window-selected receipts as the rest of the
    // report (so `--last` / `--since` narrow it too).
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "B", "title": "b", "accept": "true" },
        { "id": "C", "title": "c", "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt =
        |task: &str, attempt: u32, ts: u64, wall: f64, outcome: &str, error: Option<&str>| {
            Receipt {
                task: task.into(),
                attempt,
                worker: "w1".into(),
                model: "m".into(),
                wall_clock_s: wall,
                tokens: None,
                ts,
                outcome: outcome.into(),
                error: error.map(str::to_string),
            }
        };
    for r in [
        receipt(
            "A",
            1,
            1_000,
            100.0,
            "failed",
            Some("acceptance gate failed (exit 1): boom"),
        ),
        receipt("A", 2, 2_000, 10.0, "merged", None),
        receipt(
            "B",
            1,
            3_000,
            20.0,
            "failed",
            Some("agent exited NonZero (code 7)"),
        ),
        receipt("C", 1, 4_000, 5.0, "merged", None),
    ] {
        store.append_receipt(&r).unwrap();
    }

    // Plain `af cost`: waste section is always present — exact summed
    // failed seconds, failed/total attempt counts, and the percentage.
    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("WASTED BY REASON"), "header: {out}");
    assert!(
        out.contains("WASTED: 120.0s on 2 of 4 attempt(s) (50.0%)"),
        "waste total: {out}"
    );
    assert!(
        out.contains("acceptance gate failed (exit 1): boom"),
        "reason key: {out}"
    );
    assert!(out.contains("100.0s"), "first reason seconds: {out}");
    assert!(
        out.contains("agent exited NonZero (code 7)"),
        "second reason key: {out}"
    );
    assert!(out.contains("20.0s"), "second reason seconds: {out}");

    // `--last` drops A's older failed attempt: only B's failure survives,
    // so the waste figure narrows to 20.0s of 3 receipts — proving the
    // waste section shares the window selection rather than restating the
    // full-report number.
    let (code, out) = cli.af(&["cost", "--last"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("WASTED: 20.0s on 1 of 3 attempt(s) (33.3%)"),
        "windowed waste total: {out}"
    );
    assert!(
        !out.contains("acceptance gate failed"),
        "the windowed-out failure is not reported: {out}"
    );
    assert!(
        out.contains("agent exited NonZero (code 7)"),
        "the surviving failure is reported: {out}"
    );

    // A window with no failures reports 0.0s explicitly (no panic, no
    // division by zero) and omits the reason breakdown.
    let (code, out) = cli.af(&["cost", "--since", "4000"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("WASTED: 0.0s on 0 of 1 attempt(s) (0.0%)"),
        "zero-failure window: {out}"
    );
    assert!(
        !out.contains("WASTED BY REASON"),
        "no reasons when there are no failures: {out}"
    );
}

// spec: cli/run-commands
#[test]
fn dry_run_prints_plan_without_spawning_agents() {
    let cli = Cli::new();
    let (code, out) = cli.af(&["run", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(out.contains("Dry run"));
    assert!(out.contains("A"));
}

#[test]
fn dry_run_lists_every_task_with_depth_and_readiness() {
    // A (depth 0), C (depth 0), B deps A (depth 1): the plan must show ALL
    // tasks — including blocked ones — with depth and readiness columns.
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "deps": [], "accept": "true" },
        { "id": "B", "title": "b", "deps": ["A"], "accept": "true" },
        { "id": "C", "title": "c", "deps": [], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (code, out) = cli.af(&["run", "--dry-run"]);
    assert_eq!(code, 0);
    for id in ["A", "B", "C"] {
        assert!(out.contains(id), "task {id} missing from plan: {out}");
    }
    assert!(out.contains("DEPTH"), "depth column header: {out}");
    assert!(out.contains("READINESS"), "readiness column header: {out}");
    assert!(out.contains("ready"), "A and C ready: {out}");
    assert!(
        out.contains("blocked by A"),
        "B must name its pending dep: {out}"
    );
    // Wave order: depth-0 roots (A, C) listed before depth-1 B.
    let pos_a = out
        .find("A ")
        .or_else(|| out.find("A\n"))
        .unwrap_or(usize::MAX);
    let pos_b = out.find("B ").unwrap_or(usize::MAX);
    let pos_c = out.find("C ").unwrap_or(usize::MAX);
    assert!(pos_a < pos_b, "A (depth 0) before B (depth 1): {out}");
    assert!(pos_c < pos_b, "C (depth 0) before B (depth 1): {out}");
    assert!(out.contains("2 ready now"), "summary counts ready: {out}");
}

#[test]
fn help_lists_validate_subcommand() {
    let cli = Cli::new();
    let (code, out) = cli.af(&["--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("validate"), "USAGE mentions validate: {out}");
}

#[test]
fn validate_reports_ok_and_exits_zero() {
    let cli = Cli::new();
    let (code, out) = cli.af(&["validate"]);
    assert_eq!(code, 0, "valid config exits 0: {out}");
    assert!(out.contains("config OK"), "summary line: {out}");
    assert!(out.contains("1 task"), "task count: {out}");
    assert!(out.contains("1 enabled"), "enabled count: {out}");
    // Pre-flight must not touch state or dispatch anything.
    assert!(
        !cli.dir.join("state").exists(),
        "validate must not create state dirs"
    );
}

// spec: config/task-schema-loading
// spec: config/task-schema-loading#dependency-cycle-rejected
// spec: scheduling/dependency-dag
// spec: scheduling/dependency-dag#cycle-rejected
#[test]
fn validate_cycle_exits_nonzero_and_names_the_cycle() {
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "deps": ["B"], "accept": "true" },
        { "id": "B", "title": "b", "deps": ["A"], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (code, out) = cli.af(&["validate"]);
    assert_ne!(code, 0, "cyclic config must exit nonzero: {out}");
    assert!(
        out.contains("config error"),
        "surfaced as config error: {out}"
    );
    assert!(
        out.contains("dependency cycle"),
        "error names the cycle: {out}"
    );
    assert!(
        out.contains("A") && out.contains("B"),
        "cycle members reported: {out}"
    );
}

// spec: config/task-schema-loading
// spec: config/task-schema-loading#duplicate-id-rejected
#[test]
fn validate_duplicate_id_exits_nonzero() {
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "A", "title": "dup", "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (code, out) = cli.af(&["validate"]);
    assert_ne!(code, 0, "duplicate ids must exit nonzero: {out}");
    assert!(out.contains("duplicate task id"), "error text: {out}");
}

// spec: config/worker-schema-loading
// spec: config/worker-schema-loading#zero-enabled-workers-rejected
#[test]
fn validate_no_enabled_workers_exits_nonzero() {
    let workers =
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m", "enabled": false } ] }"#;
    let cli = Cli::new_with(TASKS, workers);
    let (code, out) = cli.af(&["validate"]);
    assert_ne!(code, 0, "no enabled workers must exit nonzero: {out}");
    assert!(out.contains("no enabled workers"), "error text: {out}");
}

// spec: config/task-schema-loading#task-without-gate-and-not-manual-warns
#[test]
fn validate_prints_warnings_without_failing() {
    // Gate-less non-manual task: a warning, not a hard error.
    let tasks = r#"{ "tasks": [ { "id": "A", "title": "t" } ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (code, out) = cli.af(&["validate"]);
    assert_eq!(code, 0, "warnings must not fail validation: {out}");
    assert!(out.contains("warning:"), "warning printed: {out}");
    assert!(out.contains("1 warning(s)"), "warning counted: {out}");
}

#[test]
fn validate_rejects_unknown_worker_filter() {
    let cli = Cli::new();
    let (code, out) = cli.af(&["validate", "--worker", "ghost"]);
    assert_ne!(code, 0, "unknown worker must exit nonzero: {out}");
    assert!(out.contains("ghost"), "error names the worker: {out}");
    // A known enabled worker passes the pre-flight.
    let (code, out) = cli.af(&["validate", "--worker", "w1"]);
    assert_eq!(code, 0, "known worker validates: {out}");
    assert!(out.contains("config OK"), "{out}");
}

#[test]
fn clean_dry_run_reports_and_real_clean_removes_orphan_worktree() {
    let cli = Cli::new();
    let orphan = cli.dir.join("wt").join("ORPHAN");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("junk.txt"), "leftover\n").unwrap();

    // --dry-run prints the orphan but changes nothing.
    let (code, out) = cli.af(&["clean", "--dry-run"]);
    assert_eq!(code, 0, "dry run exits 0: {out}");
    assert!(out.contains("ORPHAN"), "dry run names the orphan: {out}");
    assert!(orphan.exists(), "dry run must not remove anything");

    // Real clean removes the orphan dir and exits 0.
    let (code, out) = cli.af(&["clean"]);
    assert_eq!(code, 0, "clean exits 0: {out}");
    assert!(out.contains("ORPHAN"), "clean reports the orphan: {out}");
    assert!(!orphan.exists(), "orphan worktree dir is gone after clean");
}

#[test]
fn clean_with_nothing_to_do_exits_zero() {
    let cli = Cli::new();
    let (code, out) = cli.af(&["clean"]);
    assert_eq!(code, 0, "empty clean exits 0: {out}");
    assert!(
        out.contains("no orphaned worktrees"),
        "reports nothing: {out}"
    );
}

#[test]
fn dry_run_reflects_persisted_state_in_readiness() {
    // A done from a previous run → B becomes ready; terminal states surface
    // as done/failed instead of silently disappearing from the plan.
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "deps": [], "accept": "true" },
        { "id": "B", "title": "b", "deps": ["A"], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    cli.seed_done_task("A");
    let (code, out) = cli.af(&["run", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(out.contains("A"), "done A still listed: {out}");
    assert!(
        out.contains("done") && !out.contains("blocked by A"),
        "A shown as done, B unblocked: {out}"
    );
}

// ---------------------------------------------------------------------------
// Spec-coverage gap tests (see docs/spec-traceability.md).
//
// Small library-level tests for requirements whose only existing coverage
// lives in src/ unit-test modules (outside this task's file scope). They
// reference the config and scheduling spec libraries directly.
// ---------------------------------------------------------------------------

// spec: cli/cost-report
// spec: cli/cost-report#trust-section-lists-each-worker-with-history
#[test]
fn cost_prints_per_worker_trust_section() {
    let cli = Cli::new();
    // The spec's exact scenario: w1 has 2 merged + 1 failed, w2 has 1 merged.
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    for (worker, task, outcome) in [
        ("w1", "A", "merged"),
        ("w1", "A", "merged"),
        ("w1", "A", "failed"),
        ("w2", "A", "merged"),
    ] {
        store
            .append_receipt(&Receipt {
                task: task.into(),
                attempt: 1,
                worker: worker.into(),
                model: "m".into(),
                wall_clock_s: 1.0,
                tokens: None,
                ts: 1,
                outcome: outcome.into(),
                error: None,
            })
            .unwrap();
    }
    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "cost exits 0: {out}");
    assert!(
        out.contains("WORKER") && out.contains("TRUST"),
        "header: {out}"
    );
    // The spec scenario rows, in the cost table's padded column format.
    assert!(
        out.contains(&format!("{:<14} {:<11} {}", "w1", "2/3", "0.67")),
        "w1 trust row: {out}"
    );
    assert!(
        out.contains(&format!("{:<14} {:<11} {}", "w2", "1/1", "1.00")),
        "w2 trust row: {out}"
    );
}

// spec: config/task-schema-loading
// spec: config/task-schema-loading#valid-config-loads
// spec: config/task-schema-loading#dangling-dependency-warns
#[test]
fn config_task_schema_loads_validates_and_warns() {
    let dir = std::env::temp_dir().join(format!("af-cli-cfg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config_dir = dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();

    // Valid load: two tasks, one dependency, string + numeric priorities.
    std::fs::write(
        config_dir.join("tasks.json"),
        r#"{ "_meta": { "project": "x" }, "tasks": [
            { "id": "A", "title": "a", "priority": "HIGH", "accept": "true" },
            { "id": "B", "title": "b", "deps": ["A"], "priority": 3, "accept": "true" }
        ] }"#,
    )
    .unwrap();
    std::fs::write(
        config_dir.join("workers.json"),
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m", "enabled": true } ] }"#,
    )
    .unwrap();
    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .expect("valid config loads");
    assert_eq!(cfg.tasks.len(), 2);
    assert!(cfg.by_id.contains_key("B"), "dependency graph resolves");
    assert!(cfg.warnings.is_empty(), "no warnings: {:?}", cfg.warnings);

    // Dangling dep and gate-less non-manual task: WARN, never fail.
    std::fs::write(
        config_dir.join("tasks.json"),
        r#"{ "tasks": [
            { "id": "A", "title": "a", "deps": ["GHOST"] },
            { "id": "B", "title": "b", "accept": "true" }
        ] }"#,
    )
    .unwrap();
    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .expect("dangling dep is a warning, not an error");
    assert!(
        cfg.warnings.iter().any(|w| w.contains("GHOST")),
        "warning names the missing dep: {:?}",
        cfg.warnings
    );
    assert!(
        cfg.warnings
            .iter()
            .any(|w| w.contains("gate will be skipped")),
        "warning names the gate-less task: {:?}",
        cfg.warnings
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// spec: config/worker-schema-loading
// spec: config/worker-schema-loading#valid-workers-load
#[test]
fn config_worker_schema_loads_enabled_workers() {
    let dir = std::env::temp_dir().join(format!("af-cli-workers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("tasks.json"),
        r#"{ "tasks": [ { "id": "A", "title": "t", "accept": "true" } ] }"#,
    )
    .unwrap();
    // Two enabled workers with the documented fields → both parse.
    std::fs::write(
        dir.join("workers.json"),
        r#"{ "defaults": { "max_attempts": 2 },
            "workers": [
                { "name": "w1", "provider": "openai", "model": "gpt-4o", "enabled": true },
                { "name": "w2", "provider": "zai", "model": "glm-5.2", "api_base": "http://x", "enabled": true }
            ] }"#,
    )
    .unwrap();
    let cfg = config::load(&dir.join("tasks.json"), &dir.join("workers.json"))
        .expect("two enabled workers load");
    assert_eq!(cfg.workers.len(), 2);
    assert_eq!(cfg.defaults.max_attempts, 2);

    // Duplicate worker names are rejected.
    std::fs::write(
        dir.join("workers.json"),
        r#"{ "workers": [
                { "name": "w1", "provider": "p", "model": "m" },
                { "name": "w1", "provider": "p", "model": "m" }
            ] }"#,
    )
    .unwrap();
    let err = config::load(&dir.join("tasks.json"), &dir.join("workers.json"))
        .expect_err("duplicate worker names must fail");
    assert!(err.contains("w1"), "error names the duplicate: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}

// spec: config/environment-overrides
// spec: config/environment-overrides#overrides-applied
// spec: config/environment-overrides#defaults-when-unset
#[test]
fn settings_from_env_honors_tf_overrides_and_defaults() {
    // Mutates process-global env; every other test in this binary spawns
    // `af` with explicit .env() overrides (or never reads these vars), so
    // the window is safe. Vars are removed again before the default arm.
    for k in [
        "TF_REPO_DIR",
        "TF_STATE_DIR",
        "TF_MAX_PARALLEL",
        "TF_BRANCH_PREFIX",
        "TF_POLL",
        "TF_GATE_ENV",
    ] {
        std::env::remove_var(k);
    }
    // Defaults when unset: poll = 15s, branch prefix = tf.
    let st = Settings::from_env();
    assert_eq!(st.poll_secs, 15, "default poll interval");
    assert_eq!(st.branch_prefix, "tf", "default branch prefix");
    assert_eq!(st.max_parallel, 0, "0 = one per enabled worker");

    // Overrides applied.
    std::env::set_var("TF_REPO_DIR", "/tmp/ov-repo");
    std::env::set_var("TF_STATE_DIR", "/tmp/ov-state");
    std::env::set_var("TF_MAX_PARALLEL", "2");
    std::env::set_var("TF_BRANCH_PREFIX", "ov");
    std::env::set_var("TF_POLL", "5");
    std::env::set_var("TF_GATE_ENV", "K=1");
    let st = Settings::from_env();
    assert_eq!(st.repo_dir, std::path::PathBuf::from("/tmp/ov-repo"));
    assert_eq!(st.state_dir, std::path::PathBuf::from("/tmp/ov-state"));
    assert_eq!(st.max_parallel, 2);
    assert_eq!(st.branch_prefix, "ov");
    assert_eq!(st.poll_secs, 5);
    assert_eq!(st.gate_env, vec![("K".to_string(), "1".to_string())]);

    for k in [
        "TF_REPO_DIR",
        "TF_STATE_DIR",
        "TF_MAX_PARALLEL",
        "TF_BRANCH_PREFIX",
        "TF_POLL",
        "TF_GATE_ENV",
    ] {
        std::env::remove_var(k);
    }
}

// spec: config/optional-repos-json-defines-named-repositories
// spec: config/optional-repos-json-defines-named-repositories#missing-repos-json-is-single-repo-mode
// spec: config/optional-repos-json-defines-named-repositories#relative-repo-paths-resolve-against-the-file
#[test]
fn repos_json_is_optional_and_resolves_relative_paths() {
    let dir = std::env::temp_dir().join(format!("af-cli-repos-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config_dir = dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();

    // Missing file → single-repo mode (empty map).
    let map = config::load_repos(&config_dir.join("repos.json")).unwrap();
    assert!(map.is_empty(), "missing repos.json = no repositories");

    // Relative paths resolve against the repos.json file's directory.
    std::fs::write(
        config_dir.join("repos.json"),
        r#"{ "repos": { "docs": "../docs-site", "main": "." } }"#,
    )
    .unwrap();
    let map = config::load_repos(&config_dir.join("repos.json")).unwrap();
    assert_eq!(map["docs"], dir.join("docs-site"));
    assert_eq!(map["main"], dir.join("config"));

    let _ = std::fs::remove_dir_all(&dir);
}

// spec: scheduling/critical-path-priority
// spec: scheduling/critical-path-priority#deeper-task-first
#[test]
fn deeper_ready_task_dispatches_first() {
    use agentflow::scheduler::ready_tasks;

    // A is done from a previous run; B (deps A → depth 1) and C (no deps →
    // depth 0) are both ready — the deeper dependency depth goes first.
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "root", "accept": "true" },
        { "id": "B", "title": "follow on", "deps": ["A"], "accept": "true" },
        { "id": "C", "title": "independent leaf", "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (cfg, _) = cli.settings();
    let mut status = HashMap::new();
    status.insert(
        "A".to_string(),
        TaskStatus {
            state: agentflow::config::TaskState::Done,
            attempts: 1,
            ..Default::default()
        },
    );
    let order: Vec<String> = ready_tasks(&cfg, &status, &[], 3)
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(order.contains(&"B".to_string()) && order.contains(&"C".to_string()));
    assert_eq!(
        order.first().map(String::as_str),
        Some("B"),
        "deeper dependency depth dispatches first: {order:?}"
    );
}

// spec: scheduling/scope-contention-avoidance
// spec: scheduling/scope-contention-avoidance#overlapping-scope-deferred
#[test]
fn overlapping_scope_is_deferred_while_a_sibling_runs() {
    use agentflow::scheduler::ready_tasks;

    // X and Y touch the same file glob; while X runs, Y must be held back.
    let tasks = r#"{ "tasks": [
        { "id": "X", "title": "x", "scope": ["src/lib.rs"], "accept": "true" },
        { "id": "Y", "title": "y", "scope": ["src/lib.rs"], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (cfg, _) = cli.settings();
    // Nothing running: both are ready (one is dispatched, the other waits
    // for a free slot — the default `defer` behavior).
    let ready = ready_tasks(&cfg, &HashMap::new(), &[], 3);
    assert_eq!(ready.len(), 2, "both ready when idle");

    // X running: Y must NOT be dispatched until X finishes.
    let ready = ready_tasks(&cfg, &HashMap::new(), &["X".to_string()], 3);
    assert!(
        ready.iter().all(|t| t.id != "Y"),
        "overlapping scope deferred: {:?}",
        ready.iter().map(|t| t.id.clone()).collect::<Vec<_>>()
    );

    // Disjoint scope: Z may run while X runs (the end-to-end parallel arm
    // is proven by tests/e2e.rs `parallel_multi_worker_dispatch_…`).
    let tasks_disjoint = r#"{ "tasks": [
        { "id": "X", "title": "x", "scope": ["src/lib.rs"], "accept": "true" },
        { "id": "Z", "title": "z", "scope": ["docs/guide.md"], "accept": "true" }
    ] }"#;
    let cli2 = Cli::new_with_tasks(tasks_disjoint);
    let (cfg2, _) = cli2.settings();
    let ready = ready_tasks(&cfg2, &HashMap::new(), &["X".to_string()], 3);
    assert!(
        ready.iter().any(|t| t.id == "Z"),
        "disjoint scope stays dispatchable: {:?}",
        ready.iter().map(|t| t.id.clone()).collect::<Vec<_>>()
    );
}

// spec: scheduling/ucb1-worker-selection
// spec: scheduling/ucb1-worker-selection#fresh-state-picks-first-configured-worker
// spec: scheduling/ucb1-worker-selection#unexplored-worker-is-tried-before-a-failing-one
// spec: scheduling/ucb1-worker-selection#reliable-worker-wins-at-equal-counts
#[test]
fn ucb1_selection_matches_the_spec_scenarios() {
    use agentflow::router::Router;

    let w = |name: &str| agentflow::Worker {
        name: name.to_string(),
        ..Default::default()
    };

    // Fresh state (no receipts): the FIRST enabled worker in config order.
    let r = Router::default();
    let pool = [w("a"), w("b")];
    assert_eq!(r.pick(pool.iter()).unwrap().name, "a");

    // Unexplored worker is tried before a failing one (exploration term).
    let mut r = Router::default();
    for _ in 0..3 {
        r.record("a", false); // a: 0/3 wins
    }
    assert_eq!(r.pick(pool.iter()).unwrap().name, "b");

    // Reliable worker wins at equal counts (exploitation term).
    let mut r = Router::default();
    for _ in 0..3 {
        r.record("a", true); // a: 3/3
        r.record("b", false); // b: 0/3
    }
    assert_eq!(r.pick(pool.iter()).unwrap().name, "a");
}

// spec: config/task-schema-loading
// spec: config/task-schema-loading#string-priority-levels-accepted
#[test]
fn priority_levels_parse_and_rank_deterministically() {
    // String levels and plain numbers both parse onto the same typed field.
    let dir = std::env::temp_dir().join(format!("af-cli-prio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("tasks.json"),
        r#"{ "tasks": [
            { "id": "A", "title": "a", "priority": "HIGH", "accept": "true" },
            { "id": "B", "title": "b", "priority": 3, "accept": "true" },
            { "id": "C", "title": "c", "priority": "CRITICAL", "accept": "true" }
        ] }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("workers.json"),
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m", "enabled": true } ] }"#,
    )
    .unwrap();
    let cfg = config::load(&dir.join("tasks.json"), &dir.join("workers.json"))
        .expect("mixed priority levels must load");

    // And they rank deterministically: CRITICAL (20) > HIGH (10) > a
    // number-ranked task (3).
    let (a, b, c) = (
        cfg.by_id["A"].priority.rank(),
        cfg.by_id["B"].priority.rank(),
        cfg.by_id["C"].priority.rank(),
    );
    assert!(c > a, "CRITICAL outranks HIGH: {c} !> {a}");
    assert!(a > b, "HIGH outranks a number-ranked task: {a} !> {b}");

    let _ = std::fs::remove_dir_all(&dir);
}

// spec: config/per-task-repo-resolution-with-compat-fallback
// spec: config/per-task-repo-resolution-with-compat-fallback#empty-repo-means-the-default-repo
// spec: config/per-task-repo-resolution-with-compat-fallback#main-falls-back-to-the-default-repo
#[test]
fn repo_resolution_empty_and_main_fall_back_to_the_default_repo() {
    use agentflow::config::Task;

    // No repos.json: single-repo mode, so both "" and "main" resolve to
    // TF_REPO_DIR and neither warns ("main" is the canonical default name).
    let dir = std::env::temp_dir().join(format!("af-cli-repores-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("tasks.json"),
        r#"{ "tasks": [
            { "id": "A", "title": "a", "repo": "", "accept": "true" },
            { "id": "B", "title": "b", "repo": "main", "accept": "true" }
        ] }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("workers.json"),
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m", "enabled": true } ] }"#,
    )
    .unwrap();
    let cfg = config::load(&dir.join("tasks.json"), &dir.join("workers.json")).unwrap();
    let default = std::path::PathBuf::from("/default/repo");

    let mk = |repo: &str| Task {
        id: "t".into(),
        title: "t".into(),
        repo: repo.into(),
        ..Default::default()
    };
    assert_eq!(cfg.repo_dir_for(&mk(""), &default), default);
    assert_eq!(cfg.repo_dir_for(&mk("main"), &default), default);
    assert!(
        cfg.repo_warnings(&default).is_empty(),
        "empty/main must not warn: {:?}",
        cfg.repo_warnings(&default)
    );

    // A registered name still wins over the fallback (the resolution order
    // is name → default, never the reverse).
    let mut cfg_named = cfg.clone();
    cfg_named
        .repos
        .insert("main".into(), std::path::PathBuf::from("/repos/main"));
    assert_eq!(
        cfg_named.repo_dir_for(&mk("main"), &default),
        std::path::PathBuf::from("/repos/main")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// spec: scheduling/deadlock-detection
// spec: scheduling/deadlock-detection#all-tasks-blocked-by-failure
#[test]
fn run_deadlocks_when_remaining_tasks_are_blocked_by_failure() {
    // A real scratch git repo so task A's attempt can create a worktree; the
    // worker's agent CLI is a missing binary, so A's attempt always fails.
    // B depends on A → nothing can progress → the run must exit 2 naming B.
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "doomed", "accept": "true" },
        { "id": "B", "title": "waits on A", "deps": ["A"], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let repo = cli.dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let (code, out) = cli.af(&["run"]);
    assert_eq!(code, 2, "blocked-by-failure run exits 2: {out}");
    assert!(out.contains("DEADLOCK"), "deadlock is reported: {out}");
    assert!(
        out.contains("Blocked:"),
        "the blocked tasks are listed: {out}"
    );
    assert!(out.contains('B'), "B named as blocked: {out}");
}

// spec: scheduling/deadlock-detection
// spec: scheduling/deadlock-detection#absent-dependency-is-a-deadlock
#[test]
fn run_deadlocks_cleanly_on_absent_dependency() {
    // A's dep id exists in no loaded config (e.g. merged in from a sibling
    // file that is not present): the run must exit 2 instead of looping.
    let tasks =
        r#"{ "tasks": [ { "id": "A", "title": "a", "deps": ["GHOST"], "accept": "true" } ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let (code, out) = cli.af(&["run"]);
    assert_eq!(code, 2, "absent dependency must deadlock cleanly: {out}");
    assert!(out.contains("DEADLOCK"), "deadlock is reported: {out}");
    assert!(out.contains('A'), "A named as blocked: {out}");
}
