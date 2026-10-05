//! E2E: a worker's `output: "json"` mode captures REAL token usage from
//! the agent CLI's JSON Lines transcript (config spec: Worker schema
//! loading) — the attempt receipt's `tokens` carries the transcript's
//! final `totalTokens` and `state/logs/<task>.log` carries a human-readable
//! rendering (assistant text, compact activity) instead of raw JSONL —
//! while the DEFAULT text mode keeps today's behaviour exactly: no extra
//! argv, the raw output in the log, `tokens: None`.
//!
//! Same substrate as tests/e2e.rs / tests/budget.rs: a scratch git repo in
//! a uniquely named temp dir, repo-local git identity, and the bundled
//! `example_agent` stub as the agent CLI. `FAKE_AGENT_JSON` makes the stub
//! emit a realistic `--mode json` transcript with KNOWN usage numbers (the
//! streaming events carry a partial total one smaller, so the receipt
//! assertion also proves the parser took the LAST usage, not the first).
//! The knobs mutate process-global env vars and must ride
//! `TF_AGENT_ENV_PASSTHROUGH` (the sandbox strips the agent child's env),
//! so the tests share one mutex and run serialized.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::Store;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const AGENT: &str = env!("CARGO_BIN_EXE_example_agent");

/// Env-mutation guard (module docs): the FAKE_AGENT_* knobs and
/// TF_AGENT_ENV_PASSTHROUGH are process-global.
static ENV_GUARD: Mutex<()> = Mutex::new(());

/// The distinctive total the stub is told to report — and the receipt must
/// carry. The stub's streaming events report one less, so equality here
/// proves the FINAL usage won.
const KNOWN_TOTAL: u64 = 4242;
/// The stub's usage splits the total as input = total - 2, output = 2.
const EXPECTED_USAGE_LINE: &str = "usage: 4240 in, 2 out, 4242 total";
/// The stub's default summary line — the assistant text of the transcript.
const ASSISTANT_TEXT: &str = "example-agent: task complete";

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

/// Scratch repo + config + hand-built Settings whose agent CLI is the
/// bundled stub. Mirrors tests/budget.rs::fixture. The FAKE_AGENT knobs
/// must ride `TF_AGENT_ENV_PASSTHROUGH` because the sandbox strips the
/// agent child's env; every knob starts removed so each arm arms exactly
/// what it needs.
fn fixture(tasks_json: &str, workers_json: &str, tag: &str) -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-token-cost-{}-{n}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var(
        "TF_AGENT_ENV_PASSTHROUGH",
        "FAKE_AGENT_JSON,FAKE_AGENT_JSON_TOKENS,FAKE_AGENT_EXIT,FAKE_AGENT_TOUCH,FAKE_AGENT_OUT",
    );
    std::env::remove_var("FAKE_AGENT_JSON");
    std::env::remove_var("FAKE_AGENT_JSON_TOKENS");
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");

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

fn tasks_json() -> String {
    format!(
        r#"{{ "tasks": [ {{"id":"A","title":"emit a transcript","scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
        g = gate_cmd("DONE.txt")
    )
}

/// Worker json with an explicit `output` (Some) or the field omitted
/// entirely (None) — the legacy config shape.
fn workers_json(output: Option<&str>) -> String {
    let agent_escaped = AGENT.replace('\\', "\\\\");
    let output_field = match output {
        Some(mode) => format!(r#", "output": "{mode}""#),
        None => String::new(),
    };
    format!(
        r#"{{ "workers": [ {{ "name": "w1", "provider": "p", "model": "m",
                            "enabled": true, "cli": "{agent_escaped}"{output_field} }} ] }}"#
    )
}

/// End-to-end: `output: "json"` turns the agent CLI's JSON Lines transcript
/// into (a) a receipt with REAL token usage and (b) a human-readable task
/// log — while a worker that omits `output` (the default text mode) still
/// produces `tokens: None` and its unchanged raw log. The config contract
/// is pinned first: an absent field defaults to `"text"`, and a typo is
/// rejected at load time naming the worker and the accepted values.
// spec: config/worker-schema-loading#output-defaults-to-text
// spec: config/worker-schema-loading#json-output-mode-captures-token-usage
// spec: config/worker-schema-loading#unknown-output-value-is-rejected
#[test]
fn json_transcript_yields_tokens_and_a_readable_log() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());

    // ------------------------------------------------------------------
    // Config contract (fails fastest): absent `output` ⇒ "text"; a typo is
    // a load-time error naming the worker and the accepted values — it
    // must not silently disable the feature.
    // ------------------------------------------------------------------
    let d = std::env::temp_dir().join(format!("af-token-cost-cfg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("tasks.json"),
        r#"{ "tasks": [{"id":"A","title":"a","accept":"true"}] }"#,
    )
    .unwrap();
    std::fs::write(
        d.join("workers-text.json"),
        r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}] }"#,
    )
    .unwrap();
    let cfg = config::load(&d.join("tasks.json"), &d.join("workers-text.json")).unwrap();
    assert_eq!(
        cfg.workers[0].output, "text",
        "absent output field must default to text (legacy configs untouched)"
    );
    std::fs::write(
        d.join("workers-typo.json"),
        r#"{ "workers": [{"name":"w1","provider":"p","model":"m","output":"jsn"}] }"#,
    )
    .unwrap();
    let err = config::load(&d.join("tasks.json"), &d.join("workers-typo.json")).unwrap_err();
    assert!(
        err.contains("worker 'w1'") && err.contains("text") && err.contains("json"),
        "a typo'd output value must be rejected naming the worker and the accepted values: {err}"
    );

    // ------------------------------------------------------------------
    // Arm 1: json mode — real tokens, readable log.
    // ------------------------------------------------------------------
    let f = fixture(&tasks_json(), &workers_json(Some("json")), "json");
    std::env::set_var("FAKE_AGENT_JSON", "1");
    std::env::set_var("FAKE_AGENT_JSON_TOKENS", KNOWN_TOTAL.to_string());

    assert_eq!(
        run::run_loop(&f.cfg, &f.st, &RunOptions::default()),
        0,
        "the json-mode campaign must complete end to end"
    );
    let status = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(status["A"].state, TaskState::Done);
    assert!(f.repo.join("DONE.txt").exists(), "the attempt merged");

    // (a) The receipt carries the transcript's FINAL total (the streaming
    // events report KNOWN_TOTAL - 1, so this also proves last-usage-wins).
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    let receipt = receipts
        .iter()
        .find(|r| r.task == "A")
        .expect("the attempt left a receipt");
    assert_eq!(receipt.tokens, Some(KNOWN_TOTAL), "receipts: {receipts:?}");
    assert!(
        receipt.wall_clock_s >= 0.0,
        "wall_clock_s is untouched by the feature"
    );

    // (b) The log is for humans: the assistant text is rendered, tool
    // activity stays visible in compact form, and NO raw JSONL survives.
    let log =
        std::fs::read_to_string(f.st.state_dir.join("logs").join("A.log")).expect("log exists");
    assert!(
        !log.contains("\"type\"") && !log.contains("totalTokens"),
        "raw JSONL must not land in the task log: {log}"
    );
    assert!(
        log.contains(&format!("assistant: {ASSISTANT_TEXT}")),
        "the rendered assistant text must be in the log: {log}"
    );
    assert!(
        log.contains("[tool_call] inspect worktree"),
        "tool activity stays visible in compact form: {log}"
    );
    assert!(
        log.contains(EXPECTED_USAGE_LINE),
        "the captured usage is summarized in the log: {log}"
    );

    // ------------------------------------------------------------------
    // Arm 2: default text mode — byte-for-byte the legacy behaviour.
    // ------------------------------------------------------------------
    // fixture() removes the FAKE_AGENT_JSON knobs again.
    let f2 = fixture(&tasks_json(), &workers_json(None), "text");

    assert_eq!(
        run::run_loop(&f2.cfg, &f2.st, &RunOptions::default()),
        0,
        "a worker without `output` must keep working exactly as before"
    );
    let status2 = Store::new(f2.st.state_dir.clone()).load();
    assert_eq!(status2["A"].state, TaskState::Done);
    assert!(f2.repo.join("DONE.txt").exists(), "the attempt merged");

    let receipts2 = Store::new(f2.st.state_dir.clone()).load_receipts();
    let receipt2 = receipts2
        .iter()
        .find(|r| r.task == "A")
        .expect("the text-mode attempt left a receipt");
    assert_eq!(
        receipt2.tokens, None,
        "text mode captures no tokens (legacy guarantee): {receipts2:?}"
    );

    // The raw agent output is still the log — no rendering happened.
    let log2 =
        std::fs::read_to_string(f2.st.state_dir.join("logs").join("A.log")).expect("log exists");
    assert!(
        log2.contains(ASSISTANT_TEXT),
        "text mode keeps the raw output in the log: {log2}"
    );
    assert!(
        !log2.contains("assistant:") && !log2.contains("[tool_call]"),
        "text mode does not render anything: {log2}"
    );

    let _ = std::fs::remove_dir_all(&d);
}
