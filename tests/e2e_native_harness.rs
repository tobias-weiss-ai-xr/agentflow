//! E2E: native harness (`cli: "builtin"`) against a local stub HTTP server
//! speaking canned OpenAI chat/completions responses + a scratch git repo.
//! No live network, no external agent CLI — deterministic CI.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::Store;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn git(repo: &std::path::Path, args: &[&str]) {
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

/// Serves each queued (status, body) pair on one incoming connection, in
/// order. Excess connections get a 200 `{}` (the loop will fail it as
/// malformed, which keeps a buggy test loud instead of hung).
struct StubLlm {
    url: String,
    /// Bodies of every request served, in order — lets tests assert what the
    /// harness actually SENT. The turn-1 shape bug (system-only messages, 400
    /// on strict gateways) passed the old body-blind stub unnoticed.
    bodies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

/// Drain ONE full request (headers + Content-Length body) so the socket has
/// no unread receive data when the stub closes it. Closing with unread data
/// makes Windows RST the connection; the client then sees a transport error
/// (10054) instead of the queued response and the E2E flakes. Harness
/// request bodies are a few KiB (system prompt + envelope), so this is cheap.
fn drain_request(s: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return String::new(), // client gone; best-effort drain
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let len = String::from_utf8_lossy(&buf[..end])
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            let mut left = len.saturating_sub(buf.len() - (end + 4));
            while left > 0 {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => return String::new(),
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        left = left.saturating_sub(n);
                    }
                }
            }
            let start = end + 4;
            return String::from_utf8_lossy(&buf[start..start + len]).into_owned();
        }
    }
}

impl StubLlm {
    fn spawn(responses: Vec<(u16, String)>) -> StubLlm {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let queue: Arc<Mutex<VecDeque<(u16, String)>>> =
            Arc::new(Mutex::new(responses.into()));
        let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = bodies.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let next = queue.lock().unwrap().pop_front();
                let (code, body) = next.unwrap_or((200, "{}".to_string()));
                let req_body = drain_request(&mut s);
                sink.lock().unwrap().push(req_body);
                let reason = if code == 200 { "OK" } else { "Internal Server Error" };
                let resp = format!(
                    "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        StubLlm { url: format!("http://{addr}/v1"), bodies }
    }
}

/// One canned assistant turn that calls `bash` (OpenAI tool_calls shape).
fn bash_turn(command: &str) -> (u16, String) {
    let args = serde_json::json!({ "command": command }).to_string();
    (
        200,
        serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": null,
                "tool_calls": [{"id": "c1", "type": "function",
                    "function": {"name": "bash", "arguments": args}}]}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })
        .to_string(),
    )
}

/// One canned final assistant turn (text, no tool calls).
fn final_turn(text: &str) -> (u16, String) {
    (
        200,
        serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": text}}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5}
        })
        .to_string(),
    )
}

fn native_worker_json(url: &str, max_attempts: u32, max_turns: u32) -> String {
    format!(
        r#"{{ "defaults": {{ "max_attempts": {max_attempts}, "accept_timeout_s": 10, "agent_timeout_s": 60 }},
            "workers": [ {{ "name": "nat", "provider": "stub", "model": "stub-1",
                            "api_base": "{url}", "cli": "builtin", "max_turns": {max_turns},
                            "enabled": true }} ] }}"#
    )
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

struct Fixture {
    #[allow(dead_code)]
    dir: PathBuf,
    repo: PathBuf,
    cfg: config::Config,
    st: Settings,
}

fn fixture(tasks_json: &str, workers_json: &str) -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-native-e2e-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    // Identity for commits/merges. git user env passes the harness's
    // sandbox allowlist (AGENT_ENV_BASE carries the GIT_* names), and the
    // worktree shares this repo's common config — so both the agent's in-
    // worktree commit and af's own merges get an identity.
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
        max_parallel: 1,
        branch_prefix: "tf".into(),
        poll_secs: 1,
        gate_env: vec![],
        tasks_file: config_dir.join("tasks.json"),
        workers_file: config_dir.join("workers.json"),
        prompt_file: PathBuf::from("prompts/worker.md"),
        agent_timeout_s: 60,
        agent_stall_s: 0,
        agent_max_turns: 0,
        max_wall_clock_s: 0,
        sandbox_cmd: vec![],
    };
    Fixture { dir, repo, cfg, st }
}

fn task_json(gate: &str) -> String {
    format!(
        r#"{{ "tasks": [ {{ "id": "A", "title": "touch a file", "scope": ["A.txt"],
             "accept": "{gate}" }} ] }}"#
    )
}

#[test]
fn native_harness_happy_path_merges() {
    // The stub drives the whole attempt: turn 1 = bash that CREATES A.txt
    // and commits it (the gate needs the file; nothing else creates it),
    // turn 2 = a plain "done" answer. The bash tool runs `cmd /C`, where
    // `echo work > A.txt && git add … && git commit …` creates + commits.
    let stub = StubLlm::spawn(vec![
        bash_turn("echo work > A.txt && git add A.txt && git commit -m w"),
        final_turn("done"),
    ]);
    let f = fixture(&task_json(&gate_cmd("A.txt")), &native_worker_json(&stub.url, 1, 8));
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "run should exit 0 (all done)");
    // Regression net (found in production): turn 1's request MUST be
    // [system, user] — strict gateways (z.ai 1214, academiccloud "No user
    // query found") 400 a system-only messages array.
    let first = stub.bodies.lock().unwrap()[0].clone();
    let req: serde_json::Value = serde_json::from_str(&first).expect("stub saw a JSON body");
    let msgs = req["messages"].as_array().expect("messages array");
    assert_eq!(msgs.len(), 2, "turn 1 = system + user");
    assert_eq!(msgs[0]["role"], "system");
    assert!(!msgs[0]["content"].as_str().unwrap_or("").is_empty(), "system prompt non-empty");
    assert_eq!(msgs[1]["role"], "user");
    assert!(msgs[1]["content"].as_str().unwrap_or("").contains("touch a file"), "task prompt is the user message");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert!(f.repo.join("A.txt").exists(), "merged to main");
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    let r = receipts.iter().find(|r| r.task == "A").expect("receipt");
    assert_eq!(r.outcome, "merged");
    assert_eq!(r.tokens, Some(20), "15 (bash turn) + 5 (final turn)");
}

#[test]
fn native_harness_turn_cap_fails_and_archives() {
    // The model never stops: queue 8 identical tool-call turns; cap at 2.
    // The cap is checked BEFORE a turn dispatches (harness::run), so
    // turns 1 AND 2 each run and pay 15 tokens — the cap fires on turn 3.
    // The attempt then fails at max_attempts=1 → task terminal-Failed →
    // the run loop reports the deadlock exit code 2 (same as e2e.rs's
    // agent_failure_fails_task), NOT 0.
    let stub = StubLlm::spawn((0..8).map(|_| bash_turn("echo hi")).collect());
    let f = fixture(&task_json(&gate_cmd("A.txt")), &native_worker_json(&stub.url, 1, 2));
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "task terminal (failed) → deadlock exit 2");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    let r = receipts.iter().find(|r| r.task == "A").expect("receipt");
    assert!(r.error.as_deref().unwrap_or("").contains("turn cap 2"), "{:?}", r.error);
    assert_eq!(r.tokens, Some(30), "turns 1+2 paid 15 each before the cap");
}

#[test]
fn native_harness_provider_error_fails_with_reason() {
    // The attempt's only LLM round-trip is a 500 → harness ProviderError →
    // Outcome::Failed with the precise reason on the receipt; no tokens.
    // max_attempts=1 → task terminal-Failed → deadlock exit 2.
    let stub = StubLlm::spawn(vec![(500, r#"{"error":"boom"}"#.to_string())]);
    let f = fixture(&task_json(&gate_cmd("A.txt")), &native_worker_json(&stub.url, 1, 8));
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 2, "task terminal (failed) → deadlock exit 2");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    let r = receipts.iter().find(|r| r.task == "A").expect("receipt");
    let err = r.error.as_deref().unwrap_or("");
    assert!(err.contains("harness: provider http 500"), "{err}");
    assert_eq!(r.tokens, Some(0), "no paid round-trip succeeded");
}
