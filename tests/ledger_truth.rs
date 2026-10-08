//! Ledger truth: the cost report must stop hiding two things and must show
//! the token data already sitting in the receipts.
//!
//! * a torn `*.json` receipt is reported by name, never silently swallowed,
//!   and never blocks the command (the checked loader feeds the READ paths);
//! * wasted spend is grouped by CAUSE — the text before the first `:` in the
//!   reason — so two failures that embed different file lists in the same
//!   cause collapse into one row instead of fragmenting and truncating;
//! * the TOKENS column sums the tokens recorded on the selected receipts and
//!   shows `-` when they carry none (legacy receipts render as before).
//!
//! `Store` is driven directly and `run::cost` / `run::status_board` are called
//! with a hand-built `Settings` (the pattern from `tests/e2e.rs::fixture`), so
//! each test is hermetic — a unique temp dir, removed on drop.

use agentflow::config::{self, Settings};
use agentflow::run::{self, CostFilter};
use agentflow::state::{Receipt, Store};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const TASKS_A: &str = r#"{ "tasks": [
    { "id": "A", "title": "a", "accept": "true" }
] }"#;

const TASKS_AB: &str = r#"{ "tasks": [
    { "id": "A", "title": "a", "accept": "true" },
    { "id": "B", "title": "b", "accept": "true" }
] }"#;

static N: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    dir: PathBuf,
    cfg: config::Config,
    st: Settings,
}

impl Fixture {
    fn new(tasks_json: &str) -> Fixture {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("af-ledger-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config_dir = dir.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("tasks.json"), tasks_json).unwrap();
        std::fs::write(
            config_dir.join("workers.json"),
            r#"{ "workers": [ { "name": "w1", "provider": "openai", "model": "m", "enabled": true } ] }"#,
        )
        .unwrap();
        let cfg = config::load(
            &config_dir.join("tasks.json"),
            &config_dir.join("workers.json"),
        )
        .unwrap();
        let st = Settings {
            repo_dir: dir.join("repo"),
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
        Fixture { dir, cfg, st }
    }

    fn store(&self) -> Store {
        Store::new(self.st.state_dir.clone())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn receipt(
    task: &str,
    attempt: u32,
    ts: u64,
    wall: f64,
    outcome: &str,
    error: Option<&str>,
) -> Receipt {
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
}

/// A torn receipt must be visible in the report whose job is to account for
/// spend: the checked loader feeds `run::cost` (and `af status`), so the file
/// is named — one warning line — and the valid history is still reported.
// spec: cli/cost-report#unreadable-receipts-are-reported-by-the-cost-report
#[test]
fn unreadable_receipts_are_reported_by_the_cost_report() {
    let f = Fixture::new(TASKS_A);
    let store = f.store();
    store
        .append_receipt(&receipt("A", 1, 1, 12.5, "merged", None))
        .unwrap();
    // Exactly what a crash mid-write used to leave behind.
    std::fs::write(
        store.receipt_dir().join("A-2-99-0x.json"),
        r#"{"task":"A","attempt":2,"worker"#,
    )
    .unwrap();

    let out = run::cost(&f.cfg, &f.st, &CostFilter::default());
    assert!(
        out.contains("A-2-99-0x.json"),
        "the cost report names the unreadable file: {out}"
    );
    assert!(
        out.contains("12.5"),
        "the readable receipt's spend is still reported: {out}"
    );

    // `af status` shares the read path and reports the same loss.
    let status = run::status_board(&f.cfg, &f.st);
    assert!(
        status.contains("A-2-99-0x.json"),
        "the status board names the unreadable file: {status}"
    );
}

/// Two failures whose `error` shares the text before the first `:` (the
/// cause) but lists different files afterwards must collapse into ONE cause
/// row whose seconds are the sum and whose count is both attempts; the old
/// 48-char key split them and cut the cause mid-word.
// spec: cli/cost-report#wasted-reasons-group-by-cause-not-by-file-list
#[test]
fn wasted_reasons_group_by_cause_not_by_file_list() {
    let f = Fixture::new(TASKS_A);
    let store = f.store();
    let cause = "attempt edited files out of scope";
    store
        .append_receipt(&receipt(
            "A",
            1,
            1,
            1231.8,
            "failed",
            Some("attempt edited files out of scope: src/run.rs (a"),
        ))
        .unwrap();
    store
        .append_receipt(&receipt(
            "A",
            2,
            2,
            968.9,
            "failed",
            Some("attempt edited files out of scope: src/run.rs, tests/other.rs"),
        ))
        .unwrap();

    let out = run::cost(&f.cfg, &f.st, &CostFilter::default());
    let primary: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with(&format!("  {cause}")))
        .collect();
    assert_eq!(primary.len(), 1, "one aggregated cause row: {out}");
    assert!(
        primary[0].starts_with(&format!("  {cause} ")),
        "the key is the whole cause: {out}"
    );
    assert!(
        !primary[0].contains(':'),
        "the primary key is the cause, not the full reason: {out}"
    );
    assert!(
        primary[0].contains("2200.7s"),
        "the row sums both failures' seconds: {out}"
    );
    assert!(
        primary[0].contains("(2)"),
        "the row counts both failures: {out}"
    );

    // Each distinct full reason (with its file list) survives as a sub-count.
    assert!(
        out.contains("src/run.rs (a"),
        "the first full reason is retained: {out}"
    );
    assert!(
        out.contains("tests/other.rs"),
        "the second full reason is retained: {out}"
    );
}

/// The TOKENS column shows the summed tokens when they are recorded and a
/// `-` placeholder when the receipts carry none, so a legacy report renders
/// cleanly rather than breaking or inventing a zero.
// spec: cli/cost-report#cost-report-shows-tokens-when-present
#[test]
fn cost_report_shows_tokens_when_present() {
    let f = Fixture::new(TASKS_AB);
    let store = f.store();
    let mut a1 = receipt("A", 1, 1, 1.0, "merged", None);
    a1.tokens = Some(100);
    let mut a2 = receipt("A", 2, 2, 2.0, "merged", None);
    a2.tokens = Some(250);
    store.append_receipt(&a1).unwrap();
    store.append_receipt(&a2).unwrap();
    // A legacy receipt: no tokens recorded.
    store
        .append_receipt(&receipt("B", 1, 3, 3.0, "merged", None))
        .unwrap();

    let out = run::cost(&f.cfg, &f.st, &CostFilter::default());
    assert!(
        out.contains("TOKENS"),
        "the header advertises the column: {out}"
    );
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10} {:<9} {:<8} {}",
            "TASK", "ATTEMPTS", "WALL_S", "TOKENS", "COST", "MODEL"
        )),
        "the header carries COST right after TOKENS: {out}"
    );
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "A", 2, 3.0, "350", "-", "m"
        )),
        "A shows its summed tokens (100 + 250); the worker declares no cost basis, so COST is `-`: {out}"
    );
    assert!(
        out.contains(&format!(
            "{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
            "B", 1, 3.0, "-", "-", "m"
        )),
        "B shows the placeholder when no tokens are recorded: {out}"
    );
}

/// The cost report gives recovered attempts their own `RECOVERED` outcome
/// line: a failed attempt whose archived work was merged by `af recover`/
/// reuse is not wasted, so it leaves `WASTED` (and its reason breakdown)
/// and its seconds are reported as reclaimed instead. The `recovered` marker
/// itself is a 0.0s non-attempt, so it does not inflate the wasted
/// denominator — the real failed attempt it pairs with stays one attempt.
// spec: cli/recovered-spend-in-the-cost-report
// spec: cli/recovered-spend-in-the-cost-report#recovered-failures-leave-the-wasted-total
#[test]
fn cost_report_shows_recovered_line() {
    let f = Fixture::new(TASKS_A);
    let store = f.store();
    store
        .append_receipt(&receipt(
            "A",
            1,
            1,
            100.0,
            "failed",
            Some("acceptance gate failed (exit 1): boom"),
        ))
        .unwrap();
    // The recovered marker names the same task + attempt and measures 0.0s.
    store
        .append_receipt(&receipt("A", 1, 2, 0.0, "recovered", None))
        .unwrap();

    let out = run::cost(&f.cfg, &f.st, &CostFilter::default());
    assert!(
        out.contains("RECOVERED: 1 attempt(s) — 100.0s of failed work reclaimed (not wasted)"),
        "recovered line names the reclaimed seconds: {out}"
    );
    assert!(
        out.contains("WASTED: 0.0s on 0 of 1 attempt(s) (0.0%)"),
        "the reclaimed failure leaves WASTED while its attempt stays counted: {out}"
    );
    assert!(
        !out.contains("WASTED BY REASON"),
        "a reclaimed failure has no wasted reason: {out}"
    );
}

/// Only the failed receipt PAIRED with a recovered marker (same task AND
/// attempt) leaves `WASTED`; an unpaired failure of a different attempt is
/// still waste and still appears in the by-reason breakdown.
// spec: cli/recovered-spend-in-the-cost-report#only-the-paired-failure-is-reclaimed
#[test]
fn recovered_failed_receipts_excluded_from_wasted() {
    let f = Fixture::new(TASKS_A);
    let store = f.store();
    // Attempt 1 failed and was later recovered …
    store
        .append_receipt(&receipt(
            "A",
            1,
            1,
            100.0,
            "failed",
            Some("acceptance gate failed (exit 1): boom"),
        ))
        .unwrap();
    store
        .append_receipt(&receipt("A", 1, 2, 0.0, "recovered", None))
        .unwrap();
    // … attempt 2 failed and was NOT.
    store
        .append_receipt(&receipt(
            "A",
            2,
            3,
            20.0,
            "failed",
            Some("agent exited NonZero (code 7)"),
        ))
        .unwrap();

    let out = run::cost(&f.cfg, &f.st, &CostFilter::default());
    assert!(
        out.contains("WASTED: 20.0s on 1 of 2 attempt(s) (50.0%)"),
        "only the unpaired failure is wasted; the marker is not an attempt: {out}"
    );
    assert!(
        out.contains("RECOVERED: 1 attempt(s) — 100.0s of failed work reclaimed (not wasted)"),
        "the paired failure's seconds are reclaimed: {out}"
    );
    assert!(
        out.contains("agent exited NonZero (code 7)"),
        "the true failure's reason stays in the breakdown: {out}"
    );
    assert!(
        !out.contains("acceptance gate failed"),
        "the reclaimed failure leaves the breakdown: {out}"
    );
}
