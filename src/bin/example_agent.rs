//! example_agent — a stub OpenAI-compatible agent CLI.
//!
//! Used by agentflow's own test suite (and by CI) as a deterministic stand-in
//! for `pi`/opencode/etc. Accepts the same argument shape as the real thing
//! (`--provider X --model Y -p @file`), ignores it, and behaves according to
//! a few environment variables:
//!
//! - `FAKE_AGENT_EXIT`: exit code (default 0)
//! - `FAKE_AGENT_TOUCH`: file to write into the current directory (default `DONE.txt`)
//! - `FAKE_AGENT_OUT`:   content to write (default a summary line)
//! - `FAKE_AGENT_ENV` / `FAKE_AGENT_ENV_NAMES`: sandbox env probe (see below)
//! - `FAKE_AGENT_TOUCH_FROM_MODEL`: when set (and `FAKE_AGENT_TOUCH` unset),
//!   write `{model}.txt` instead — lets two workers with distinct `--model`s
//!   produce distinct merged artifacts in a parallel-dispatch test.
//! - `FAKE_AGENT_SLEEP_MS`: optional fixed delay before doing work, so E2E
//!   tests can hold a task in the `running` state long enough to observe that
//!   several tasks are genuinely in flight at the same instant.
//! - `FAKE_AGENT_HANG_MS`: optional delay BEFORE any output at all (capped
//!   like `FAKE_AGENT_SLEEP_MS`), so a test can present a genuinely stalled
//!   agent — silent on stdout/stderr far longer than a stall window.
//!
//! Like a real agent, it commits its work to the current branch so the
//! orchestrator's merge actually carries the changes to the base branch.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Optional `--model` value (worker selection): lets a test give each
    // worker its own output file so two merged tasks leave two artifacts.
    let model = args
        .windows(2)
        .find(|w| w[0] == "--model")
        .and_then(|w| w.get(1))
        .cloned();
    let exit: i32 = std::env::var("FAKE_AGENT_EXIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Output file: explicit `FAKE_AGENT_TOUCH` wins, else (opt-in) the model
    // name, else the historical default.
    let touch = std::env::var("FAKE_AGENT_TOUCH")
        .ok()
        .or_else(|| {
            if std::env::var("FAKE_AGENT_TOUCH_FROM_MODEL").is_ok() {
                model.map(|m| format!("{m}.txt"))
            } else {
                None
            }
        })
        .unwrap_or_else(|| "DONE.txt".to_string());
    let out = std::env::var("FAKE_AGENT_OUT")
        .unwrap_or_else(|_| "example-agent: task complete".to_string());

    if exit != 0 {
        eprintln!("example_agent: failing with exit {exit}");
        return ExitCode::from(exit.clamp(0, 255) as u8);
    }
    // Watchdog-test knob: hang BEFORE writing any output — the stub stays
    // silent on stdout/stderr (an "agent stopped producing output" stall),
    // bounded exactly like FAKE_AGENT_SLEEP_MS so no test can ask for an
    // unbounded sleep.
    if let Some(ms) = std::env::var("FAKE_AGENT_HANG_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(2000)));
    }
    // Optional fixed delay so a test can observe several tasks running at
    // once (parallel-dispatch E2E proof). Bounded and deterministically short.
    if let Some(ms) = std::env::var("FAKE_AGENT_SLEEP_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(2000)));
    }
    // Env probe (sandbox tests): dump `FAKE_AGENT_ENV_NAMES` to `FAKE_AGENT_ENV`
    // as `NAME=value` / `NAME=<unset>` lines so tests can assert what the agent
    // child actually received.
    if let Ok(probe) = std::env::var("FAKE_AGENT_ENV") {
        let names = std::env::var("FAKE_AGENT_ENV_NAMES").unwrap_or_default();
        let mut s = String::new();
        for n in names.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match std::env::var(n) {
                Ok(v) => s.push_str(&format!("{n}={v}\n")),
                Err(_) => s.push_str(&format!("{n}=<unset>\n")),
            }
        }
        let _ = std::fs::write(&probe, s);
    }
    let _ = std::fs::write(&touch, format!("{out}\n"));
    if std::env::var("FAKE_AGENT_JSON").is_ok() {
        let total: u64 = std::env::var("FAKE_AGENT_JSON_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(13_525);
        emit_json_transcript(&out, total);
    } else {
        println!("{out}");
    }

    // Commit the work so merges carry it (ignore commit failures — the
    // acceptance gate is the real check).
    let _ = std::process::Command::new("git")
        .args(["add", "-A"])
        .status();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "example_agent: task work"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    ExitCode::SUCCESS
}

/// Emit a realistic pi-style `--mode json` JSON Lines transcript on stdout:
/// one JSON object per line, the assistant text split across two
/// `text_delta` chunks (char-boundary safe), one tool call, a PARTIAL usage
/// on the streaming events and the full/final usage on `message_end` /
/// `turn_end` / `agent_end` (identical totals — the authoritative number is
/// the last one seen).
fn emit_json_transcript(text: &str, total: u64) {
    let usage = |t: u64| {
        serde_json::json!({
            "input": t.saturating_sub(2),
            "output": 2.min(t),
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0,
            "totalTokens": t,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}
        })
    };
    let partial = total.saturating_sub(1);
    // Char-boundary-safe midpoint so a multi-byte summary splits cleanly.
    let mid = text
        .char_indices()
        .nth(text.chars().count() / 2)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let (a, b) = text.split_at(mid);
    let events = [
        serde_json::json!({"type": "session", "sessionId": "fake-session"}),
        serde_json::json!({"type": "message_start", "message": {"role": "assistant", "content": []}}),
        serde_json::json!({"type": "message_update", "usage": usage(partial),
            "assistantMessageEvent": {"type": "tool_call", "toolCallId": "c1", "title": "inspect worktree"}}),
        serde_json::json!({"type": "message_update", "usage": usage(partial),
            "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": a}}),
        serde_json::json!({"type": "message_update", "usage": usage(partial),
            "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": b}}),
        serde_json::json!({"type": "message_end", "message": {"role": "assistant",
            "content": [{"type": "text", "text": text}], "provider": "example", "model": "stub",
            "usage": usage(total), "stopReason": "stop"}}),
        serde_json::json!({"type": "turn_end", "usage": usage(total)}),
        serde_json::json!({"type": "agent_end", "messages": [], "usage": usage(total), "willRetry": false}),
        serde_json::json!({"type": "agent_settled"}),
    ];
    for line in events {
        println!("{line}");
    }
}
