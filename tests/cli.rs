//! CLI contract tests: spawn the real `af` binary and assert exit codes and
//! output shape (covers main() dispatch, load_cfg, and the command arms).

use agentflow::config::{Config, Settings};
use agentflow::state::{Receipt, Store, TaskStatus};
use agentflow::{config, run};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

struct Cli {
    dir: PathBuf,
}

impl Cli {
    fn new() -> Cli {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("af-cli-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config_dir = dir.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("tasks.json"), TASKS).unwrap();
        std::fs::write(config_dir.join("workers.json"), WORKERS).unwrap();
        Cli { dir }
    }

    fn af(&self, args: &[&str]) -> (i32, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_af"))
            .args(args)
            .env("TF_TASKS_JSON", self.dir.join("config").join("tasks.json"))
            .env("TF_WORKERS_JSON", self.dir.join("config").join("workers.json"))
            .env("TF_STATE_DIR", self.dir.join("state"))
            .env("TF_REPO_DIR", self.dir.join("repo"))
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
        let (cfg, st) = self.settings();
        let mut map = HashMap::new();
        let mut s = TaskStatus::default();
        s.state = agentflow::config::TaskState::Done;
        s.attempts = 1;
        map.insert(id.to_string(), s);
        Store::new(st.state_dir.clone()).save(&map).unwrap();
        let _ = &cfg;
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
        .env("TF_WORKERS_JSON", cli.dir.join("config").join("workers.json"))
        .env("TF_STATE_DIR", cli.dir.join("state"))
        .env("TF_REPO_DIR", cli.dir.join("repo"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("config error"));
}

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
fn api_results_requires_task_flag() {
    let cli = Cli::new();
    assert_eq!(cli.af(&["api", "results"]).0, 2);
    cli.seed_done_task("A");
    cli.seed_receipt("A");
    let (code, out) = cli.af(&["api", "results", "--task", "A"]);
    assert_eq!(code, 0);
    assert!(out.contains("task A:"), "results header: {out}");
    assert!(out.contains("no such task") == false);
    let (code, out) = cli.af(&["api", "results", "--task", "NOPE"]);
    assert_eq!(code, 0);
    assert!(out.contains("no such task"));
}

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

#[test]
fn dry_run_prints_plan_without_spawning_agents() {
    let cli = Cli::new();
    let (code, out) = cli.af(&["run", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(out.contains("Dry run"));
    assert!(out.contains("A"));
}
