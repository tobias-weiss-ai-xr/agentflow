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
//! - `FAKE_AGENT_JSON`: emit a pi-style `--mode json` JSON Lines transcript
//!   instead of the plain summary line (see `json_transcript_events`)
//! - `FAKE_AGENT_JSON_TOKENS`: the transcript's final `totalTokens`
//! - `FAKE_AGENT_JSON_COST`:   the transcript's `usage.cost.total`, USD as a
//!   decimal string (e.g. "0.0123"); the default 0 means "not tracked",
//!   exactly like every real provider that reports no price
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
        // The provider's OWN reported cost, USD (default 0 = not tracked,
        // like the real providers that report no price). Parsing stays this
        // thin on purpose: llvm-cov does not credit lines inside a spawned
        // stub binary, so everything real lives in library code.
        let cost_usd: f64 = std::env::var("FAKE_AGENT_JSON_COST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        emit_json_transcript(&out, total, cost_usd);
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

/// Emit a realistic pi-style `--mode json` JSON Lines transcript on stdout.
fn emit_json_transcript(text: &str, total: u64, cost_usd: f64) {
    for event in json_transcript_events(text, total, cost_usd) {
        println!("{event}");
    }
}

/// The events `emit_json_transcript` prints, split out so the parser's own
/// test suite can round-trip them through `agentflow::transcript::parse`:
/// one JSON object per line, the assistant text split across two
/// `text_delta` chunks (char-boundary safe), a thinking block and a tool call
/// WITH their streaming chunks, the tool's result as a `toolResult` message,
/// a PARTIAL usage on the streaming events and the full/final usage on
/// `message_end` / `turn_end` / `agent_end` (identical totals — the
/// authoritative number is the last one seen).
fn json_transcript_events(text: &str, total: u64, cost_usd: f64) -> Vec<serde_json::Value> {
    let usage = |t: u64| {
        serde_json::json!({
            "input": t.saturating_sub(2),
            "output": 2.min(t),
            "cacheRead": 0,
            "cacheWrite": 0,
            "reasoning": 0,
            "totalTokens": t,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": cost_usd}
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
    vec![
        serde_json::json!({"type": "session", "sessionId": "fake-session"}),
        serde_json::json!({"type": "message_start", "message": {"role": "user", "content": [{"type": "text", "text": "<task prompt>"}]}}),
        serde_json::json!({"type": "message_end", "message": {"role": "user", "content": [{"type": "text", "text": "<task prompt>"}]}}),
        serde_json::json!({"type": "message_start", "message": {"role": "assistant", "content": []}}),
        // A thinking block: start + chunks + end. The chunks must NOT become
        // log lines (one real tool call produced 110 of them).
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "thinking_start", "contentIndex": 0}}),
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "thinking_delta", "contentIndex": 0, "delta": "Let me "}}),
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "thinking_delta", "contentIndex": 0, "delta": "inspect the worktree."}}),
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "thinking_end", "contentIndex": 0, "content": "Let me inspect the worktree."}}),
        // A tool call: start + argument chunks + the finished call.
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "toolcall_start", "contentIndex": 1}}),
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "toolcall_delta", "contentIndex": 1, "delta": "{\"command\":"}}),
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "toolcall_delta", "contentIndex": 1, "delta": "\"git status\"}"}}),
        serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "toolcall_end", "contentIndex": 1,
            "toolCall": {"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"command": "git status"}}}}),
        serde_json::json!({"type": "message_end", "message": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "Let me inspect the worktree."},
            {"type": "toolCall", "name": "bash"}], "usage": usage(partial)}}),
        // The tool runs; its RESULT comes back as a `toolResult` message.
        serde_json::json!({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash", "args": {"command": "git status"}}),
        serde_json::json!({"type": "tool_execution_end", "toolCallId": "c1", "toolName": "bash",
            "result": {"content": [{"type": "text", "text": "stub tool output"}]}, "isError": false}),
        serde_json::json!({"type": "message_end", "message": {"role": "toolResult", "content": [{"type": "text", "text": "stub tool output"}]}}),
        serde_json::json!({"type": "turn_end", "usage": usage(partial)}),
        // Turn 2: the assistant's answer, streamed in chunks then finalized.
        serde_json::json!({"type": "message_start", "message": {"role": "assistant", "content": []}}),
        serde_json::json!({"type": "message_update", "usage": usage(partial),
            "assistantMessageEvent": {"type": "text_start", "contentIndex": 0}}),
        serde_json::json!({"type": "message_update", "usage": usage(partial),
            "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": a}}),
        serde_json::json!({"type": "message_update", "usage": usage(partial),
            "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": b}}),
        serde_json::json!({"type": "message_update", "usage": usage(partial),
            "assistantMessageEvent": {"type": "text_end", "contentIndex": 0, "content": text}}),
        serde_json::json!({"type": "message_end", "message": {"role": "assistant",
            "content": [{"type": "text", "text": text}], "provider": "example", "model": "stub",
            "usage": usage(total), "stopReason": "stop"}}),
        serde_json::json!({"type": "turn_end", "usage": usage(total)}),
        serde_json::json!({"type": "agent_end", "messages": [], "usage": usage(total), "willRetry": false}),
        serde_json::json!({"type": "agent_settled"}),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stub is the transcript parser's fixture: emit → parse must
    /// round-trip. This is what proves the JSON the stub prints is the same
    /// shape the parser was built against, so an e2e `af cost` showing
    /// tokens can only mean the real path works.
    #[test]
    fn the_json_fixture_round_trips_through_the_transcript_parser() {
        let stream: String = json_transcript_events("done", 4242, 0.0)
            .iter()
            .map(|e| format!("{e}\n"))
            .collect();
        let t = agentflow::transcript::parse(&stream);
        // The final total, not the streaming partial.
        assert_eq!(t.usage.map(|u| u.total_tokens), Some(4242));
        // Rendered as prose for a human, with roles kept honest.
        assert!(t.rendered.contains("user: <task prompt>"), "{}", t.rendered);
        assert!(t.rendered.contains("assistant: done"), "{}", t.rendered);
        // The tool call and its output, without the streaming chunks.
        assert!(
            t.rendered.contains("[tool] bash: git status"),
            "{}",
            t.rendered
        );
        assert!(t.rendered.contains("stub tool output"), "{}", t.rendered);
        for chunk in ["thinking_delta", "toolcall_delta", "text_delta"] {
            assert!(!t.rendered.contains(chunk), "{}", t.rendered);
        }
        // The default cost is 0 — "not tracked" — so the fixture's default
        // output stays byte-identical to the pre-cost stub: no `$` anywhere.
        assert_eq!(t.usage.and_then(|u| u.cost_micros), None);
        assert!(!t.rendered.contains('$'), "{}", t.rendered);
    }

    /// `FAKE_AGENT_JSON_COST` (the provider's own reported cost) rides the
    /// usage objects through to the parser: a positive dollar amount is
    /// captured as integer micro-USD on the receipt path, and the default
    /// 0 is captured as NOTHING — a zero is not a measurement.
    #[test]
    fn the_fixture_carries_a_reported_cost_through_to_micro_usd() {
        let stream: String = json_transcript_events("done", 4242, 0.0123)
            .iter()
            .map(|e| format!("{e}\n"))
            .collect();
        let t = agentflow::transcript::parse(&stream);
        // 0.0123 USD → 12300 micro-USD, from the FINAL usage (the streaming
        // events carry the same cost, so last-wins is also exercised).
        assert_eq!(
            t.usage,
            Some(agentflow::transcript::Usage {
                input: 4240,
                output: 2,
                total_tokens: 4242,
                cost_micros: Some(12_300),
            })
        );
        assert!(
            t.rendered
                .contains("usage: 4240 in, 2 out, 4242 total ($0.0123)"),
            "{}",
            t.rendered
        );
    }

    /// A multi-byte summary must split at a char boundary, not mid-codepoint.
    #[test]
    fn the_fixture_splits_multibyte_text_without_panicking() {
        let stream: String = json_transcript_events("fertig \u{2713} \u{2713}", 10, 0.0)
            .iter()
            .map(|e| format!("{e}\n"))
            .collect();
        let t = agentflow::transcript::parse(&stream);
        assert!(
            t.rendered.contains("assistant: fertig \u{2713} \u{2713}"),
            "{}",
            t.rendered
        );
    }
}
