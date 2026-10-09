# Native Rust Agent Harness (`cli: "builtin"`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development (recommended) or executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a native in-process agent loop to `af` — a worker with `"cli": "builtin"` talks to any OpenAI-compatible `/chat/completions` endpoint directly and executes three tools (`bash`, `write`, `edit`) in the worktree, with no external agent CLI installed.

**Architecture:** One new module `src/harness.rs` (loop + tools + HTTP), wired into `execute_attempt` in `src/execute.rs` as a third dispatch shape next to `command` and legacy argv. Native failures take an early-return branch that reuses the existing preserve-and-note/cleanup machinery. Everything downstream (work integrity, scope, gate, merge, receipts, router) is untouched. Spec: `docs/superpowers/specs/2026-10-09-native-rust-harness-design.md`.

**Tech Stack:** Rust (edition 2021), `serde`/`serde_json` (already deps), `ureq` 3 with rustls (new dep, sync — no tokio, ADR-2), existing `subprocess::run_with_stall` for tool execution, existing `gate::shell()` for platform shell.

**Conventions for this codebase:** tests live in `#[cfg(test)] mod tests` inside the module for unit tests and `tests/*.rs` integration binaries; gates and the bash tool use the platform shell via `crate::gate::shell()`; comments explain *why*. Run tests with `cargo test` from the repo root.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `Cargo.toml` | modify | add `ureq` |
| `src/lib.rs` | modify | `pub mod harness;` |
| `src/config.rs` | modify | `Worker.max_turns`, `Settings.agent_max_turns`, `validate()` builtin checks |
| `src/harness.rs` | create | the whole harness: types, HTTP, loop, tools |
| `src/execute.rs` | modify | dispatch branch in `execute_attempt` |
| `tests/contract_config.rs` | modify | builtin validation contract tests |
| `tests/e2e_native_harness.rs` | create | stub-HTTP-server E2E (happy / turn-cap / provider-error) |
| `docs/arc42/09-architecture-decisions.md` | modify | ADR-11 |
| `docs/arc42/12-glossary.md` | modify | Native harness entry |
| `docs/arc42/02-constraints.md` | modify | constraint wording exception |
| `README.md` | modify | worker schema row + standalone section |

---

### Task 1: Config surface — `max_turns`, `agent_max_turns`, builtin validation

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Modify: `src/config.rs`
- Modify: `tests/contract_config.rs`

- [ ] **Step 1: Add the failing contract tests**

Append to the end of `tests/contract_config.rs` (before the final `}` only if the file wraps tests in a module — it does not; append at top level):

```rust
// spec: docs/superpowers/specs/2026-10-09-native-rust-harness-design.md
// `cli: "builtin"` selects the native harness. Config-time validation must
// catch unusable combos BEFORE any agent is paid.
#[test]
fn builtin_worker_requires_api_base() {
    let err = config::load_workers_str(
        r#"{ "workers": [ { "name": "nat", "provider": "p", "model": "m",
             "cli": "builtin", "enabled": true } ] }"#,
    )
    .unwrap_err();
    assert!(err.contains("api_base"), "err: {err}");
}

#[test]
fn builtin_worker_rejects_command_template() {
    let err = config::load_workers_str(
        r#"{ "workers": [ { "name": "nat", "provider": "p", "model": "m",
             "api_base": "http://x/v1", "cli": "builtin", "command": "x {prompt}",
             "enabled": true } ] }"#,
    )
    .unwrap_err();
    assert!(err.contains("command"), "err: {err}");
}

#[test]
fn builtin_worker_rejects_zero_max_turns() {
    let err = config::load_workers_str(
        r#"{ "workers": [ { "name": "nat", "provider": "p", "model": "m",
             "api_base": "http://x/v1", "cli": "builtin", "max_turns": 0,
             "enabled": true } ] }"#,
    )
    .unwrap_err();
    assert!(err.contains("max_turns"), "err: {err}");
}

#[test]
fn builtin_worker_parses_with_defaults() {
    let cfg = config::load_workers_str(
        r#"{ "workers": [ { "name": "nat", "provider": "p", "model": "m",
             "api_base": "http://x/v1", "cli": "builtin", "enabled": true } ] }"#,
    )
    .unwrap();
    assert_eq!(cfg.workers[0].max_turns, None);
}

#[test]
fn cli_worker_max_turns_is_optional_and_parsed() {
    let cfg = config::load_workers_str(
        r#"{ "workers": [ { "name": "w", "provider": "p", "model": "m",
             "enabled": true, "cli": "pi", "max_turns": 7 } ] }"#,
    )
    .unwrap();
    assert_eq!(cfg.workers[0].max_turns, Some(7));
}
```

Note: check how existing tests in `tests/contract_config.rs` build configs (they may call `config::load(tasks_path, workers_path)` with files rather than a string helper). If no `load_workers_str` helper exists in `config.rs`, add it in Step 3 and use it here; if the file's tests all write temp files, mirror that pattern instead — either way the five assertions above are the contract.

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo test --test contract_config 2>&1 | tail -20`
Expected: compile errors — `load_workers_str` and `max_turns` do not exist yet.

- [ ] **Step 3: Add ureq, the module, and the config fields**

`Cargo.toml` — inside `[dependencies]`, add:

```toml
ureq = { version = "3", default-features = false, features = ["rustls", "json"] }
```

`src/lib.rs` — add to the module list (alphabetical, after `pub mod gate;`):

```rust
pub mod harness;
```

`src/config.rs` — in `struct Worker`, after the `price_per_mtok_usd` field, add:

```rust
    /// Turn cap for the native harness (`cli: "builtin"`): the loop stops
    /// (and the attempt fails, archive-preserving) after this many LLM
    /// round-trips. Absent = `TF_AGENT_MAX_TURNS` if set (>0), else 32.
    /// Ignored for CLI workers (their flags live in `args`).
    #[serde(default)]
    pub max_turns: Option<u32>,
```

In `impl Default for Worker`, add to the initializer:

```rust
            max_turns: None,
```

In `struct Settings`, after `agent_stall_s`, add:

```rust
    /// Global native-harness turn cap (`TF_AGENT_MAX_TURNS`). 0 = no global
    /// opinion; the worker's `max_turns` wins, else 32.
    pub agent_max_turns: u32,
```

In `impl Settings { pub fn from_env()`, add to the initializer (after `agent_stall_s`):

```rust
            agent_max_turns: env_or_int("TF_AGENT_MAX_TURNS", 0) as u32,
```

Add this helper near `load_repos` (public so contract tests and future callers can load a workers string without temp files):

```rust
/// Load a workers file from an in-memory JSON string. Test seam + programmatic
/// use; same validation as the file path.
pub fn load_workers_str(json: &str) -> Result<Config, String> {
    #[derive(Deserialize)]
    struct WorkersFile {
        #[serde(default)]
        _meta: BTreeMap<String, serde_json::Value>,
        #[serde(default)]
        defaults: WorkerDefaults,
        #[serde(default)]
        workers: Vec<Worker>,
    }
    let wf: WorkersFile =
        serde_json::from_str(json).map_err(|e| format!("parse workers: {e}"))?;
    let (workers, warnings) = validate(&[], &wf.workers)?;
    Ok(Config {
        tasks: Vec::new(),
        by_id: BTreeMap::new(),
        workers,
        defaults: wf.defaults,
        warnings,
        repos: BTreeMap::new(),
    })
}
```

IMPORTANT: check `Config`'s real field list first (`rg -n "pub struct Config" -A 12 src/config.rs`) and make the initializer match it exactly — the shape above is the expected one but the struct is the source of truth. If `validate` requires at least one task or its signature differs, adapt: either relax `validate` for the empty-tasks case (workers-only validation is legitimate) or have `load_workers_str` skip task checks by calling the worker-validation part only. Also check how the existing `load` builds `Config` and mirror it.

In `fn validate`, after the duplicate-id loop and BEFORE the task warnings loop, add:

```rust
    // Native harness (`cli: "builtin"`) config-time checks: unusable combos
    // must fail here, before any agent is paid.
    for w in workers {
        if w.cli != "builtin" {
            continue;
        }
        if w.command.is_some() {
            return Err(format!(
                "worker \"{}\": command is not supported with cli \"builtin\"",
                w.name
            ));
        }
        if w.api_base.as_deref().map_or(true, |b| b.trim().is_empty()) {
            return Err(format!(
                "worker \"{}\": cli \"builtin\" requires api_base",
                w.name
            ));
        }
        if w.max_turns == Some(0) {
            return Err(format!(
                "worker \"{}\": max_turns must be >= 1",
                w.name
            ));
        }
    }
```

- [ ] **Step 4: Fix compile errors from the new Settings field**

Run: `cargo test --no-run 2>&1 | grep -E "^error" | head`
Expected: errors about missing field `agent_max_turns` in `Settings` struct literals (e.g. the fixture in `tests/e2e.rs` and test literals in `src/execute.rs`). Add `agent_max_turns: 0,` to each named literal (after `poll_secs`).

- [ ] **Step 5: Create a stub harness so the build is green**

`src/harness.rs` (temporary — Tasks 2–3 replace the internals; the module must exist for `pub mod harness` to compile):

```rust
//! Native agent harness (`cli: "builtin"`): af owns the LLM loop.
//! ADR-11 amends ADR-1 narrowly: the orchestrator may originate an agent
//! loop, but only inside this worker mode. CLI workers keep the ADR-1
//! subprocess contract unchanged.

//! Implemented in Tasks 2-3 of docs/superpowers/plans/2026-10-09-native-rust-harness.md.
```

Run: `cargo test --test contract_config 2>&1 | tail -5`
Expected: 5 new tests pass (or fail only on `load_workers_str` behavior — fix per the IMPORTANT note above), all pre-existing tests in that binary pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/config.rs src/harness.rs tests/contract_config.rs tests/e2e.rs src/execute.rs
git commit -m "feat(config): cli \"builtin\" worker mode + max_turns, load_workers_str, validation"
```

---

### Task 2: `harness.rs` — tools (write/edit/bash) with guards, unit-tested

**Files:**
- Create/replace: `src/harness.rs`

- [ ] **Step 1: Write the module skeleton, types, and tool executors**

Replace `src/harness.rs` contents with:

```rust
//! Native agent harness (`cli: "builtin"`): af owns the LLM loop.
//! ADR-11 amends ADR-1 narrowly: the orchestrator may originate an agent
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
    if rel.is_absolute() {
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
            other => norm.push(other.as_os_str()),
        }
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
    let field = |k: &str| {
        parsed
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default()
    };
    let result = match name {
        "bash" => match field("command") {
            c if c.is_empty() => Err("bash: missing command".into()),
            c => tool_bash(wt, &c, env, env_allow),
        },
        "write" => tool_write(wt, &field("path"), &field("content")),
        "edit" => tool_edit(wt, &field("path"), &field("old"), &field("new")),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wt() -> PathBuf {
        let d = std::env::temp_dir().join(format!("af-harness-{}", std::process::id()));
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
        let _ = std::fs::remove_dir_all(&d);
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
}
```

- [ ] **Step 2: Run the unit tests**

Run: `cargo test --lib harness 2>&1 | tail -10`
Expected: all new harness unit tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/harness.rs
git commit -m "feat(harness): tool surface (bash/write/edit) with worktree containment + unit tests"
```

---

### Task 3: `harness.rs` — chat loop (HTTP, usage, caps)

**Files:**
- Modify: `src/harness.rs`

- [ ] **Step 1: Add the HTTP client function and the loop**

Append to `src/harness.rs` (above the `#[cfg(test)] mod tests` block):

```rust
// ---------------------------------------------------------------------------
// Provider HTTP — one wire format, hand-rolled (ADR-11): every worker target
// speaks OpenAI chat/completions, so a framework would only add tokio.
// ---------------------------------------------------------------------------

fn text_head(s: &str, max: usize) -> String {
    truncate_chars(s, max)
}

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
            let text = resp
                .into_body()
                .read_to_string()
                .map_err(|e| format!("provider read: {e}"))?;
            serde_json::from_str(&text)
                .map_err(|e| format!("provider json: {e} (head: {})", text_head(&text, 512)))
        }
        Err(ureq::Error::StatusCode(code, resp)) => {
            let text = resp.into_body().read_to_string().unwrap_or_default();
            Err(format!("provider http {code}: {}", text_head(&text, 512)))
        }
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

    let mut messages = vec![json!({"role": "system", "content": prompt})];
    let (mut total_tokens, mut turns) = (0u64, 0u32);
    let (mut final_text, mut stop, mut error) =
        (String::new(), Stop::ProviderError, Some("loop exited abnormally".into()));

    let (env_pairs, env_allow) = crate::execute::agent_env(worker, &|k| std::env::var(k).ok());
    let env_pairs: Vec<(String, String)> =
        env_pairs.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();

    loop {
        if turns >= max_turns {
            stop = Stop::TurnCap;
            error = Some(format!("harness: turn cap {max_turns} reached"));
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            stop = Stop::Timeout;
            error = Some(format!("harness: exceeded agent_timeout_s ({}s)", st.agent_timeout_s));
            break;
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(left))
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
                stop = Stop::ProviderError;
                error = Some(format!("harness: {e}"));
                break;
            }
        };
        if let Some(u) = resp
            .pointer("/usage/total_tokens")
            .and_then(|t| t.as_u64())
        {
            total_tokens += u;
        }
        let (msg, calls) = match parse_message(&resp) {
            Ok(m) => m,
            Err(e) => {
                stop = Stop::ProviderError;
                error = Some(format!("harness: {e}"));
                break;
            }
        };
        final_text = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        if calls.is_empty() {
            stop = Stop::Normal;
            error = None;
            log(&format!("harness: done after {turns} turn(s), {total_tokens} tokens"));
            break;
        }
        messages.push(msg);
        for call in &calls {
            let (id, name, args) = tool_call_fields(call);
            log(&format!("tool {name} {args}"));
            let result = run_tool(&name, &args, wt, &env_pairs, &env_allow, log);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result,
            }));
        }
    }

    Output { final_text, total_tokens, turns, stop, error }
}
```

Also add to `src/harness.rs`'s test module:

```rust
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
```

Add `use std::io::Read as _;` at the top of the file (needed for `read_to_string` on the ureq body) — extend the existing use block:

```rust
use std::io::Read as _;
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --lib harness 2>&1 | tail -10`
Expected: all harness unit tests PASS. If `ureq::Error::StatusCode` does not match (API drift), check `cargo doc -p ureq --no-deps --open` and adapt the match arms — the semantic contract is: non-2xx status → error carrying status + body head; transport error → error; 2xx → parsed JSON.

- [ ] **Step 3: Commit**

```bash
git add src/harness.rs
git commit -m "feat(harness): chat/completions loop with usage accumulation, turn + deadline caps"
```

---

### Task 4: Wire into `execute_attempt`

**Files:**
- Modify: `src/execute.rs` (agent dispatch block, around lines 277–320)

- [ ] **Step 1: Add the native dispatch branch**

In `src/execute.rs`, locate this exact block inside `execute_attempt`:

```rust
        let using_command = worker.command.is_some();
        let (agent_cmd, agent_args) = match &worker.command {
            Some(tpl) => command_argv(tpl, &prompt_path),
            None => {
                let argv = spawn_argv(&ctx.st, worker, &prompt_path);
                let (c, a) = argv.split_first().unwrap();
                (c.clone(), a.to_vec())
            }
        };
```

Replace it with:

```rust
        let using_command = worker.command.is_some();
        let native = worker.cli == crate::harness::BUILTIN;
        let (agent_cmd, agent_args) = match &worker.command {
            Some(tpl) => command_argv(tpl, &prompt_path),
            None => {
                let argv = spawn_argv(&ctx.st, worker, &prompt_path);
                let (c, a) = argv.split_first().unwrap();
                (c.clone(), a.to_vec())
            }
        };
```

Then locate this exact block (the stall watchdog + agent spawn):

```rust
        append("-- agent --");
        // Stall watchdog (agent CLI ONLY — git and gate calls stay on plain
        // `run`: a silent gate is not necessarily a stalled one, and their
        // timeout contracts are pinned by tests): 0 = disabled, exactly the
        // legacy total-timeout-only behaviour.
        let stall = if ctx.st.agent_stall_s == 0 {
            None
        } else {
            Some(Duration::from_secs(ctx.st.agent_stall_s))
        };
        let agent_out = crate::subprocess::run_with_stall(
            &agent_cmd,
            &agent_args,
            Some(&wt_path),
            &env_pairs,
            env_mode,
            Duration::from_secs(ctx.st.agent_timeout_s),
            stall,
        );
```

Replace with:

```rust
        append("-- agent --");
        // Native harness (ADR-11): the loop runs INSIDE af; every tool call
        // is logged by the harness itself. Its outcome synthesizes the SAME
        // CmdKind contract as a CLI child, so all downstream handling
        // (work integrity, gate, merge, retry memory) is byte-identical.
        let agent_out = if native {
            let out = crate::harness::run(worker, &prompt, &wt_path, &ctx.st, &append);
            append(&format!(
                "harness: stop={:?} turns={} tokens={}",
                out.stop, out.turns, out.total_tokens
            ));
            spend.tokens = Some(out.total_tokens);
            crate::subprocess::CmdOut {
                kind: out.stop.cmd_kind(),
                code: if out.stop == crate::harness::Stop::Normal { Some(0) } else { None },
                stdout: out.final_text,
                stderr: out.error.unwrap_or_default(),
            }
        } else {
            // Stall watchdog (agent CLI ONLY — git and gate calls stay on plain
            // `run`: a silent gate is not necessarily a stalled one, and their
            // timeout contracts are pinned by tests): 0 = disabled, exactly the
            // legacy total-timeout-only behaviour.
            let stall = if ctx.st.agent_stall_s == 0 {
                None
            } else {
                Some(Duration::from_secs(ctx.st.agent_stall_s))
            };
            crate::subprocess::run_with_stall(
                &agent_cmd,
                &agent_args,
                Some(&wt_path),
                &env_pairs,
                env_mode,
                Duration::from_secs(ctx.st.agent_timeout_s),
                stall,
            )
        };
        // Native failures return early with the PRECISE reason (`harness: …`)
        // on the receipt, reusing the same preserve-and-note machinery: the
        // paid-for work lands on the archive branch exactly like a CLI
        // failure. The legacy `agent exited …` message below stays
        // byte-identical for CLI workers.
        if native && !agent_out.passed() {
            let kept = preserve_and_note(&repo, &wt, id, attempt, &mut append);
            cleanup(&repo, &wt);
            let reason = if agent_out.stderr.is_empty() {
                format!("harness failed ({:?})", agent_out.kind)
            } else {
                agent_out.stderr.clone()
            };
            return (Outcome::Failed(format!("{reason}{kept}")), spend);
        }
```

Note: `env_mode` and the `using_command` flag remain used by the CLI branch (env_mode is computed above the dispatch; if the compiler flags it as unused in the native path, it is still consumed by the else branch — no change needed). The `native` guard uses `preserve_and_note`/`cleanup` which are in scope in `execute_attempt` (they are called by the legacy failure path in the same function).

- [ ] **Step 2: Build and run the full suite**

Run: `cargo test 2>&1 | tail -15`
Expected: everything passes, no new warnings. The existing 340+ tests are untouched (default workers are CLI workers).

- [ ] **Step 3: Commit**

```bash
git add src/execute.rs
git commit -m "feat(execute): dispatch cli=builtin workers through the native harness"
```

---

### Task 5: E2E — stub HTTP server, scratch repo, real `af run`

**Files:**
- Create: `tests/e2e_native_harness.rs`

- [ ] **Step 1: Write the test file**

```rust
//! E2E: native harness (`cli: "builtin"`) against a local stub HTTP server
//! speaking canned OpenAI chat/completions responses + a scratch git repo.
//! No live network, no external agent CLI — deterministic CI.

use agentflow::config::{self, Settings};
use agentflow::run::{self, RunOptions};
use agentflow::state::{Store, TaskState};
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
}

impl StubLlm {
    fn spawn(responses: Vec<(u16, String)>) -> StubLlm {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let queue: Arc<Mutex<VecDeque<(u16, String)>>> =
            Arc::new(Mutex::new(responses.into()));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let next = queue.lock().unwrap().pop_front();
                let (code, body) = next.unwrap_or((200, "{}".to_string()));
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf); // request head; not parsed
                let reason = if code == 200 { "OK" } else { "Internal Server Error" };
                let resp = format!(
                    "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        StubLlm { url: format!("http://{addr}/v1") }
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
    let stub = StubLlm::spawn(vec![
        bash_turn("git add A.txt && git commit -m work"),
        final_turn("done"),
    ]);
    let f = fixture(&task_json(&gate_cmd("A.txt")), &native_worker_json(&stub.url, 1, 8));
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "run should exit 0 (all done)");
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
    let stub = StubLlm::spawn((0..8).map(|_| bash_turn("echo hi")).collect());
    let f = fixture(&task_json(&gate_cmd("A.txt")), &native_worker_json(&stub.url, 1, 2));
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    assert_eq!(code, 0, "task terminal (failed), run still exits 0");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    let r = receipts.iter().find(|r| r.task == "A").expect("receipt");
    assert!(r.error.as_deref().unwrap_or("").contains("turn cap 2"), "{:?}", r.error);
    assert_eq!(r.tokens, Some(15), "only turn 1 was paid before the cap");
}

#[test]
fn native_harness_provider_error_fails_with_reason() {
    let stub = StubLlm::spawn(vec![(500, r#"{"error":"boom"}"#.to_string())]);
    let f = fixture(&task_json(&gate_cmd("A.txt")), &native_worker_json(&stub.url, 1, 8));
    let _ = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);
    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    let r = receipts.iter().find(|r| r.task == "A").expect("receipt");
    let err = r.error.as_deref().unwrap_or("");
    assert!(err.contains("harness: provider http 500"), "{err}");
    assert_eq!(r.tokens, Some(0), "no paid round-trip succeeded");
}
```

- [ ] **Step 2: Run the E2E tests**

Run: `cargo test --test e2e_native_harness 2>&1 | tail -15`
Expected: 3 tests PASS. Debugging hints: on failure, dump the attempt log (`f.st.state_dir/logs/A.log`) — it contains every harness turn + tool call, which is the whole point of native mode. If the stub server sees more requests than queued (retries at the `run_loop` level), check `max_attempts` in the worker JSON is 1.

- [ ] **Step 3: Full suite + commit**

Run: `cargo test 2>&1 | tail -5`
Expected: all green.

```bash
git add tests/e2e_native_harness.rs
git commit -m "test(e2e): native harness happy path, turn cap, provider error via stub LLM"
```

---

### Task 6: Docs (ADR-11, glossary, constraints, README) + final verification

**Files:**
- Modify: `docs/arc42/09-architecture-decisions.md`
- Modify: `docs/arc42/12-glossary.md`
- Modify: `docs/arc42/02-constraints.md`
- Modify: `README.md`

- [ ] **Step 1: ADR-11**

In `docs/arc42/09-architecture-decisions.md`, append a row to the ADR table (match the existing table's column format exactly — read the table header first):

```markdown
| **ADR-11** | accepted | **Native harness as an opt-in worker mode.** ADR-1's "no agent loop" is amended narrowly: a worker with `cli: "builtin"` runs the OpenAI chat/completions loop inside af (`src/harness.rs`) with three tools (bash/write/edit) under the standard env allowlist. Motivation: single static binary for fleet hosts + first-class token/turn ownership. CLI workers remain the default contract; `pi`/`opencode` behavior is unchanged. | `src/harness.rs`, `execute_attempt` | 01, 02, 05 |
```

- [ ] **Step 2: Glossary + constraints**

In `docs/arc42/12-glossary.md`, add after the **Agent** row:

```markdown
| **Native harness** | In-process agent loop (`cli: "builtin"`, ADR-11): af calls the OpenAI-compatible chat endpoint directly and executes `bash`/`write`/`edit` tool calls in the worktree under the standard env allowlist. No external agent CLI needed. |
```

In `docs/arc42/02-constraints.md`, change line 49 from:

```markdown
- `af` orchestrates; it does not originate: no agent loop, no tool calling, no embedded git server.
```

to:

```markdown
- `af` orchestrates; it does not originate: no agent loop, no tool calling, no embedded git server. **Exception (ADR-11):** workers with `cli: "builtin"` run a minimal in-process loop; the subprocess contract is the default.
```

- [ ] **Step 3: README**

In `README.md`, in the workers configuration section (find the table or list documenting worker fields — `rg -n "api_key_env|\"cli\"" README.md`), add a row/bullet:

```markdown
- `cli`: agent CLI binary (default `pi`). **`"builtin"`** runs af's native in-process harness instead — no external agent CLI needed; the worker then requires `api_base` (OpenAI-compatible `/chat/completions`) and optional `api_key_env`, supports `max_turns` (default 32, `TF_AGENT_MAX_TURNS`), and reports measured token usage on every receipt. Tools: `bash`, `write`, `edit` in the worktree.
```

- [ ] **Step 4: Final verification**

Run: `cargo test 2>&1 | tail -5 && cargo clippy --all-targets 2>&1 | tail -5`
Expected: all tests pass; no new clippy warnings. Run `cargo build --release` once to confirm the release binary builds (this is the fleet artifact).

```bash
git add docs/arc42/09-architecture-decisions.md docs/arc42/12-glossary.md docs/arc42/02-constraints.md README.md
git commit -m "docs: ADR-11 native harness, glossary, constraints exception, README worker docs"
```

---

## Self-Review (done at plan time)

- **Spec coverage:** config surface (§4 → Task 1), tools + sandbox (§6 → Task 2), wire protocol + loop + usage (§5, §7 → Task 3), failure semantics (§3, §8 → Task 4), receipts/tokens (§7 → Task 3+5), tests (§9 → Tasks 1, 2, 3, 5), docs (§10 → Task 6), dep delta (§12 → Task 1). Non-goals (§11): nothing added beyond the three tools.
- **Placeholders:** none — every code step contains complete code; the two "check the real struct first" notes (Config fields in Task 1, ureq error API in Task 3) are verification steps with fallback code, not gaps.
- **Type consistency:** `Stop::cmd_kind()` (Task 2) used by Task 4; `harness::run(worker, prompt, wt, st, log)` signature consistent between Tasks 3 and 4; `Worker.max_turns: Option<u32>` consistent between Tasks 1, 2, 3; `Settings.agent_max_turns: u32` between Tasks 1, 2, 5.
