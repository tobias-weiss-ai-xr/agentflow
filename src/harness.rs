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
