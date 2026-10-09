//! Native agent harness (`cli: "builtin"`): af owns the LLM loop.
//! ADR-14 amends ADR-1 narrowly: the orchestrator may originate an agent
//! loop, but only inside this worker mode. CLI workers keep the ADR-1
//! subprocess contract unchanged.
//!
//! Wire format: OpenAI `POST {api_base}/chat/completions` with `tools` —
//! the ONE format every af worker target (litellm, zai, vLLM) speaks.
//! Tools: bash (platform shell via `gate::shell`), write, edit. Every tool
//! call is logged to the attempt log; tool side effects run under the same
//! env allowlist as a CLI agent child (sandbox layer 1), so native mode is
//! strictly tighter than CLI mode.

use crate::config::{Settings, Worker};
use crate::subprocess::{self, CmdKind, EnvMode};
use serde_json::{json, Value};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Worker `cli` value that selects this harness.
pub const BUILTIN: &str = "builtin";

const DEFAULT_MAX_TURNS: u32 = 32;
/// Per-tool-call hard timeout. A hung `bash` call stops burning the clock
/// without dragging the whole attempt down with it.
const TOOL_TIMEOUT: Duration = Duration::from_secs(300);
/// Tool output is truncated into the tool result (chars, not bytes — never
/// split a UTF-8 char) so one noisy command cannot blow the context.
const TOOL_OUTPUT_MAX: usize = 16 * 1024;

/// Why the loop stopped. `Normal` = the model answered with text and no
/// tool calls; everything else maps onto the existing failure classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Normal,
    TurnCap,
    Timeout,
    ProviderError,
}

impl Stop {
    /// CmdKind classification for the synthetic CmdOut handed to the
    /// unchanged downstream failure machinery.
    pub fn cmd_kind(self) -> CmdKind {
        match self {
            Stop::Normal => CmdKind::Success,
            Stop::TurnCap => CmdKind::Stalled,
            Stop::Timeout => CmdKind::Timeout,
            Stop::ProviderError => CmdKind::NonZero,
        }
    }
}

/// One harness run's result. `total_tokens` is the accumulated
/// `usage.total_tokens` across all round-trips (first-class receipt data,
/// no transcript parsing).
#[derive(Debug)]
pub struct Output {
    pub final_text: String,
    pub total_tokens: u64,
    pub turns: u32,
    pub stop: Stop,
    /// Machine-readable failure reason for the receipt (`harness: …`).
    pub error: Option<String>,
}

/// Effective turn cap: worker field wins, then TF_AGENT_MAX_TURNS (>0),
/// then 32.
fn effective_max_turns(worker: &Worker, st: &Settings) -> u32 {
    worker
        .max_turns
        .or((st.agent_max_turns > 0).then_some(st.agent_max_turns))
        .unwrap_or(DEFAULT_MAX_TURNS)
}

/// Lexical path containment: reject absolute paths and any `..` that would
/// climb out of the worktree. `ponytail:` lexical only — a symlink inside
/// the worktree pointing out is not detected; tighten if agents start
/// creating symlinks (none do today).
fn resolve_inside(wt: &Path, p: &str) -> Result<PathBuf, String> {
    if p.is_empty() {
        return Err("empty path".into());
    }
    let rel = Path::new(p);
    if rel.is_absolute()
        // Windows-only: `/etc/passwd` is root-relative, not absolute —
        // `is_absolute()` is false, yet it resolves outside the worktree.
        // Reject any RootDir first component so containment holds on both OSes.
        || matches!(rel.components().next(), Some(std::path::Component::RootDir))
    {
        return Err(format!("absolute path not allowed: {p}"));
    }
    let mut norm = wt.to_path_buf();
    for c in rel.components() {
        match c {
            std::path::Component::ParentDir => {
                if !norm.pop() || !norm.starts_with(wt) {
                    return Err(format!("path escapes worktree: {p}"));
                }
            }
            std::path::Component::CurDir => {}
            // Drive-relative paths like `C:foo` are neither absolute nor
            // RootDir, yet pushing a Prefix component replaces the base and
            // escapes the worktree.
            std::path::Component::Prefix(_) => return Err(format!("path escapes worktree: {p}")),
            other => norm.push(other.as_os_str()),
        }
    }
    // Cheap invariant: containment must still hold after the loop (catches
    // any future component type that escapes).
    if !norm.starts_with(wt) {
        return Err(format!("path escapes worktree: {p}"));
    }
    Ok(norm)
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn tool_write(wt: &Path, path: &str, content: &str) -> Result<String, String> {
    let full = resolve_inside(wt, path)?;
    std::fs::write(&full, content).map_err(|e| format!("write {path}: {e}"))?;
    Ok(format!("wrote {} ({} bytes)", path, content.len()))
}

fn tool_edit(wt: &Path, path: &str, old: &str, new: &str) -> Result<String, String> {
    let full = resolve_inside(wt, path)?;
    let s = std::fs::read_to_string(&full).map_err(|e| format!("read {path}: {e}"))?;
    let n = s.matches(old).count();
    if n == 0 {
        return Err(format!("edit {path}: old string not found"));
    }
    if n > 1 {
        return Err(format!("edit {path}: old string not unique ({n} occurrences)"));
    }
    std::fs::write(&full, s.replacen(old, new, 1))
        .map_err(|e| format!("write {path}: {e}"))?;
    Ok(format!("edited {path}"))
}

fn tool_bash(
    wt: &Path,
    command: &str,
    env: &[(String, String)],
    env_allow: &[String],
) -> Result<String, String> {
    let (shell, flag) = crate::gate::shell();
    let out = subprocess::run_with_stall(
        shell,
        &[flag.to_string(), command.to_string()],
        Some(wt),
        env,
        EnvMode::Allowlist(env_allow.to_vec()),
        TOOL_TIMEOUT,
        None,
    );
    let body = truncate_chars(&out.combined(), TOOL_OUTPUT_MAX);
    Ok(format!(
        "exit {}\n{}",
        out.code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
        body
    ))
}

/// Dispatch one tool call. Tool errors are the model's problem: they come
/// back as the tool-result content so the model can correct course — they
/// do NOT abort the attempt.
fn run_tool(
    name: &str,
    args: &str,
    wt: &Path,
    env: &[(String, String)],
    env_allow: &[String],
    log: &dyn Fn(&str),
) -> String {
    let parsed: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
    let field = |k: &str| parsed.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let req_field = |k: &str, tool: &str| {
        field(k).ok_or_else(|| format!("{tool}: missing or non-string '{k}'"))
    };
    let result = match name {
        "bash" => field("command")
            .filter(|c| !c.is_empty())
            .map(|c| tool_bash(wt, &c, env, env_allow))
            .unwrap_or_else(|| Err("bash: missing command".into())),
        "write" => match (req_field("path", "write"), req_field("content", "write")) {
            (Ok(path), Ok(content)) => tool_write(wt, &path, &content),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        "edit" => match (
            req_field("path", "edit"),
            req_field("old", "edit"),
            req_field("new", "edit"),
        ) {
            (Ok(path), Ok(old), Ok(new)) => tool_edit(wt, &path, &old, &new),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => Err(e),
        },
        other => Err(format!("unknown tool: {other}")),
    };
    match result {
        Ok(s) => {
            log(&format!("  -> {s}"));
            s
        }
        Err(e) => {
            log(&format!("  -> ERROR {e}"));
            format!("ERROR: {e}")
        }
    }
}

/// Tool schemas sent as `tools` on every request (static — build once).
fn tool_schemas() -> Value {
    json!([
        {"type":"function","function":{"name":"bash","description":"Run a shell command with the worktree as cwd. Use it for git, builds, tests, and reading files.","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}}},
        {"type":"function","function":{"name":"write","description":"Create or overwrite a file inside the worktree.","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}},
        {"type":"function","function":{"name":"edit","description":"Replace the FIRST UNIQUE occurrence of a string in a worktree file. Errors when the string is missing or appears more than once.","parameters":{"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"}},"required":["path","old","new"]}}}
    ])
}

// ---------------------------------------------------------------------------
// Provider HTTP — one wire format, hand-rolled (ADR-14): every worker target
// speaks OpenAI chat/completions, so a framework would only add tokio.
// ---------------------------------------------------------------------------

/// Role instructions for the builtin loop. Kept minimal on purpose: it is
/// re-sent on every turn and every turn is billed.
const SYSTEM_PROMPT: &str = "You are a coding agent working inside a git worktree. \
Complete the user's task using the bash, write, and edit tools. \
Run the task's acceptance command with bash before you finish; the result is \
merged only if the acceptance command passes. Finish with a short summary.";

fn chat_once(
    agent: &ureq::Agent,
    url: &str,
    key: Option<&str>,
    body: &Value,
) -> Result<Value, String> {
    let mut req = agent.post(url).header("Content-Type", "application/json");
    if let Some(k) = key {
        req = req.header("Authorization", &format!("Bearer {k}"));
    }
    match req.send_json(body) {
        Ok(resp) => {
            // ureq 3.4 `Error::StatusCode` carries no body, so the agent is
            // built with http_status_as_error(false): non-2xx arrives as a
            // response here and the body head is read ourselves (contract:
            // non-2xx -> error carrying status + body head).
            let code = resp.status().as_u16();
            // Cap guards against hostile/runaway bodies; an oversized response
            // exceeds any legitimate chat reply and fails JSON parse with its
            // head in the error.
            let mut text = String::new();
            resp.into_body()
                .into_reader()
                .take(1 << 20)
                .read_to_string(&mut text)
                .map_err(|e| format!("provider read: {e}"))?;
            if !(200..300).contains(&code) {
                return Err(format!("provider http {code}: {}", truncate_chars(&text, 512)));
            }
            serde_json::from_str(&text)
                .map_err(|e| format!("provider json: {e} (head: {})", truncate_chars(&text, 512)))
        }
        Err(ureq::Error::StatusCode(code)) => Err(format!("provider http {code}")),
        Err(e) => Err(format!("provider transport: {e}")),
    }
}

/// Extract (assistant-message, tool_calls) from a chat response.
fn parse_message(resp: &Value) -> Result<(Value, Vec<Value>), String> {
    let msg = resp
        .pointer("/choices/0/message")
        .cloned()
        .ok_or_else(|| "provider response has no choices[0].message".to_string())?;
    let calls = msg
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    Ok((msg, calls))
}

/// A message with no signal: absent/null/empty content AND no usable
/// tool_calls. The provider answering with neither text nor tool calls is a
/// protocol error, not a normal answer.
fn is_empty_response(msg: &Value) -> bool {
    let no_text = !msg
        .get("content")
        .and_then(|c| c.as_str())
        .is_some_and(|c| !c.is_empty());
    let no_calls = !msg
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|c| !c.is_empty());
    no_text && no_calls
}

fn tool_call_fields(call: &Value) -> (String, String, String) {
    let id = call
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = call
        .pointer("/function/name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // `arguments` is a JSON-encoded STRING in the OpenAI format; tolerate
    // providers that inline an object instead.
    let args = match call.pointer("/function/arguments") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "{}".into(),
    };
    (id, name, args)
}

/// Run the full agent loop for one attempt. Logs every round-trip and tool
/// call through `log` (the attempt log). Tool errors feed back to the model
/// as content; only turn-cap / deadline / provider errors stop the loop.
pub fn run(
    worker: &Worker,
    prompt: &str,
    wt: &Path,
    st: &Settings,
    log: &dyn Fn(&str),
) -> Output {
    let max_turns = effective_max_turns(worker, st);
    let deadline = Instant::now() + Duration::from_secs(st.agent_timeout_s);
    let url = format!(
        "{}/chat/completions",
        worker.api_base.as_deref().unwrap_or("").trim_end_matches('/')
    );
    let key = worker
        .api_key_env
        .as_ref()
        .and_then(|k| std::env::var(k).ok())
        .filter(|v| !v.is_empty());

    // Turn 1 MUST be [system, user]: strict gateways (z.ai error 1214,
    // academiccloud "No user query found in messages") reject a system-only
    // messages array. Role instructions live in system; the task itself is
    // the user query.
    let mut messages = vec![
        json!({"role": "system", "content": SYSTEM_PROMPT}),
        json!({"role": "user", "content": prompt}),
    ];
    let (mut total_tokens, mut turns) = (0u64, 0u32);
    let mut final_text = String::new();

    let (env_pairs, env_allow) = crate::execute::agent_env(worker, &|k| std::env::var(k).ok());
    let env_pairs: Vec<(String, String)> =
        env_pairs.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();

    // Loop yields (stop, error) on every exit; no dead sentinel init needed.
    let (stop, error) = loop {
        if turns >= max_turns {
            break (Stop::TurnCap, Some(format!("harness: turn cap {max_turns} reached")));
        }
        // Deadline is enforced at turn boundaries; a single tool call may
        // overshoot by up to TOOL_TIMEOUT.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break (
                Stop::Timeout,
                Some(format!("harness: exceeded agent_timeout_s ({}s)", st.agent_timeout_s)),
            );
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(left))
            .http_status_as_error(false)
            .build()
            .new_agent();
        turns += 1;
        log(&format!("harness: turn {turns}/{max_turns}"));
        let body = json!({
            "model": worker.model,
            "messages": messages,
            "tools": tool_schemas(),
            "tool_choice": "auto",
        });
        let resp = match chat_once(&agent, &url, key.as_deref(), &body) {
            Ok(r) => r,
            Err(e) => {
                log(&format!("harness: {e}"));
                break (Stop::ProviderError, Some(format!("harness: {e}")));
            }
        };
        if let Some(u) = resp
            .pointer("/usage/total_tokens")
            .and_then(|t| t.as_u64())
        {
            total_tokens = total_tokens.saturating_add(u);
        }
        let (msg, calls) = match parse_message(&resp) {
            Ok(m) => m,
            Err(e) => break (Stop::ProviderError, Some(format!("harness: {e}"))),
        };
        final_text = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if calls.is_empty() {
            // Spec §8: no text and no tool_calls is a provider error, not a
            // normal answer.
            if is_empty_response(&msg) {
                log(&format!("harness: provider empty response (no text, no tool_calls)"));
                break (
                    Stop::ProviderError,
                    Some("harness: provider empty response (no text, no tool_calls)".to_string()),
                );
            }
            log(&format!("harness: done after {turns} turn(s), {total_tokens} tokens"));
            break (Stop::Normal, None);
        }
        messages.push(msg);
        for call in &calls {
            let (id, name, args) = tool_call_fields(call);
            log(&format!("tool {name} {}", truncate_chars(&args, 512)));
            let result = run_tool(&name, &args, wt, &env_pairs, &env_allow, log);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result,
            }));
        }
    };

    Output { final_text, total_tokens, turns, stop, error }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt() -> PathBuf {
        // Parallel tests share one process, hence one pid: without a unique
        // suffix they clobber each other's temp dir (remove_dir_all in one
        // test deletes f.txt another test is mid-write).
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "af-harness-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resolve_inside_rejects_absolute_and_escaping() {
        let d = wt();
        assert!(resolve_inside(&d, "/etc/passwd").is_err());
        assert!(resolve_inside(&d, "../outside.txt").is_err());
        assert!(resolve_inside(&d, "a/../../outside.txt").is_err());
        assert!(resolve_inside(&d, "").is_err());
        // Windows drive-relative path: neither absolute nor RootDir, but a
        // Prefix push replaces the base — must be rejected.
        #[cfg(windows)]
        {
            assert!(resolve_inside(&d, "C:foo").is_err());
            assert!(resolve_inside(&d, r"C:\temp\x").is_err());
        }
        // On non-Windows these are ordinary relative names and stay contained.
        #[cfg(not(windows))]
        {
            assert!(resolve_inside(&d, "C:foo").is_ok());
            assert!(resolve_inside(&d, r"C:\temp\x").is_ok());
        }
        assert_eq!(
            resolve_inside(&d, "src/a.txt").unwrap(),
            d.join("src").join("a.txt")
        );
        assert_eq!(
            resolve_inside(&d, "a/../b.txt").unwrap(),
            d.join("b.txt")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn write_creates_file_and_edit_replaces_first_unique() {
        let d = wt();
        tool_write(&d, "f.txt", "alpha beta alpha").unwrap();
        assert_eq!(std::fs::read_to_string(d.join("f.txt")).unwrap(), "alpha beta alpha");
        tool_edit(&d, "f.txt", "beta", "BETA").unwrap();
        assert_eq!(std::fs::read_to_string(d.join("f.txt")).unwrap(), "alpha BETA alpha");
        // not unique now
        let err = tool_edit(&d, "f.txt", "alpha", "x").unwrap_err();
        assert!(err.contains("not unique"), "{err}");
        // not found
        let err = tool_edit(&d, "f.txt", "missing", "x").unwrap_err();
        assert!(err.contains("not found"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn run_tool_reports_errors_as_content_not_panic() {
        let d = wt();
        let logged = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let lg = logged.clone();
        let log = move |s: &str| lg.lock().unwrap().push(s.to_string());
        let out = run_tool("nope", "{}", &d, &[], &[], &log);
        assert!(out.starts_with("ERROR: unknown tool"), "{out}");
        let out = run_tool("edit", "not json", &d, &[], &[], &log);
        assert!(out.contains("ERROR"), "{out}");
        let out = run_tool("write", r#"{"path":"x.txt","content":123}"#, &d, &[], &[], &log);
        assert!(out.starts_with("ERROR:"), "{out}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn truncate_chars_counts_chars_not_bytes() {
        let mut s = String::new();
        // 'é' is 2 bytes in UTF-8; exceed the char budget with multibyte chars.
        while s.chars().count() <= TOOL_OUTPUT_MAX {
            s.push('é');
        }
        assert_eq!(truncate_chars(&s, TOOL_OUTPUT_MAX).chars().count(), TOOL_OUTPUT_MAX);
    }

    #[test]
    fn turn_cap_resolution_prefers_worker_then_env_then_default() {
        let mut w = Worker::default();
        w.max_turns = Some(7);
        let mut st = Settings::from_env();
        st.agent_max_turns = 9;
        assert_eq!(effective_max_turns(&w, &st), 7);
        w.max_turns = None;
        assert_eq!(effective_max_turns(&w, &st), 9);
        st.agent_max_turns = 0;
        assert_eq!(effective_max_turns(&w, &st), DEFAULT_MAX_TURNS);
    }

    #[test]
    fn stop_maps_to_cmd_kinds() {
        assert_eq!(Stop::Normal.cmd_kind(), CmdKind::Success);
        assert_eq!(Stop::TurnCap.cmd_kind(), CmdKind::Stalled);
        assert_eq!(Stop::Timeout.cmd_kind(), CmdKind::Timeout);
        assert_eq!(Stop::ProviderError.cmd_kind(), CmdKind::NonZero);
    }

    #[test]
    fn parse_message_extracts_calls_and_tool_args() {
        let resp: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":null,
               "tool_calls":[{"id":"c1","type":"function",
               "function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]}}],
               "usage":{"total_tokens":15}}"#,
        )
        .unwrap();
        let (msg, calls) = parse_message(&resp).unwrap();
        assert_eq!(calls.len(), 1);
        let (id, name, args) = tool_call_fields(&calls[0]);
        assert_eq!((id.as_str(), name.as_str()), ("c1", "bash"));
        assert!(args.contains("ls"));
        assert!(msg.get("tool_calls").is_some());

        let empty = parse_message(&serde_json::json!({"choices":[{"message":{"content":"hi"}}]})).unwrap();
        assert!(empty.1.is_empty());
    }

    #[test]
    fn parse_message_rejects_malformed() {
        assert!(parse_message(&serde_json::json!({})).is_err());
    }

    #[test]
    fn is_empty_response_classifies_no_signal_messages() {
        assert!(is_empty_response(&serde_json::json!({ "content": null })));
        assert!(is_empty_response(&serde_json::json!({})));
        assert!(is_empty_response(&serde_json::json!({ "content": "" })));
        assert!(!is_empty_response(&serde_json::json!({ "content": "hi" })));
        assert!(!is_empty_response(&serde_json::json!(
            { "tool_calls": [{ "id": "c1", "function": { "name": "bash", "arguments": "{}" } }] }
        )));
    }
}
