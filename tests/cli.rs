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
use agentflow::worktree::branch_exists;
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
        self.af_env(args, &[])
    }

    /// Run `af` like [`Cli::af`], layered with extra environment variables
    /// (used to exercise the `TF_NO_REUSE=1` escape hatch). The child still
    /// inherits the test process env for everything not overridden here.
    fn af_env(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_af"));
        cmd.args(args)
            .env("TF_TASKS_JSON", self.dir.join("config").join("tasks.json"))
            .env(
                "TF_WORKERS_JSON",
                self.dir.join("config").join("workers.json"),
            )
            .env("TF_STATE_DIR", self.dir.join("state"))
            .env("TF_REPO_DIR", self.dir.join("repo"))
            .env("TF_WORKTREE_ROOT", self.dir.join("wt"));
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn af");
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
            agent_stall_s: 0,
            max_wall_clock_s: 0,
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

    fn seed_running_task(&self, id: &str) {
        let (_, st) = self.settings();
        let mut map = HashMap::new();
        let s = TaskStatus {
            state: agentflow::config::TaskState::Running,
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
                cost_micros: None,
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
    "workers": [ { "name": "w1", "provider": "openai", "model": "gpt-4o", "params_b": 8, "enabled": true, "cli": "unused" } ] }"#;

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
    // The task filter narrows the TABLE rows (and with them the TOTAL):
    // A has no row under a filter naming another task — even though the
    // per-worker block still spans every selected receipt, so its MEAN_S
    // keeps reporting the worker's measured mean.
    assert!(
        !out.lines().any(|l| l.starts_with("A ")),
        "filter excludes other tasks' rows: {out}"
    );
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
            cost_micros: None,
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
        cost_micros: None,
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
// spec: cli/cost-report#wasted-spend-surfaces-failed-attempts
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
                cost_micros: None,
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

/// With NO provider-reported price, the COST column still ranks expense:
/// each worker's declared `params_b` becomes a RATE relative to the
/// cheapest declaring worker (exactly `1.00x`) — a PROXY, so the report
/// announces the basis above the tables and never prints a `$`. A receipt
/// without tokens cannot be priced and shows `-` instead.
// spec: cli/cost-report#cost-report-shows-relative-expense-when-no-provider-reports-a-price
#[test]
fn cost_report_shows_relative_expense_when_no_provider_reports_a_price() {
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "B", "title": "b", "accept": "true" },
        { "id": "C", "title": "c", "accept": "true" }
    ] }"#;
    // Two workers, different declared sizes: 8B and 40B — no price anywhere.
    let workers = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
        "workers": [
            { "name": "w1", "provider": "openai", "model": "gpt-4o", "params_b": 8, "enabled": true, "cli": "unused" },
            { "name": "w2", "provider": "openai", "model": "o1", "params_b": 40, "enabled": true, "cli": "unused" }
        ] }"#;
    let cli = Cli::new_with(tasks, workers);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt = |task: &str, ts: u64, wall: f64, worker: &str, tokens: Option<u64>| Receipt {
        task: task.into(),
        attempt: 1,
        worker: worker.into(),
        model: "m".into(),
        wall_clock_s: wall,
        tokens,
        ts,
        outcome: "merged".into(),
        error: None,
        cost_micros: None,
    };
    store
        .append_receipt(&receipt("A", 1_000, 1.0, "w1", Some(1_000_000)))
        .unwrap();
    store
        .append_receipt(&receipt("B", 2_000, 2.0, "w2", Some(500_000)))
        .unwrap();
    // A legacy receipt: tokens were never recorded, so its expense is
    // unknown — the placeholder, never an invented rate.
    store
        .append_receipt(&receipt("C", 3_000, 4.0, "w1", None))
        .unwrap();

    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "{out}");
    // ONE basis line above the tables announces the proxy before any
    // number is read.
    assert!(
        out.contains("cost basis: params_b proxy (relative; cheapest declared worker = 1.00x)"),
        "basis line: {out}"
    );
    // The cheapest declaring worker anchors the scale at exactly 1.00x…
    // (w1 has two verdict receipts — A at 1.0s and the legacy C at 4.0s,
    // both merged — so its MEAN_S is (1.0 + 4.0) / 2 = 2.5.)
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w1", "2/2", 1.00, "2.5", "1.00x"
        )),
        "w1 anchors the proxy at 1.00x: {out}"
    );
    // …and the 40B worker runs at 40/8 = 5x that rate (mean 2.0s).
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w2", "1/1", 1.00, "2.0", "5.00x"
        )),
        "w2 shows its real ratio: {out}"
    );
    // TASK rows carry the same estimate, between TOKENS and MODEL.
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "A", 1, 1.0, "1000000", "1.00x", "m"
        )),
        "A row: {out}"
    );
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "B", 1, 2.0, "500000", "5.00x", "m"
        )),
        "B row: {out}"
    );
    // The legacy receipt shows `-` in the COST column, not a zero rate.
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "C", 1, 4.0, "-", "-", "m"
        )),
        "C row shows the placeholder for absent tokens: {out}"
    );
    // A size proxy is not money: no `$` anywhere in the report.
    assert!(
        !out.contains('$'),
        "a proxy report never shows dollars: {out}"
    );
}

/// A DECLARED price is real money: the COST cell shows `$` + 4 decimals
/// computed from the row's tokens — even when the same worker ALSO
/// declares `params_b` (the price wins) — and never an `x` ratio.
// spec: cli/cost-report#a-declared-price-is-reported-as-dollars
#[test]
fn a_declared_price_beats_the_params_proxy_in_the_cost_report() {
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "B", "title": "b", "accept": "true" }
    ] }"#;
    // w1 declares BOTH a price and a size: the price must win. w2 keeps the
    // proxy scale, so the mixed report says so on the basis line.
    let workers = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
        "workers": [
            { "name": "w1", "provider": "openai", "model": "gpt-4o", "params_b": 100, "price_per_mtok_usd": 2.0, "enabled": true, "cli": "unused" },
            { "name": "w2", "provider": "openai", "model": "mini", "params_b": 8, "enabled": true, "cli": "unused" }
        ] }"#;
    let cli = Cli::new_with(tasks, workers);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt = |task: &str, ts: u64, wall: f64, worker: &str, tokens: u64| Receipt {
        task: task.into(),
        attempt: 1,
        worker: worker.into(),
        model: "m".into(),
        wall_clock_s: wall,
        tokens: Some(tokens),
        ts,
        outcome: "merged".into(),
        error: None,
        cost_micros: None,
    };
    // w1: 500k tokens at $2/Mtok = $1.0000.
    store
        .append_receipt(&receipt("A", 1_000, 1.0, "w1", 500_000))
        .unwrap();
    // w2: proxy worker, the only Sized declaration → 1.00x.
    store
        .append_receipt(&receipt("B", 2_000, 2.0, "w2", 250_000))
        .unwrap();

    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "{out}");
    // Prices put the report on the prices basis, and the proxy straggler
    // is named on the SAME line so mixed cells cannot be misread.
    assert!(
        out.contains(
            "cost basis: declared prices (USD per 1M tokens); some workers declare only params_b"
        ),
        "basis line: {out}"
    );
    // w1's row: dollars from its tokens, never a ratio.
    let w1_row = out
        .lines()
        .find(|l| l.starts_with("w1 "))
        .unwrap_or_else(|| panic!("w1 worker row: {out}"));
    assert!(
        w1_row.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w1", "1/1", 1.00, "1.0", "$1.0000"
        )),
        "w1 shows dollars computed from its tokens: {w1_row}"
    );
    assert!(
        !w1_row.contains('x'),
        "a priced worker never shows a ratio: {w1_row}"
    );
    // The TASK row sums the same dollars.
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "A", 1, 1.0, "500000", "$1.0000", "m"
        )),
        "A row: {out}"
    );
    // w2 keeps the proxy scale on the SAME report (mean 2.0s).
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w2", "1/1", 1.00, "2.0", "1.00x"
        )),
        "w2 keeps its proxy rate: {out}"
    );
}

/// A DECLARED basis must never render as `-`. The rate a worker declares is
/// a property of the WORKER, not of whether its receipts happened to record
/// tokens — and the default output mode records none. Without this, a
/// campaign whose workers run in text mode shows `-` in every COST cell,
/// which is precisely the question the column exists to answer ("is the big
/// model eating the budget?"). A declared price with no tokens shows the
/// declared RATE, unit-suffixed so it can never be misread as a spend.
// spec: cli/cost-report#a-declared-basis-is-never-blank
#[test]
fn cost_report_shows_a_declared_rate_even_when_no_tokens_were_recorded() {
    let tasks = r#"{ "tasks": [ { "id": "A", "title": "a", "accept": "true" } ] }"#;
    let workers = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
        "workers": [
            { "name": "small", "provider": "openai", "model": "flash", "params_b": 8, "enabled": true, "cli": "unused" },
            { "name": "big", "provider": "openai", "model": "glm", "params_b": 400, "enabled": true, "cli": "unused" },
            { "name": "priced", "provider": "openai", "model": "cheap", "price_per_mtok_usd": 0.6, "enabled": true, "cli": "unused" },
            { "name": "nobasis", "provider": "openai", "model": "m", "enabled": true, "cli": "unused" }
        ] }"#;
    let cli = Cli::new_with(tasks, workers);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    // EVERY attempt is a legacy/text-mode receipt: no tokens anywhere, which
    // is exactly the case that used to blank the whole column.
    for (worker, ts) in [
        ("small", 1_000u64),
        ("big", 2_000),
        ("priced", 3_000),
        ("nobasis", 4_000),
    ] {
        store
            .append_receipt(&Receipt {
                task: "A".into(),
                attempt: 1,
                worker: worker.into(),
                model: "m".into(),
                wall_clock_s: 1.0,
                tokens: None,
                ts,
                outcome: "merged".into(),
                error: None,
                cost_micros: None,
            })
            .unwrap();
    }

    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "{out}");
    // 400B against the cheapest declared 8B basis is exactly 50x — the
    // assumption the operator declared, visible with no token data at all
    // (every attempt here took 1.0s, so MEAN_S is 1.0).
    for (worker, cell) in [("big", "50.00x"), ("small", "1.00x")] {
        assert!(
            out.contains(&format!(
                "{:<14} {:<11} {:.2} {} {}",
                worker, "1/1", 1.00, "1.0", cell
            )),
            "{worker} must show its declared rate {cell}: {out}"
        );
    }
    // A declared price with no tokens shows the declared RATE (with its unit),
    // never a `-` and never a bare `$` that would read as a spend figure.
    assert!(
        out.contains("$0.6000/Mtok"),
        "a priced worker with no tokens shows its declared rate: {out}"
    );
    // The invariant is "declared means rendered", never "always invented":
    // a worker declaring NOTHING keeps the placeholder.
    let nobasis = out
        .lines()
        .find(|l| l.starts_with("nobasis"))
        .unwrap_or_else(|| panic!("nobasis worker row: {out}"));
    assert!(
        nobasis.trim_end().ends_with('-'),
        "a worker declaring no basis shows the placeholder: {nobasis}"
    );
}

/// Historical receipts outlive config edits: a receipt naming a worker that
/// is no longer in workers.json renders as `-` and gets ONE footnote line
/// naming those workers (sorted, deduped, with the attempt count) — a note,
/// never an error, never blocking the report.
// spec: cli/cost-report#receipts-naming-a-worker-absent-from-the-config-are-footnoted
#[test]
fn receipts_naming_an_absent_worker_are_footnoted_not_fatal() {
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "B", "title": "b", "accept": "true" },
        { "id": "C", "title": "c", "accept": "true" }
    ] }"#;
    // Only w1 is configured; `ghost` (twice) and `foo` are history.
    let workers = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
        "workers": [
            { "name": "w1", "provider": "openai", "model": "gpt-4o", "price_per_mtok_usd": 1.0, "enabled": true, "cli": "unused" }
        ] }"#;
    let cli = Cli::new_with(tasks, workers);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt =
        |task: &str, attempt: u32, ts: u64, wall: f64, worker: &str, tokens: u64| Receipt {
            task: task.into(),
            attempt,
            worker: worker.into(),
            model: "m".into(),
            wall_clock_s: wall,
            tokens: Some(tokens),
            ts,
            outcome: "merged".into(),
            error: None,
            cost_micros: None,
        };
    store
        .append_receipt(&receipt("A", 1, 1_000, 1.0, "ghost", 1_000_000))
        .unwrap();
    store
        .append_receipt(&receipt("A", 2, 2_000, 2.0, "ghost", 1_000_000))
        .unwrap();
    store
        .append_receipt(&receipt("B", 1, 3_000, 3.0, "foo", 10))
        .unwrap();
    // The configured worker keeps its priced row, proving the footnote
    // changes nothing about the rest of the report.
    store
        .append_receipt(&receipt("C", 1, 4_000, 4.0, "w1", 2_000_000))
        .unwrap();

    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "absent workers are a note, not an error: {out}");
    // ONE footnote after the tables: total attempt count, names sorted and
    // deduped (ghost appears twice, listed once, after foo).
    assert!(
        out.contains(
            "note: 3 attempt(s) name a worker absent from the config, shown as '-': foo, ghost"
        ),
        "footnote: {out}"
    );
    // The unknown workers' rows show `-`, both tables (ghost's two
    // attempts took 1.0s and 2.0s → MEAN_S 1.5).
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "ghost", "2/2", 1.00, "1.5", "-"
        )),
        "ghost worker row: {out}"
    );
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "A", 2, 3.0, "2000000", "-", "m"
        )),
        "A's attempts name an absent worker, so COST is `-`: {out}"
    );
    // The configured worker's priced row is untouched (mean 4.0s).
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w1", "1/1", 1.00, "4.0", "$2.0000"
        )),
        "w1 keeps its dollars: {out}"
    );
}

/// The cost the provider itself REPORTS is real money and outranks any
/// DECLARED basis for the attempt it belongs to: a measured receipt on a
/// worker that also declares a price shows the measured dollars (no `~` —
/// nothing about it is assumed), a row mixing a measured attempt with an
/// estimated-priced one sums the dollars but marks them `~` (an assumption
/// must never be presented as a measurement), a measured+sized row stays
/// `-` (dollars and a parameter ratio are incommensurable), and the basis
/// line names the measured source.
// spec: cli/cost-report#the-cost-report-prefers-a-measured-cost-over-a-declared-one
#[test]
fn the_cost_report_prefers_a_measured_cost_over_a_declared_one() {
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "B", "title": "b", "accept": "true" },
        { "id": "C", "title": "c", "accept": "true" }
    ] }"#;
    // w1 ALSO declares a price — its 500k-token receipts would estimate to
    // $1.0000 each; the provider reported $0.0123 instead. w2 is sized.
    let workers = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
        "workers": [
            { "name": "w1", "provider": "openai", "model": "gpt-4o", "price_per_mtok_usd": 2.0, "enabled": true, "cli": "unused" },
            { "name": "w2", "provider": "openai", "model": "o1", "params_b": 8, "enabled": true, "cli": "unused" }
        ] }"#;
    let cli = Cli::new_with(tasks, workers);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt = |task: &str,
                   attempt: u32,
                   ts: u64,
                   wall: f64,
                   worker: &str,
                   tokens: u64,
                   cost: Option<u64>| Receipt {
        task: task.into(),
        attempt,
        worker: worker.into(),
        model: "m".into(),
        wall_clock_s: wall,
        tokens: Some(tokens),
        ts,
        outcome: "merged".into(),
        error: None,
        cost_micros: cost,
    };
    // A: one MEASURED attempt on the priced worker — the measurement wins.
    store
        .append_receipt(&receipt("A", 1, 1_000, 1.0, "w1", 500_000, Some(12_300)))
        .unwrap();
    // B: a measured attempt PLUS an estimated-priced attempt.
    store
        .append_receipt(&receipt("B", 1, 2_000, 1.0, "w1", 500_000, Some(12_300)))
        .unwrap();
    store
        .append_receipt(&receipt("B", 2, 3_000, 1.0, "w1", 500_000, None))
        .unwrap();
    // C: a measured attempt MIXED with a sized (ratio-only) attempt.
    store
        .append_receipt(&receipt("C", 1, 4_000, 1.0, "w1", 100, Some(12_300)))
        .unwrap();
    store
        .append_receipt(&receipt("C", 2, 5_000, 1.0, "w2", 1_000_000, None))
        .unwrap();

    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "{out}");
    // The basis line names the measured source and counts the attempts
    // still riding a declared-price estimate (B's second attempt).
    assert!(
        out.contains("cost basis: provider-reported (USD)"),
        "basis line: {out}"
    );
    assert!(
        out.contains("1 of 5 attempt(s) estimated from a declared price"),
        "the estimate count explains the report's `~` markers: {out}"
    );
    // A: measured dollars, NO `~` — not the $1.0000 its price would have
    // estimated, and not an estimate at all.
    let a_row = out
        .lines()
        .find(|l| l.starts_with("A "))
        .unwrap_or_else(|| panic!("A row: {out}"));
    assert!(
        a_row.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "A", 1, 1.0, "500000", "$0.0123", "m"
        )),
        "the measured cost outranks the declared price: {a_row}"
    );
    assert!(
        !a_row.contains('~'),
        "a measurement carries no `~`: {a_row}"
    );
    // B: measured + estimated → summed dollars WITH `~`.
    let b_row = out
        .lines()
        .find(|l| l.starts_with("B "))
        .unwrap_or_else(|| panic!("B row: {out}"));
    assert!(
        b_row.contains("~$1.0123"),
        "summed dollars, marked estimated: {b_row}"
    );
    // C: measured + sized → incommensurable → `-`, never a converted figure.
    let c_row = out
        .lines()
        .find(|l| l.starts_with("C "))
        .unwrap_or_else(|| panic!("C row: {out}"));
    assert!(
        c_row.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "C", 2, 2.0, "1000100", "-", "m"
        )),
        "dollars and a parameter ratio never mix: {c_row}"
    );
    // The worker rows follow the same ladder: w1's whole selected spend is
    // 3 measured × $0.0123 + 1 estimate × $1.0000, marked `~`; w2 keeps its
    // proxy rate (no measured cost of its own). Both workers' four/one
    // verdict attempts each took 1.0s → MEAN_S 1.0.
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w1", "4/4", 1.00, "1.0", "~$1.0369"
        )),
        "w1 folds measured-first like the task rows: {out}"
    );
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w2", "1/1", 1.00, "1.0", "1.00x"
        )),
        "w2 keeps its proxy rate: {out}"
    );
}

/// The WORKER table's MEAN_S column shows the exact duration statistic the
/// router reads: the MEAN wall-clock seconds over the worker's VERDICT
/// attempts — interrupted attempts are excluded on both sides (their
/// duration is a placeholder, not a measurement) — at one decimal, so the
/// routing tie-break's input is inspectable from the same report.
// spec: cli/cost-report#the-cost-report-shows-the-duration-the-router-reads
#[test]
fn the_cost_report_shows_the_duration_the_router_reads() {
    let tasks = r#"{ "tasks": [
        { "id": "A", "title": "a", "accept": "true" },
        { "id": "B", "title": "b", "accept": "true" }
    ] }"#;
    // Neither worker declares a basis, so COST is `-` and MEAN_S is the
    // only measured figure in the worker rows.
    let workers = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
        "workers": [
            { "name": "w1", "provider": "openai", "model": "gpt-4o", "enabled": true, "cli": "unused" },
            { "name": "w2", "provider": "openai", "model": "o1", "enabled": true, "cli": "unused" }
        ] }"#;
    let cli = Cli::new_with(tasks, workers);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    let receipt = |task: &str, ts: u64, wall: f64, worker: &str, outcome: &str| Receipt {
        task: task.into(),
        attempt: 1,
        worker: worker.into(),
        model: "m".into(),
        wall_clock_s: wall,
        tokens: None,
        ts,
        outcome: outcome.into(),
        error: None,
        cost_micros: None,
    };
    // w1: two verdict attempts (2.0s + 6.0s) → MEAN_S 4.0. w2: one (3.0s).
    store
        .append_receipt(&receipt("A", 1_000, 2.0, "w1", "merged"))
        .unwrap();
    store
        .append_receipt(&receipt("A", 2_000, 6.0, "w1", "failed"))
        .unwrap();
    store
        .append_receipt(&receipt("B", 3_000, 3.0, "w2", "merged"))
        .unwrap();
    // An interrupted attempt on w1 with a huge placeholder duration: it
    // must enter NEITHER the mean NOR the wins/total.
    store
        .append_receipt(&receipt("A", 4_000, 999.0, "w1", "interrupted"))
        .unwrap();

    let (code, out) = cli.af(&["cost"]);
    assert_eq!(code, 0, "{out}");
    // The header advertises the column between TRUST and COST.
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {} {} {}",
            "WORKER", "WINS/TOTAL", "TRUST", "MEAN_S", "COST"
        )),
        "worker table header: {out}"
    );
    // The cells: the mean over verdict attempts only, ONE decimal. If the
    // interrupted placeholder entered, w1 would read 335.7 (and 1/3).
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w1", "1/2", 0.50, "4.0", "-"
        )),
        "w1's MEAN_S is (2.0 + 6.0) / 2, interrupted excluded: {out}"
    );
    assert!(
        out.contains(&format!(
            "{:<14} {:<11} {:.2} {} {}",
            "w2", "1/1", 1.00, "3.0", "-"
        )),
        "w2's MEAN_S is its single verdict attempt: {out}"
    );
    assert!(
        !out.contains("335"),
        "the interrupted placeholder duration never reaches MEAN_S: {out}"
    );
    // The interrupted attempt is still accounted for distinctly.
    assert!(
        out.contains("INTERRUPTED: 999.0s on 1 attempt(s)"),
        "the lost attempt keeps its own line: {out}"
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

/// The user-visible half of the archived-ref sweep: `--dry-run` prints
/// `would remove branch <name>` and touches nothing; the real run prints
/// `removed branch <name>` and the ref is gone; a ref whose task is still
/// Running is kept (and never reported as removed), exactly like the
/// running-worktree rule. A foreign-prefixed ref is not ours to sweep.
// spec: worktree/af-clean-sweeps-archived-rejected-branches
// spec: worktree/af-clean-sweeps-archived-rejected-branches#sweeps-archived-branches-but-keeps-a-running-task-s
#[test]
fn clean_sweeps_archived_rejected_branches_and_keeps_running_ones() {
    let tasks = r#"{ "tasks": [
        { "id": "dead", "title": "d", "accept": "true" },
        { "id": "live", "title": "l", "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);

    // A real scratch repo: `af clean` sweeps refs in TF_REPO_DIR.
    let repo = cli.dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&repo)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["commit", "--allow-empty", "-m", "init"]);
    for b in [
        "tf/dead-rejected-111",
        "tf/live-rejected-222",
        "other/x-rejected-1",
    ] {
        run_git(&["branch", b]);
    }
    // `live` is still Running: its archived ref must survive the sweep.
    cli.seed_running_task("live");

    // Dry run reports every ref that would go, and removes none of them.
    let (code, out) = cli.af(&["clean", "--dry-run"]);
    assert_eq!(code, 0, "dry run exits 0: {out}");
    assert!(
        out.contains("would remove branch tf/dead-rejected-111"),
        "dry run names the dead ref: {out}"
    );
    assert!(
        !out.contains("live-rejected-222"),
        "a running task's ref would not be removed, so dry run must not name it: {out}"
    );
    assert!(
        !out.contains("other/x-rejected-1"),
        "a foreign prefix is not ours: {out}"
    );
    assert!(
        branch_exists(&repo, "tf/dead-rejected-111")
            && branch_exists(&repo, "tf/live-rejected-222"),
        "dry run touches nothing: {out}"
    );

    // Real clean removes the dead task's ref and keeps the running one.
    let (code, out) = cli.af(&["clean"]);
    assert_eq!(code, 0, "clean exits 0: {out}");
    assert!(
        out.contains("removed branch tf/dead-rejected-111"),
        "clean reports the removed ref: {out}"
    );
    assert!(
        !out.contains("removed branch tf/live-rejected-222"),
        "a running task's ref is never reported as removed: {out}"
    );
    assert!(
        !branch_exists(&repo, "tf/dead-rejected-111"),
        "the dead task's archived ref is gone"
    );
    assert!(
        branch_exists(&repo, "tf/live-rejected-222"),
        "the running task's archived ref survives"
    );
    assert!(
        branch_exists(&repo, "other/x-rejected-1"),
        "the foreign-prefixed ref survives"
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
// `af recover` — re-validate an archived rejected branch and merge it
// without re-running the agent (r12-recover-rejected).
// --------------------------------------------------------------------------

/// Create a scratch git repo at `repo` and return a closure that runs git
/// verbatim in it (failing hard on any git error).
fn scratch_repo(repo: &std::path::Path) -> impl Fn(&[&str]) {
    std::fs::create_dir_all(repo).unwrap();
    let repo = repo.to_path_buf();
    let run_git = move |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&repo)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["commit", "--allow-empty", "-m", "init"]);
    run_git
}

/// An archived rejected branch carrying a committed change that is in scope
/// and passes the gate; `scope` and `accept` are the task's current config.
fn fixture_recover_task(cli: &Cli, id: &str) {
    let repo = cli.dir.join("repo");
    let git = scratch_repo(&repo);
    git(&["checkout", "-b", &format!("tf/{id}")]);
    std::fs::write(repo.join("WORK.txt"), "done\n").unwrap();
    git(&["add", "WORK.txt"]);
    git(&["commit", "-m", "agent work"]);
    git(&["branch", &format!("tf/{id}-rejected-1791259017")]);
    git(&["checkout", "main"]);
}

// spec: cli/recover-command
// spec: cli/recover-command#recover-merges-an-archived-branch-after-revalidating-scope-and-gate
// spec: worktree/recovered-branches-are-re-validated-then-merged#recovery-checks-out-the-archived-branch-on-its-own-tip
// spec: worktree/recovered-branches-are-re-validated-then-merged#an-archived-branch-is-consumed-by-a-successful-recovery
#[test]
fn recover_merges_an_archived_branch_after_revalidating_scope_and_gate() {
    let tasks = r#"{ "tasks": [
        { "id": "C", "title": "recover me", "scope": ["WORK.txt"], "accept": "test -f WORK.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    fixture_recover_task(&cli, "C");
    let repo = cli.dir.join("repo");

    let (code, out) = cli.af(&["recover", "--task", "C"]);
    assert_eq!(code, 0, "recover must exit 0: {out}");
    assert!(
        out.contains("C recovered to done (agent not re-run)"),
        "success message: {out}"
    );
    // The task is Done in the state file.
    let (_, st) = cli.settings();
    let status = Store::new(st.state_dir.clone()).load();
    assert_eq!(
        status["C"].state,
        agentflow::config::TaskState::Done,
        "task marked done: {out}"
    );
    // The change is in the base branch.
    assert!(repo.join("WORK.txt").exists(), "change merged into base");
    // The archived branch is gone (its work now lives in the base).
    assert!(
        !branch_exists(&repo, "tf/C-rejected-1791259017"),
        "archived branch removed after recovery"
    );
}

// spec: cli/recover-command#out-of-scope-branch-fails-and-survives
#[test]
fn recover_out_of_scope_branch_fails_and_survives() {
    let tasks = r#"{ "tasks": [
        { "id": "O", "title": "t", "scope": ["OK.txt"], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let repo = cli.dir.join("repo");
    let git = scratch_repo(&repo);
    git(&["checkout", "-b", "tf/O"]);
    std::fs::write(repo.join("OUT.txt"), "x\n").unwrap();
    git(&["add", "OUT.txt"]);
    git(&["commit", "-m", "out of scope"]);
    git(&["branch", "tf/O-rejected-111"]);
    git(&["checkout", "main"]);

    let (code, out) = cli.af(&["recover", "--task", "O"]);
    assert_eq!(code, 1, "out-of-scope recover exits 1: {out}");
    assert!(out.contains("OUT.txt"), "names the offending file: {out}");
    assert!(
        branch_exists(&repo, "tf/O-rejected-111"),
        "archived branch survives a failed recover"
    );
    assert!(
        !repo.join("OUT.txt").exists(),
        "out-of-scope change not merged"
    );
}

// spec: cli/recover-command#gate-failing-branch-fails-and-survives
#[test]
fn recover_gate_failing_branch_fails_and_survives() {
    let tasks = r#"{ "tasks": [
        { "id": "G", "title": "t", "scope": [], "accept": "test -f MISSING.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let repo = cli.dir.join("repo");
    let git = scratch_repo(&repo);
    git(&["checkout", "-b", "tf/G"]);
    std::fs::write(repo.join("IN.txt"), "x\n").unwrap();
    git(&["add", "IN.txt"]);
    git(&["commit", "-m", "in scope but gate fails"]);
    git(&["branch", "tf/G-rejected-222"]);
    git(&["checkout", "main"]);

    let (code, out) = cli.af(&["recover", "--task", "G"]);
    assert_eq!(code, 1, "gate-failing recover exits 1: {out}");
    assert!(out.contains("acceptance gate"), "names the gate: {out}");
    assert!(
        branch_exists(&repo, "tf/G-rejected-222"),
        "branch survives a gate-failing recover"
    );
    assert!(!repo.join("IN.txt").exists(), "not merged");
}

// spec: cli/recover-command#dry-run-reports-without-changing-anything
#[test]
fn recover_dry_run_reports_without_changing_anything() {
    let tasks = r#"{ "tasks": [
        { "id": "D", "title": "t", "scope": ["WORK.txt"], "accept": "test -f WORK.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    fixture_recover_task(&cli, "D");
    let repo = cli.dir.join("repo");

    let (code, out) = cli.af(&["recover", "--task", "D", "--dry-run"]);
    assert_eq!(code, 0, "dry run exits 0: {out}");
    assert!(
        out.contains("tf/D-rejected-1791259017"),
        "names the branch: {out}"
    );
    // Nothing changed: branch survives, change not merged, task not Done.
    assert!(branch_exists(&repo, "tf/D-rejected-1791259017"));
    assert!(!repo.join("WORK.txt").exists(), "no merge on dry run");
    let (_, st) = cli.settings();
    let status = Store::new(st.state_dir.clone()).load();
    assert_ne!(
        status.get("D").map(|s| &s.state),
        Some(&agentflow::config::TaskState::Done),
        "dry run must not mark the task done"
    );
}

// spec: cli/recover-command#unknown-task-exits-two
#[test]
fn recover_unknown_task_exits_two() {
    let cli = Cli::new_with_tasks(TASKS);
    let (code, out) = cli.af(&["recover", "--task", "NOPE"]);
    assert_eq!(code, 2, "unknown task exits 2: {out}");
    assert!(out.contains("unknown task 'NOPE'"), "names the task: {out}");
}

// spec: cli/recover-command#no-archived-branch-exits-two
#[test]
fn recover_with_no_archived_branch_exits_two() {
    let tasks = r#"{ "tasks": [ { "id": "E", "title": "t", "accept": "true" } ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    let repo = cli.dir.join("repo");
    let _git = scratch_repo(&repo);
    let (code, out) = cli.af(&["recover", "--task", "E"]);
    assert_eq!(code, 2, "no archived branch exits 2: {out}");
    assert!(out.contains("E"), "names the task: {out}");
    assert!(
        out.contains("rejected"),
        "names the pattern searched: {out}"
    );
}

/// An archived rejected branch in the CURRENT (r14) name format
/// `<prefix>/<id>-rejected-<attempt>-<ts>`, so recovery can pair the marker
/// with the failed attempt. `fixture_recover_task` covers the legacy,
/// attempt-less form.
fn fixture_recover_task_at_attempt(cli: &Cli, id: &str, attempt: u32, ts: u64) {
    let repo = cli.dir.join("repo");
    let git = scratch_repo(&repo);
    git(&["checkout", "-b", &format!("tf/{id}")]);
    std::fs::write(repo.join("WORK.txt"), "done\n").unwrap();
    git(&["add", "WORK.txt"]);
    git(&["commit", "-m", "agent work"]);
    git(&["branch", &format!("tf/{id}-rejected-{attempt}-{ts}")]);
    git(&["checkout", "main"]);
}

/// Several archived rejected branches for one task, one per
/// `(attempt, ts, content)` triple. Each archive carries distinct `WORK.txt`
/// content so a recovery names the winning attempt, letting `--attempt N`
/// selection be told apart from the default newest-archive pick.
fn fixture_recover_task_attempts(cli: &Cli, id: &str, archives: &[(u32, u64, &str)]) {
    let repo = cli.dir.join("repo");
    let git = scratch_repo(&repo);
    for (attempt, ts, content) in archives {
        git(&["checkout", "-B", &format!("tf/{id}"), "main"]);
        std::fs::write(repo.join("WORK.txt"), content).unwrap();
        git(&["add", "WORK.txt"]);
        git(&["commit", "-m", "agent work"]);
        git(&["branch", &format!("tf/{id}-rejected-{attempt}-{ts}")]);
        git(&["checkout", "main"]);
    }
    // Round 11's cleanup deletes the original attempt branch after archiving.
    git(&["branch", "-D", &format!("tf/{id}")]);
}

// spec: cli/recover-command#attempt-flag-selects-the-archived-attempt
#[test]
fn recover_with_attempt_flag_selects_the_right_archive() {
    let tasks = r#"{ "tasks": [
        { "id": "C", "title": "recover me", "scope": ["WORK.txt"], "accept": "test -f WORK.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    fixture_recover_task_attempts(
        &cli,
        "C",
        &[
            (1, 1791259001, "attempt-one\n"),
            (2, 1791259002, "attempt-two\n"),
        ],
    );
    let repo = cli.dir.join("repo");

    let (code, out) = cli.af(&["recover", "--task", "C", "--attempt", "1"]);
    assert_eq!(code, 0, "recover --attempt 1 exits 0: {out}");
    assert_eq!(
        std::fs::read_to_string(repo.join("WORK.txt")).unwrap(),
        "attempt-one\n",
        "the requested attempt's work is merged, not the newest: {out}"
    );
    assert!(
        !branch_exists(&repo, "tf/C-rejected-1-1791259001"),
        "the recovered attempt's archive is consumed"
    );
    assert!(
        branch_exists(&repo, "tf/C-rejected-2-1791259002"),
        "the other attempt's archive survives"
    );
}

// spec: cli/recover-command#no-attempt-flag-picks-the-newest-archive
#[test]
fn recover_attempt_flag_picks_newest_without_flag() {
    let tasks = r#"{ "tasks": [
        { "id": "C", "title": "recover me", "scope": ["WORK.txt"], "accept": "test -f WORK.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    fixture_recover_task_attempts(
        &cli,
        "C",
        &[
            (1, 1791259001, "attempt-one\n"),
            (2, 1791259002, "attempt-two\n"),
        ],
    );
    let repo = cli.dir.join("repo");

    // No --attempt: the newest archive by parsed <ts> wins, whatever attempt
    // it names.
    let (code, out) = cli.af(&["recover", "--task", "C"]);
    assert_eq!(code, 0, "recover without --attempt exits 0: {out}");
    assert_eq!(
        std::fs::read_to_string(repo.join("WORK.txt")).unwrap(),
        "attempt-two\n",
        "the newest archive wins without --attempt: {out}"
    );
    assert!(
        !branch_exists(&repo, "tf/C-rejected-2-1791259002"),
        "the newest archive is consumed"
    );
    assert!(
        branch_exists(&repo, "tf/C-rejected-1-1791259001"),
        "the older attempt's archive survives"
    );
}

// spec: cli/recover-command#attempt-flag-rejects-an-unknown-attempt
#[test]
fn recover_attempt_flag_rejects_unknown_attempt() {
    let tasks = r#"{ "tasks": [
        { "id": "C", "title": "recover me", "scope": ["WORK.txt"], "accept": "test -f WORK.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    fixture_recover_task_attempts(&cli, "C", &[(1, 1791259001, "attempt-one\n")]);
    let repo = cli.dir.join("repo");

    let (code, out) = cli.af(&["recover", "--task", "C", "--attempt", "9"]);
    assert_eq!(code, 2, "an unknown attempt exits 2: {out}");
    assert!(
        out.contains("attempt 9"),
        "names the attempt searched: {out}"
    );
    assert!(
        branch_exists(&repo, "tf/C-rejected-1-1791259001"),
        "the existing archive survives"
    );
    assert!(!repo.join("WORK.txt").exists(), "nothing merged");
}

/// A successful `af recover` is a recovery, not a re-run: it appends a
/// NON-VERDICT `recovered` receipt naming the archived attempt and the failed
/// attempt's worker/model, measures 0.0s (no agent ran), and leaves the
/// failed receipt untouched — history stays append-only, and `af cost` can
/// pair the two.
// spec: state/recovered-receipts
// spec: state/recovered-receipts#a-recovered-receipt-pairs-with-the-failed-attempt
#[test]
fn recover_writes_a_recovered_receipt() {
    let tasks = r#"{ "tasks": [
        { "id": "C", "title": "recover me", "scope": ["WORK.txt"], "accept": "test -f WORK.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    fixture_recover_task_at_attempt(&cli, "C", 2, 1791259017);
    let (_, st) = cli.settings();
    let store = Store::new(st.state_dir.clone());
    // The failed attempt that produced the archive, on disk before recovery.
    store
        .append_receipt(&Receipt {
            task: "C".into(),
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

    let (code, out) = cli.af(&["recover", "--task", "C"]);
    assert_eq!(code, 0, "recover exits 0: {out}");

    let receipts = store.load_receipts();
    assert_eq!(
        receipts.len(),
        2,
        "the failed receipt is kept and one recovered marker added: {receipts:?}"
    );
    assert!(
        receipts
            .iter()
            .any(|r| r.outcome == "failed" && r.attempt == 2 && r.wall_clock_s == 100.0),
        "history is append-only — the failed receipt is untouched: {receipts:?}"
    );
    let recovered = receipts
        .iter()
        .find(|r| r.outcome == "recovered")
        .unwrap_or_else(|| panic!("a recovered receipt exists: {receipts:?}"));
    assert_eq!(recovered.task, "C");
    assert_eq!(recovered.attempt, 2, "paired to the archived attempt");
    assert_eq!(recovered.worker, "w1", "names the failed attempt's worker");
    assert_eq!(recovered.model, "gpt-4o");
    assert_eq!(recovered.wall_clock_s, 0.0, "no agent ran");
    assert!(
        !recovered.counts_as_verdict(),
        "recovery is not a verdict on the worker"
    );
}

// ---------------------------------------------------------------------------
// Pre-dispatch archived-branch reuse — `af run` re-validates an archive
// instead of paying an agent again (r13-auto-recover).
// ---------------------------------------------------------------------------

/// Seed exactly the on-disk state a rejected attempt leaves behind: an
/// archived rejected branch `tf/<id>-rejected-<ts>` carrying a committed
/// `file` = `content`, `main` checked out, and NO original attempt branch
/// (round 11's `cleanup` deletes `tf/<id>` after archiving).
fn seed_rejected_branch(cli: &Cli, id: &str, ts: u64, file: &str, content: &str) {
    let repo = cli.dir.join("repo");
    let git = scratch_repo(&repo);
    git(&["checkout", "-b", &format!("tf/{id}")]);
    std::fs::write(repo.join(file), content).unwrap();
    git(&["add", file]);
    git(&["commit", "-m", "agent work"]);
    git(&["branch", &format!("tf/{id}-rejected-{ts}")]);
    git(&["checkout", "main"]);
    git(&["branch", "-D", &format!("tf/{id}")]);
}

/// The `-- agent --` marker in a task's attempt log counts agent spawns
/// (`execute_attempt` appends it immediately before every spawn); a missing
/// log is zero.
fn agent_spawns(cli: &Cli, id: &str) -> usize {
    std::fs::read_to_string(cli.dir.join("state").join("logs").join(format!("{id}.log")))
        .unwrap_or_default()
        .matches("-- agent --")
        .count()
}

// spec: lifecycle/pre-dispatch-reuse-of-an-archived-branch#an-out-of-scope-archive-falls-through-to-the-agent
#[test]
fn run_falls_through_an_out_of_scope_archived_branch() {
    let tasks = r#"{ "tasks": [
        { "id": "O", "title": "t", "scope": ["OK.txt"], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    seed_rejected_branch(&cli, "O", 111, "OUT.txt", "x\n");
    let repo = cli.dir.join("repo");

    let (code, out) = cli.af(&["run"]);
    // The archive is out of scope, so the run must NOT merge it. It falls
    // through to a normal agent dispatch; the harness worker is a missing
    // binary, the attempt fails, and max_attempts=1 deadlocks the campaign.
    assert_ne!(code, 0, "out-of-scope archive must not merge: {out}");
    assert!(
        out.contains("out of scope") && out.contains("tf/O-rejected-111"),
        "names the rejected candidate: {out}"
    );
    assert!(
        branch_exists(&repo, "tf/O-rejected-111"),
        "the rejected archive survives for a later recover/clean"
    );
    assert!(
        !repo.join("OUT.txt").exists(),
        "out-of-scope change not merged"
    );
    assert_eq!(agent_spawns(&cli, "O"), 1, "the agent WAS dispatched");
}

// spec: lifecycle/pre-dispatch-reuse-of-an-archived-branch#a-gate-failing-archive-falls-through-to-the-agent
#[test]
fn run_falls_through_a_gate_failing_archived_branch() {
    let tasks = r#"{ "tasks": [
        { "id": "G", "title": "t", "scope": [], "accept": "test -f MISSING.txt" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    seed_rejected_branch(&cli, "G", 222, "IN.txt", "x\n");
    let repo = cli.dir.join("repo");

    let (code, out) = cli.af(&["run"]);
    assert_ne!(code, 0, "gate-failing archive must not merge: {out}");
    assert!(
        out.contains("gate failed") && out.contains("tf/G-rejected-222"),
        "names the rejected candidate: {out}"
    );
    assert!(
        branch_exists(&repo, "tf/G-rejected-222"),
        "the rejected archive survives"
    );
    assert!(
        !repo.join("IN.txt").exists(),
        "gate-failing change not merged"
    );
    assert_eq!(agent_spawns(&cli, "G"), 1, "the agent WAS dispatched");
}

// spec: lifecycle/pre-dispatch-reuse-of-an-archived-branch#tf-no-reuse-opts-out
#[test]
fn run_with_tf_no_reuse_never_reuses_an_archived_branch() {
    let tasks = r#"{ "tasks": [
        { "id": "N", "title": "t", "scope": ["WORK.txt"], "accept": "true" }
    ] }"#;
    let cli = Cli::new_with_tasks(tasks);
    seed_rejected_branch(&cli, "N", 333, "WORK.txt", "done\n");
    let repo = cli.dir.join("repo");

    let (code, out) = cli.af_env(&["run"], &[("TF_NO_REUSE", "1")]);
    // The archive is perfectly reusable (in scope, gate would pass), but the
    // escape hatch forces a fresh agent — which fails here.
    assert_ne!(code, 0, "opt-out buys a fresh agent: {out}");
    assert!(
        !out.contains("reused archived branch"),
        "TF_NO_REUSE=1 must never reuse: {out}"
    );
    assert!(
        branch_exists(&repo, "tf/N-rejected-333"),
        "the archive is left untouched"
    );
    assert!(!repo.join("WORK.txt").exists(), "nothing merged");
    assert_eq!(agent_spawns(&cli, "N"), 1, "the agent WAS dispatched");
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
                cost_micros: None,
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
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m", "params_b": 8, "enabled": true } ] }"#,
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
        output: "text".into(),
        args: Vec::new(),
        params_b: None,
        price_per_mtok_usd: None,
        ..Default::default()
    };

    // Fresh state (no receipts): the FIRST enabled worker in config order.
    let r = Router::default();
    let pool = [w("a"), w("b")];
    assert_eq!(r.pick(pool.iter()).unwrap().name, "a");

    // Unexplored worker is tried before a failing one (exploration term).
    let mut r = Router::default();
    for _ in 0..3 {
        r.record("a", false, Some(1.0)); // a: 0/3 wins
    }
    assert_eq!(r.pick(pool.iter()).unwrap().name, "b");

    // Reliable worker wins at equal counts (exploitation term).
    let mut r = Router::default();
    for _ in 0..3 {
        r.record("a", true, Some(1.0)); // a: 3/3
        r.record("b", false, Some(1.0)); // b: 0/3
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
