//! Execute: one task attempt — worktree → prompt → agent → gate → merge
//! (spec: lifecycle). Retries & status transitions are decided by the run
//! loop (`run.rs`); this module performs a single attempt and reports the
//! outcome.

use crate::config::{Config, Settings, Worker};
use crate::state::{now_ts, AttemptPhase, Receipt, Store};
use crate::{gate, worktree};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Merged,
    Failed(String),
}

#[derive(Clone)]
pub struct ExecCtx {
    pub cfg: Config,
    pub st: Settings,
    pub store: Store,
    pub merge_locks: worktree::MergeLocks,
}

/// System env keys the agent child needs to function (PATH, temp dirs, OS
/// loader bits). Everything else in the orchestrator env is withheld.
const AGENT_ENV_BASE: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMFILES",
    // Commit identity for the agent's own git commits (standard git env).
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
];

/// Sandbox policy for the agent child (ADR-10): env allowlist that exposes
/// ONLY this worker's `api_key_env` (+ optional passthrough), plus git
/// hygiene pairs (no credential prompts/helpers). `lookup` abstracts env
/// access for testing.
pub fn agent_env(
    worker: &Worker,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut allow: Vec<String> = AGENT_ENV_BASE.iter().map(|s| s.to_string()).collect();
    if let Some(k) = &worker.api_key_env {
        allow.push(k.clone());
    }
    if let Some(p) = lookup("TF_AGENT_ENV_PASSTHROUGH") {
        for k in p.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            allow.push(k.to_string());
        }
    }
    let pairs = vec![
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        (
            "GIT_CONFIG_KEY_0".to_string(),
            "credential.helper".to_string(),
        ),
        ("GIT_CONFIG_VALUE_0".to_string(), String::new()),
    ];
    (pairs, allow)
}

pub fn execute_task(
    ctx: &ExecCtx,
    worker: &Worker,
    id: &str,
    attempt: u32,
    log_path: &Path,
) -> Outcome {
    let start = std::time::Instant::now();
    let (outcome, tokens) = execute_attempt(ctx, worker, id, attempt, log_path);
    // Receipt for EVERY attempt (merged + failed) — routing/cost substrate.
    let outcome_name = match outcome {
        Outcome::Merged => "merged",
        Outcome::Failed(_) => "failed",
    };
    let error = match &outcome {
        Outcome::Failed(e) => Some(e.lines().next().unwrap_or("").chars().take(200).collect()),
        Outcome::Merged => None,
    };
    let _ = ctx.store.append_receipt(&Receipt {
        task: id.to_string(),
        attempt,
        worker: worker.name.clone(),
        model: worker.model.clone(),
        wall_clock_s: start.elapsed().as_secs_f64(),
        tokens,
        ts: now_ts(),
        outcome: outcome_name.to_string(),
        error,
    });
    outcome
}

/// Episodic retry memory (ADR-13): prior failed attempts of this task, as a
/// prompt block. None for first attempts. Pure for testing.
/// ponytail: per-task only — cross-task episode recall needs task types.
fn failure_context(receipts: &[Receipt], task_id: &str, attempt: u32) -> Option<String> {
    if attempt < 2 {
        return None;
    }
    let prior: Vec<&Receipt> = receipts
        .iter()
        .filter(|r| r.task == task_id && r.attempt < attempt && r.outcome == "failed")
        .collect();
    if prior.is_empty() {
        return None;
    }
    let mut out =
        String::from("\n\n## Previous attempts on this task (avoid repeating these failures)\n");
    for r in prior {
        let why = r.error.as_deref().unwrap_or("(unknown failure)");
        out.push_str(&format!("- attempt {}: {}\n", r.attempt, why));
    }
    Some(out)
}

/// Effect-sandwich journal (pi-durable): persist one attempt-phase boundary
/// for `id` so a crashed run can resume without re-running the expensive,
/// non-replayable agent step.
///
/// Writes ONLY this task's entry via load-modify-save on the Store (atomic
/// temp+rename). The orchestrator process is the single writer (per-state-dir
/// lock), so the small race window is only against sibling attempt threads'
/// boundary writes and the run loop's whole-map saves — both atomic renames,
/// so a losing write is superseded whole, never torn. Worst case one stale
/// boundary survives a crash, which `resume_action` maps to a safe re-run
/// (e.g. gate re-runs instead of merge-only). Never the reverse.
fn record_phase(ctx: &ExecCtx, id: &str, phase: AttemptPhase) {
    let mut m = ctx.store.load();
    m.entry(id.to_string()).or_default().phase = Some(phase);
    let _ = ctx.store.save(&m);
}

/// One attempt's outcome plus the token usage captured from the agent
/// CLI's JSON transcript (`None` in text mode, on gate-only retries, and
/// whenever the stream yielded no usage — missing telemetry never fails
/// an attempt).
fn execute_attempt(
    ctx: &ExecCtx,
    worker: &Worker,
    id: &str,
    attempt: u32,
    log_path: &Path,
) -> (Outcome, Option<u64>) {
    let task = match ctx.cfg.by_id.get(id) {
        Some(t) => t.clone(),
        None => return (Outcome::Failed(format!("unknown task {id}")), None),
    };

    // Multi-repo (ADR-11): worktree, branch, and merge target the task's repo.
    let repo = ctx.cfg.repo_dir_for(&task, &ctx.st.repo_dir);
    // Cost fix (gate flakes must not discard paid agent work): when the
    // agent's committed work is still durable on the attempt branch — a
    // previous attempt of this task failed ONLY at the gate — attach to
    // that branch and re-run just the gate. The agent is never invoked
    // again for work it already committed.
    let (wt, reused) =
        match materialize_worktree(&repo, &ctx.st.worktree_root, id, &ctx.st.branch_prefix) {
            Ok(w) => w,
            Err(e) => return (Outcome::Failed(e), None),
        };
    let wt_path = wt.path.clone();
    // Token usage lifted out of the agent's JSON transcript, when the
    // worker opted into json output mode. Every failure path that did not
    // run the agent reports `None`.
    let mut tokens: Option<u64> = None;
    // Effect sandwich, boundary 1 (commit intent BEFORE the effect): the
    // attempt is live — worktree ready, agent about to spawn. A crash from
    // here resumes as RerunAgent (the agent's outcome is not durable yet).
    record_phase(ctx, id, AttemptPhase::Spawned);

    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|e| e.to_string());
    let mut append = |s: &str| {
        if let Ok(ref mut f) = log {
            let _ = writeln!(f, "{s}");
        }
    };
    append(&format!(
        "== attempt {attempt} on worker {} ({}) ==",
        worker.name, worker.model
    ));

    if reused {
        // Gate-only retry: the agent's committed work on the attempt branch
        // IS the durable AgentDone evidence (journaled below) — the agent
        // step is skipped entirely so a gate flake never re-pays for it.
        append("-- gate-only retry on committed agent work --");
    } else {
        // 1) Render + write prompt.
        let prompt = {
            let context = failure_context(&ctx.store.load_receipts(), id, attempt);
            render_prompt(&ctx.st, &task, worker, context.as_deref())
        };
        // Absolute: the prompt path crosses the process boundary into the
        // agent's cwd (a worktree) — a relative state_dir would point at a
        // path that doesn't exist there (gitignored state, e.g.).
        let prompt_path = std::path::absolute(ctx.store.prompt_dir().join(format!("{id}.md")))
            .unwrap_or_else(|_| ctx.store.prompt_dir().join(format!("{id}.md")));
        let _ = fs::create_dir_all(ctx.store.prompt_dir());
        if let Err(e) = fs::write(&prompt_path, &prompt) {
            cleanup(&repo, &wt);
            return (Outcome::Failed(format!("cannot write prompt: {e}")), None);
        }

        // 2) Agent CLI (external, OpenAI-compatible): --provider P --model M -p @file
        let argv = spawn_argv(&ctx.st, worker, &prompt_path);
        let (agent_cmd, agent_args) = argv.split_first().unwrap();
        let (env_pairs, env_allow) = agent_env(worker, &|k| std::env::var(k).ok());
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
            agent_cmd,
            agent_args,
            Some(&wt_path),
            &env_pairs,
            crate::subprocess::EnvMode::Allowlist(env_allow),
            Duration::from_secs(ctx.st.agent_timeout_s),
            stall,
        );
        // JSON output mode (r9-token-capture): the stream is telemetry,
        // not the log — render it for the human and lift the token usage
        // out of it. Text mode keeps the raw combined stream, byte-identical
        // to the legacy behaviour.
        let body = if worker.output == "json" {
            let t = crate::transcript::parse(&agent_out.stdout);
            tokens = t.usage.map(|u| u.total_tokens);
            json_log_body(&t, &agent_out.stdout, &agent_out.stderr)
        } else {
            agent_out.combined()
        };
        if !body.is_empty() {
            append(&body);
        }
        if !agent_out.passed() {
            cleanup(&repo, &wt);
            // Same single failure path, distinct diagnosis: a stall names
            // the configured window (it is a different disease than "ran
            // out of total time"), every other failure keeps the
            // historical "agent exited …" shape that callers match on.
            let reason = if agent_out.kind == crate::subprocess::CmdKind::Stalled {
                format!(
                    "agent stalled: produced no output for {}s (stall window, TF_AGENT_STALL_S={}); killed before agent_timeout_s={}",
                    ctx.st.agent_stall_s,
                    ctx.st.agent_stall_s,
                    ctx.st.agent_timeout_s
                )
            } else {
                format!(
                    "agent exited {:?} (code {})",
                    agent_out.kind,
                    agent_out
                        .code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "-".into())
                )
            };
            // The tokens were still spent (and captured) even though the
            // attempt failed — the receipt is the cost ledger.
            return (Outcome::Failed(reason), tokens);
        }
    }

    // 2b) Scope enforcement: the prompt's "do not touch files outside the
    // allowed scope" is now an enforced contract. The agent's committed
    // change must stay within `task.scope` (empty scope means any file).
    // Checked BEFORE the durable AgentDone boundary and BEFORE merge, so a
    // violating change can never reach the base branch AND a crash after the
    // check cannot resume into a merge that skips it. Runs on BOTH paths —
    // reused work must re-prove its scope before it can merge.
    {
        let base_branch = match worktree::current_branch(&repo) {
            Ok(b) => b,
            Err(e) => {
                cleanup(&repo, &wt);
                return (Outcome::Failed(e), None);
            }
        };
        let changed = match changed_paths(&wt_path, &base_branch) {
            Ok(c) => c,
            Err(e) => {
                cleanup(&repo, &wt);
                return (Outcome::Failed(e), None);
            }
        };
        let violations = scope_violations(&changed, &task.scope);
        if !violations.is_empty() {
            append(&format!(
                "-- scope violation: {} (allowed: {})",
                violations.join(", "),
                task.scope.join(", ")
            ));
            cleanup(&repo, &wt);
            return (
                Outcome::Failed(format!(
                    "attempt edited files out of scope: {} (allowed: {})",
                    violations.join(", "),
                    task.scope.join(", ")
                )),
                None,
            );
        }
    }

    // Effect sandwich, boundary 2 (commit outcome AFTER the effect): the
    // agent exited 0, its change is committed on the attempt branch, and it
    // is within scope. From here on, a resume must NEVER re-invoke the agent.
    record_phase(ctx, id, AttemptPhase::AgentDone);

    // 3) Acceptance gate (skipped for manual tasks).
    if !task.manual {
        if let Some(accept) = &task.accept {
            append("-- gate --");
            let gate_out = gate::run_accept(
                accept,
                &wt_path,
                &ctx.st.gate_env,
                Duration::from_secs(ctx.cfg.defaults.accept_timeout_s),
                task.gate_replay,
            );
            let out_lines = gate_out.combined();
            if !out_lines.is_empty() {
                append(&out_lines);
            }
            if !gate_out.passed() {
                if reused {
                    // The reused agent work failed the gate a SECOND time:
                    // drop the branch so the NEXT attempt is a full agent
                    // run. This bounds the optimisation to at most one
                    // cheap gate re-run, so genuinely wrong agent work is
                    // still recoverable.
                    cleanup(&repo, &wt);
                } else {
                    // The agent succeeded and its change is committed
                    // (AgentDone above); a gate failure may be a flake.
                    // Remove ONLY the worktree dir — the branch keeps the
                    // paid-for work durable for a gate-only retry.
                    append("-- gate failed; keeping committed branch for gate-only retry --");
                    worktree::remove_worktree_only(&repo, &wt);
                }
                return (
                    Outcome::Failed(format!(
                        "acceptance gate failed (exit {}): {}",
                        gate_out
                            .code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "-".into()),
                        gate_out.combined().trim()
                    )),
                    None,
                );
            }
            // Effect sandwich, boundary 3 (commit outcome AFTER the gate
            // effect): only the merge remains — resume re-runs it
            // idempotently without touching agent or gate.
            record_phase(ctx, id, AttemptPhase::GatePassed);
        }
    }

    // 4) Merge (serialized per repo; never force-push).
    let msg = format!("af: {} — {}", task.id, task.title);
    if let Err(e) = worktree::merge(&repo, &wt.branch, &ctx.merge_locks, &msg) {
        append(&format!("-- merge failed: {e}"));
        cleanup(&repo, &wt);
        return (Outcome::Failed(e), None);
    }
    append("-- merged --");
    cleanup(&repo, &wt);
    (Outcome::Merged, tokens)
}

fn cleanup(repo: &Path, wt: &worktree::Worktree) {
    worktree::remove(repo, wt);
}

/// Log body for a JSON-mode agent run: the RENDERED transcript (raw JSONL
/// in the log would be a debuggability regression — the log is how a human
/// debugs an attempt), falling back to the raw stdout when nothing parsed
/// (a CLI that ignored `--mode json` stays visible), with stderr appended
/// verbatim — stderr is never parsed, it rides along exactly as the legacy
/// combined stream carried it. Pure for testing.
fn json_log_body(t: &crate::transcript::Transcript, stdout: &str, stderr: &str) -> String {
    let mut body = if t.rendered.trim().is_empty() {
        stdout.trim_end()
    } else {
        t.rendered.trim_end()
    }
    .to_string();
    if !stderr.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(stderr);
    }
    body
}

/// Worktree + mode for one attempt. `(wt, true)` = gate-only retry: the
/// agent's committed work is DURABLE on the attempt branch — the branch
/// exists AND carries at least one commit beyond the base branch (e.g. a
/// previous attempt of this task failed only at the gate, whose cleanup
/// keeps the branch) — so the attempt attaches to that branch and skips
/// the agent entirely. `(wt, false)` = fresh branch via `worktree::create`
/// (which drops any stale branch first) — a full agent run, exactly the
/// pre-retry behavior. A branch that is absent or carries no work beyond
/// the base is NEVER treated as durable: legitimacy over cleverness, a
/// missing artifact must not silently skip the agent.
fn materialize_worktree(
    repo: &Path,
    wt_root: &Path,
    id: &str,
    prefix: &str,
) -> Result<(worktree::Worktree, bool), String> {
    let branch = format!("{prefix}/{id}");
    let durable = match worktree::current_branch(repo) {
        Ok(base) => {
            worktree::branch_exists(repo, &branch)
                && matches!(worktree::commits_ahead(repo, &base, &branch), Ok(n) if n > 0)
        }
        // Cannot even name the base branch → prove nothing, run the agent.
        Err(_) => false,
    };
    if durable {
        return worktree::attach(repo, wt_root, id, prefix).map(|w| (w, true));
    }
    worktree::create(repo, wt_root, id, prefix).map(|w| (w, false))
}

/// Paths changed on the attempt branch relative to the base branch's
/// merge-base (three-dot diff, so concurrent base advances are ignored).
/// Trusted git op (af's own), so the full environment is inherited.
fn changed_paths(wt_path: &Path, base_branch: &str) -> Result<Vec<String>, String> {
    let out = crate::subprocess::run(
        "git",
        &[
            "diff".to_string(),
            "--name-only".to_string(),
            format!("{base_branch}...HEAD"),
        ],
        Some(wt_path),
        &[],
        crate::subprocess::EnvMode::Inherit,
        Duration::from_secs(300),
    );
    if !out.passed() {
        return Err(format!(
            "cannot compute attempt diff ({base_branch}...HEAD): {}",
            out.combined().trim()
        ));
    }
    Ok(out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Changed paths no scope entry allows. An empty `scope` means "any file"
/// (current semantics). Uses the SAME matcher as the scheduler
/// (`crate::scheduler::scope_overlap`) so admission control and enforcement
/// agree on what "in scope" means.
fn scope_violations(changed: &[String], scope: &[String]) -> Vec<String> {
    if scope.is_empty() {
        return Vec::new();
    }
    changed
        .iter()
        .filter(|p| !scope.iter().any(|s| crate::scheduler::scope_overlap(p, s)))
        .cloned()
        .collect()
}

/// Full agent child argv: optional sandbox wrapper prefix, then the CLI and
/// its arguments (sandbox layer 3; pure for testing).
fn spawn_argv(st: &Settings, worker: &Worker, prompt_path: &Path) -> Vec<String> {
    let mut argv = st.sandbox_cmd.clone();
    argv.push(worker.cli.clone());
    argv.push("--provider".to_string());
    argv.push(worker.provider.clone());
    argv.push("--model".to_string());
    argv.push(worker.model.clone());
    // JSON output mode (r9-token-capture): ask the agent CLI for its JSON
    // Lines transcript so token usage can be captured (see `transcript`).
    // Placed with the other built-in flags — BEFORE the worker's own
    // `args` (same last-wins rule: a worker can still override the mode)
    // and never after the `-p @file` handoff. Text mode adds nothing, so
    // its argv stays byte-identical to the legacy one.
    if worker.output == "json" {
        argv.push("--mode".to_string());
        argv.push("json".to_string());
    }
    // STABLE POSITION (do not move silently): the worker's extra `args` go
    // AFTER `--model M` and BEFORE `-p @file`. After the built-in flags so a
    // caller can override anything the earlier argv set (most CLIs take the
    // last occurrence of a repeated flag); before `-p` so the prompt
    // handoff always stays LAST — no extra arg can displace or swallow the
    // prompt file the orchestrator rendered. Entries are passed through
    // verbatim (CLI-agnostic per ADR-1).
    argv.extend(worker.args.iter().cloned());
    argv.push("-p".to_string());
    argv.push(format!("@{}", prompt_path.display()));
    argv
}

const DEFAULT_PROMPT: &str = r#"You are an autonomous coding agent working in a git worktree.

TASK ID: {{TASK_ID}}
TITLE: {{TASK_TITLE}}
{{#SCOPE}}
Files you are allowed to modify:
{{SCOPE}}
{{/SCOPE}}
Acceptance criteria:
{{ACCEPTANCE}}

Acceptance gate command (run verbatim to verify this task):
{{ACCEPT_CMD}}

Work on TASK_ID only. Do not touch files outside the allowed scope.
When done, make sure the acceptance criteria hold and your changes are
committed on the current branch."#;

/// Render the agent prompt for one task from the configured template
/// (missing file ⇒ built-in `DEFAULT_PROMPT`). Public as the pinned
/// contract surface for `tests/contract_prompt.rs` — every render path
/// must surface scope, id/title, and the exact gate command.
pub fn render_prompt(
    st: &Settings,
    task: &crate::config::Task,
    worker: &Worker,
    context: Option<&str>,
) -> String {
    // A configured template wins; if it is unreadable/missing we fall back
    // to the built-in DEFAULT_PROMPT (which carries every placeholder).
    let custom = fs::read_to_string(&st.prompt_file).ok();
    let template = custom.as_deref().unwrap_or(DEFAULT_PROMPT);
    let scope = if task.scope.is_empty() {
        "*".to_string()
    } else {
        task.scope.join("\n")
    };
    let acceptance = task
        .acceptance_prose
        .clone()
        .unwrap_or_else(|| "(declared acceptance gate command)".to_string());
    // The EXACT shell command the acceptance gate will run (gate.rs) — the
    // agent must see it verbatim, not a prose paraphrase.
    let accept_cmd = task
        .accept
        .clone()
        .unwrap_or_else(|| "(no acceptance gate command — manual task)".to_string());
    let map = [
        ("{{TASK_ID}}", task.id.as_str()),
        ("{{TASK_TITLE}}", task.title.as_str()),
        ("{{SCOPE}}", &scope),
        ("{{ACCEPTANCE}}", &acceptance),
        ("{{ACCEPT_CMD}}", &accept_cmd),
        ("{{MODEL}}", worker.model.as_str()),
        ("{{PROVIDER}}", worker.provider.as_str()),
    ];
    let mut out = template.to_string();
    for (k, v) in map {
        out = out.replace(k, v);
    }
    out = strip_scope_markers(&out);

    // Hardening: a configured template that omits a placeholder would
    // silently drop the agent's file scope or acceptance gate (the exact
    // failure mode of the stray foreign template removed in f961ffe). Warn
    // on stderr and append the missing sections so scope/acceptance — and
    // the verbatim gate command — can never be silently dropped.
    if let Some(tpl) = &custom {
        let missing = |ph: &str| !tpl.contains(ph);
        if missing("{{SCOPE}}") {
            eprintln!(
                "af: warning: prompt template {} omits {{{{SCOPE}}}} — appending file scope",
                st.prompt_file.display()
            );
            out.push_str(&format!("\n\n## File scope (auto-appended)\n{scope}\n"));
        }
        if missing("{{ACCEPTANCE}}") {
            eprintln!(
                "af: warning: prompt template {} omits {{{{ACCEPTANCE}}}} — appending acceptance criteria",
                st.prompt_file.display()
            );
            out.push_str(&format!(
                "\n\n## Acceptance criteria (auto-appended)\n{acceptance}\n"
            ));
        }
        if missing("{{ACCEPT_CMD}}") {
            eprintln!(
                "af: warning: prompt template {} omits {{{{ACCEPT_CMD}}}} — appending gate command",
                st.prompt_file.display()
            );
            out.push_str(&format!(
                "\n\n## Acceptance gate command (auto-appended)\n{accept_cmd}\n"
            ));
        }
    }

    if let Some(ctx) = context {
        out.push_str(ctx);
    }
    out
}

/// Remove the literal `{{#SCOPE}}` / `{{/SCOPE}}` conditional markers from
/// a rendered prompt — they are template punctuation, not agent-visible
/// content, and used to leak verbatim into the rendered text. Lines that
/// consist solely of a marker are dropped entirely; inline occurrences are
/// replaced with the empty string. A trailing newline is preserved.
fn strip_scope_markers(rendered: &str) -> String {
    let mut out = rendered
        .lines()
        .filter(|l| {
            let t = l.trim();
            t != "{{#SCOPE}}" && t != "{{/SCOPE}}"
        })
        .map(|l| l.replace("{{#SCOPE}}", "").replace("{{/SCOPE}}", ""))
        .collect::<Vec<_>>()
        .join("\n");
    if rendered.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use std::path::PathBuf;

    fn worker(api_key_env: Option<&str>) -> Worker {
        Worker {
            name: "w".into(),
            provider: "p".into(),
            model: "m".into(),
            api_key_env: api_key_env.map(str::to_string),
            output: "text".into(),
            args: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn agent_env_exposes_only_worker_key_plus_git_hygiene() {
        let w = worker(Some("AF_TEST_KEY"));
        let lookup = |k: &str| match k {
            "TF_AGENT_ENV_PASSTHROUGH" => Some("A,B".into()),
            _ => None,
        };
        let (pairs, allow) = agent_env(&w, &lookup);
        assert!(pairs.contains(&("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())));
        assert!(pairs.contains(&(
            "GIT_CONFIG_KEY_0".to_string(),
            "credential.helper".to_string()
        )));
        assert!(pairs.contains(&("GIT_CONFIG_VALUE_0".to_string(), String::new())));
        assert!(
            allow.contains(&"AF_TEST_KEY".to_string()),
            "worker api key allowlisted"
        );
        assert!(allow.contains(&"PATH".to_string()));
        assert!(allow.contains(&"A".to_string()) && allow.contains(&"B".to_string()));
        assert!(!allow.contains(&"AF_OTHER_SECRET".to_string()));
    }

    #[test]
    fn agent_env_without_api_key_has_no_key_slot() {
        let (pairs, allow) = agent_env(&worker(None), &|_| None);
        assert!(!allow.iter().any(|k| k.starts_with("AF_")));
        assert_eq!(pairs.len(), 4);
    }

    #[test]
    fn spawn_argv_prepends_sandbox_wrapper() {
        let mut st = Settings::from_env();
        st.sandbox_cmd = vec!["echo".into(), "wrapped".into()];
        let argv = spawn_argv(&st, &worker(None), &PathBuf::from("p.md"));
        assert_eq!(&argv[..2], &["echo".to_string(), "wrapped".to_string()]);
        assert_eq!(argv[2], "pi");
        assert!(argv.contains(&"-p".to_string()) && argv.contains(&"@p.md".to_string()));

        st.sandbox_cmd.clear();
        let argv = spawn_argv(&st, &worker(None), &PathBuf::from("p.md"));
        assert_eq!(argv[0], "pi", "unset wrapper is a no-op");
    }

    #[test]
    fn spawn_argv_adds_mode_json_only_in_json_output_mode() {
        let mut st = Settings::from_env();
        st.sandbox_cmd = vec![];
        // json mode: `--mode json` rides with the built-in flags — after
        // `--model M`, before the worker's own `args`, never after `-p`.
        let mut w = worker(None);
        w.output = "json".into();
        w.args = vec!["--flag".into()];
        let argv = spawn_argv(&st, &w, &PathBuf::from("p.md"));
        let mode = argv
            .iter()
            .position(|a| a == "--mode")
            .expect("--mode present in json mode");
        assert_eq!(argv[mode + 1], "json");
        let model = argv.iter().position(|a| a == "--model").unwrap();
        assert!(mode > model, "--mode follows --model: {argv:?}");
        let flag = argv.iter().position(|a| a == "--flag").unwrap();
        assert!(flag > mode, "worker args follow --mode: {argv:?}");
        let p = argv.iter().position(|a| a == "-p").unwrap();
        assert_eq!(p, argv.len() - 2, "-p @file stays LAST: {argv:?}");

        // text mode (the default): the legacy argv, byte-identical — no
        // --mode anywhere.
        let text = spawn_argv(&st, &worker(None), &PathBuf::from("p.md"));
        assert!(!text.contains(&"--mode".to_string()), "{text:?}");
        assert_eq!(text.last().unwrap(), "@p.md");
    }

    #[test]
    fn json_log_body_renders_appends_stderr_and_falls_back_to_raw() {
        let t = crate::transcript::parse(
            "{\"type\":\"message_end\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"usage\":{\"input\":1,\"output\":1,\"totalTokens\":2}}}\n",
        );
        // Rendered transcript + stderr appended verbatim.
        assert_eq!(
            json_log_body(&t, "RAW", "a warning on stderr"),
            "assistant: hi\nusage: 1 in, 1 out, 2 total\na warning on stderr"
        );
        // Nothing parsed: the raw stdout stays visible so a CLI that
        // ignored `--mode json` is still debuggable from the log.
        let empty = crate::transcript::Transcript::default();
        assert_eq!(
            json_log_body(&empty, "plain text output", ""),
            "plain text output"
        );
        // Nothing at all: nothing appended (the caller skips empty bodies).
        assert_eq!(json_log_body(&empty, "", ""), "");
    }

    fn receipt_of(task: &str, attempt: u32, outcome: &str, error: Option<&str>) -> Receipt {
        Receipt {
            task: task.into(),
            attempt,
            worker: "w".into(),
            model: "m".into(),
            wall_clock_s: 1.0,
            tokens: None,
            ts: 0,
            outcome: outcome.into(),
            error: error.map(|e| e.into()),
        }
    }

    #[test]
    fn scope_violations_empty_scope_allows_any_file() {
        let changed = vec!["src/a.rs".to_string(), "docs/b.md".to_string()];
        assert!(scope_violations(&changed, &[]).is_empty());
    }

    #[test]
    fn scope_violations_flags_paths_no_entry_matches() {
        let changed = vec!["in_scope.txt".to_string(), "out_of_scope.txt".to_string()];
        let scope = vec!["in_scope.txt".to_string()];
        assert_eq!(
            scope_violations(&changed, &scope),
            vec!["out_of_scope.txt".to_string()]
        );
    }

    #[test]
    fn scope_violations_honors_glob_prefix_like_the_scheduler() {
        let changed = vec!["src/execute.rs".to_string(), "tests/x.rs".to_string()];
        let scope = vec!["src/*".to_string()];
        assert_eq!(
            scope_violations(&changed, &scope),
            vec!["tests/x.rs".to_string()]
        );
    }

    #[test]
    fn failure_context_first_attempt_has_no_block() {
        assert!(failure_context(&[], "A", 1).is_none());
    }

    #[test]
    fn failure_context_lists_only_earlier_failures_of_this_task() {
        let rs = vec![
            receipt_of("A", 1, "failed", Some("agent exited NonZero (code 7)")),
            receipt_of("B", 1, "failed", Some("other task")),
            receipt_of("A", 2, "failed", Some("gate failed (exit 1)")),
            receipt_of("A", 3, "merged", None),
        ];
        let ctx = failure_context(&rs, "A", 3).unwrap();
        assert!(ctx.contains("attempt 1: agent exited NonZero (code 7)"));
        assert!(ctx.contains("attempt 2: gate failed (exit 1)"));
        assert!(!ctx.contains("other task"), "other tasks excluded");
        assert!(!ctx.contains("merged"), "merged attempts excluded");
    }

    #[test]
    fn render_prompt_uses_custom_template_when_configured() {
        let dir = std::env::temp_dir().join(format!("af-prompt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let tpl = dir.join("tpl.md");
        std::fs::write(
            &tpl,
            "# {{TASK_ID}}: {{TASK_TITLE}}
scope: {{SCOPE}}
accept: {{ACCEPTANCE}}
model {{MODEL}}/{{PROVIDER}}
",
        )
        .unwrap();
        let st = crate::config::Settings {
            repo_dir: dir.clone(),
            state_dir: dir.clone(),
            worktree_root: dir.clone(),
            max_parallel: 1,
            branch_prefix: "tf".into(),
            poll_secs: 1,
            gate_env: vec![],
            tasks_file: dir.join("t.json"),
            workers_file: dir.join("w.json"),
            prompt_file: tpl.clone(),
            agent_timeout_s: 60,
            agent_stall_s: 0,
            max_wall_clock_s: 0,
            sandbox_cmd: vec![],
        };
        let task = crate::config::Task {
            id: "X1".into(),
            title: "do things".into(),
            scope: vec!["a.md".into(), "b.md".into()],
            acceptance_prose: Some("test -f done".into()),
            ..Default::default()
        };
        let worker = crate::config::Worker {
            name: "w".into(),
            provider: "openai".into(),
            model: "gpt-x".into(),
            enabled: true,
            cli: "c".into(),
            output: "text".into(),
            args: Vec::new(),
            ..Default::default()
        };
        let out = render_prompt(
            &st,
            &task,
            &worker,
            Some(
                "

## Previous attempts on this task
- attempt 1: boom
",
            ),
        );
        assert!(out.contains("# X1: do things"));
        assert!(
            out.contains(
                "a.md
b.md"
            ),
            "scope list joined"
        );
        assert!(out.contains("test -f done"));
        assert!(out.contains("gpt-x/openai"));
        assert!(out.contains("attempt 1: boom"), "context appended");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn settings_with_prompt_file(prompt_file: PathBuf) -> Settings {
        let mut st = Settings::from_env();
        st.prompt_file = prompt_file;
        st
    }

    #[test]
    fn prompt_includes_accept_command() {
        let task = crate::config::Task {
            id: "G1".into(),
            title: "gate task".into(),
            scope: vec!["src/lib.rs".into()],
            accept: Some("cargo test --all-features --locked".into()),
            ..Default::default()
        };
        // Default template (configured prompt file does not exist).
        let st = settings_with_prompt_file(PathBuf::from("no-such-default-template.md"));
        let out = render_prompt(&st, &task, &worker(None), None);
        assert!(
            out.contains("cargo test --all-features --locked"),
            "default prompt must embed the task's exact accept gate command"
        );
        // Custom template using the {{ACCEPT_CMD}} placeholder.
        let dir = std::env::temp_dir().join(format!("af-accept-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let tpl = dir.join("tpl.md");
        std::fs::write(&tpl, "run this: {{ACCEPT_CMD}}\n").unwrap();
        let out = render_prompt(
            &settings_with_prompt_file(tpl.clone()),
            &task,
            &worker(None),
            None,
        );
        assert!(
            out.contains("cargo test --all-features --locked"),
            "custom template must embed the task's exact accept gate command"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_prompt_strips_scope_conditional_markers() {
        let task = crate::config::Task {
            id: "S1".into(),
            title: "strip markers".into(),
            scope: vec!["a.rs".into()],
            accept: Some("true".into()),
            ..Default::default()
        };
        // DEFAULT_PROMPT itself contains the markers — they must not survive.
        assert!(DEFAULT_PROMPT.contains("{{#SCOPE}}"));
        let st = settings_with_prompt_file(PathBuf::from("no-such-default-template.md"));
        let out = render_prompt(&st, &task, &worker(None), None);
        assert!(!out.contains("{{#SCOPE}}"), "opening marker stripped");
        assert!(!out.contains("{{/SCOPE}}"), "closing marker stripped");
        assert!(out.contains("Files you are allowed to modify:"));
        assert!(out.contains("a.rs"), "scope content survives the strip");
    }

    #[test]
    fn render_prompt_appends_missing_scope_and_acceptance_for_custom_template() {
        let dir = std::env::temp_dir().join(format!("af-missing-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // Template carries NO {{SCOPE}}, {{ACCEPTANCE}} or {{ACCEPT_CMD}} —
        // the f961ffe failure mode: placeholders silently unsubstituted.
        let tpl = dir.join("bare.md");
        std::fs::write(&tpl, "Task {{TASK_ID}}: {{TASK_TITLE}}\n").unwrap();
        let task = crate::config::Task {
            id: "M1".into(),
            title: "missing placeholders".into(),
            scope: vec!["src/one.rs".into(), "src/two.rs".into()],
            accept: Some("cargo test --test cli".into()),
            acceptance_prose: Some("cli suite green".into()),
            ..Default::default()
        };
        let out = render_prompt(
            &settings_with_prompt_file(tpl.clone()),
            &task,
            &worker(None),
            None,
        );
        assert!(
            out.contains("src/one.rs\nsrc/two.rs"),
            "scope appended even though template omits {{SCOPE}}"
        );
        assert!(
            out.contains("cli suite green"),
            "acceptance appended even though template omits {{ACCEPTANCE}}"
        );
        assert!(
            out.contains("cargo test --test cli"),
            "gate command appended even though template omits {{ACCEPT_CMD}}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
