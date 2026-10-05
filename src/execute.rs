//! Execute: one task attempt — worktree → prompt → agent → gate → merge
//! (spec: lifecycle). Retries & status transitions are decided by the run
//! loop (`run.rs`); this module performs a single attempt and reports the
//! outcome.

use crate::config::{Config, Settings, Worker};
use crate::state::{now_ts, Receipt, Store};
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
    let outcome = execute_attempt(ctx, worker, id, attempt, log_path);
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
        tokens: None,
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

fn execute_attempt(
    ctx: &ExecCtx,
    worker: &Worker,
    id: &str,
    attempt: u32,
    log_path: &Path,
) -> Outcome {
    let task = match ctx.cfg.by_id.get(id) {
        Some(t) => t.clone(),
        None => return Outcome::Failed(format!("unknown task {id}")),
    };

    // Multi-repo (ADR-11): worktree, branch, and merge target the task's repo.
    let repo = ctx.cfg.repo_dir_for(&task, &ctx.st.repo_dir);
    let wt = match worktree::create(&repo, &ctx.st.worktree_root, id, &ctx.st.branch_prefix) {
        Ok(w) => w,
        Err(e) => return Outcome::Failed(e),
    };
    let wt_path = wt.path.clone();

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
        return Outcome::Failed(format!("cannot write prompt: {e}"));
    }

    // 2) Agent CLI (external, OpenAI-compatible): --provider P --model M -p @file
    let argv = spawn_argv(&ctx.st, worker, &prompt_path);
    let (agent_cmd, agent_args) = argv.split_first().unwrap();
    let (env_pairs, env_allow) = agent_env(worker, &|k| std::env::var(k).ok());
    append("-- agent --");
    let agent_out = crate::subprocess::run(
        agent_cmd,
        agent_args,
        Some(&wt_path),
        &env_pairs,
        crate::subprocess::EnvMode::Allowlist(env_allow),
        Duration::from_secs(ctx.st.agent_timeout_s),
    );
    let mut out_lines = agent_out.combined();
    if !out_lines.is_empty() {
        append(&out_lines);
    }
    if !agent_out.passed() {
        cleanup(&repo, &wt);
        return Outcome::Failed(format!(
            "agent exited {:?} (code {})",
            agent_out.kind,
            agent_out
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into())
        ));
    }

    // 3) Acceptance gate (skipped for manual tasks).
    if !task.manual {
        if let Some(accept) = &task.accept {
            append("-- gate --");
            let gate_out = gate::run_accept(
                accept,
                &wt_path,
                &ctx.st.gate_env,
                Duration::from_secs(ctx.cfg.defaults.accept_timeout_s),
            );
            out_lines = gate_out.combined();
            if !out_lines.is_empty() {
                append(&out_lines);
            }
            if !gate_out.passed() {
                cleanup(&repo, &wt);
                return Outcome::Failed(format!(
                    "acceptance gate failed (exit {}): {}",
                    gate_out
                        .code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "-".into()),
                    gate_out.combined().trim()
                ));
            }
        }
    }

    // 4) Merge (serialized per repo; never force-push).
    let msg = format!("af: {} — {}", task.id, task.title);
    if let Err(e) = worktree::merge(&repo, &wt.branch, &ctx.merge_locks, &msg) {
        append(&format!("-- merge failed: {e}"));
        cleanup(&repo, &wt);
        return Outcome::Failed(e);
    }
    append("-- merged --");
    cleanup(&repo, &wt);
    Outcome::Merged
}

fn cleanup(repo: &Path, wt: &worktree::Worktree) {
    worktree::remove(repo, wt);
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

fn render_prompt(
    st: &Settings,
    task: &crate::config::Task,
    worker: &Worker,
    context: Option<&str>,
) -> String {
    // A configured template wins; if it is unreadable/missing we fall back
    // to the built-in DEFAULT_PROMPT (which carries every placeholder).
    let custom = fs::read_to_string(&st.prompt_file).ok();
    let template = custom
        .as_deref()
        .unwrap_or(DEFAULT_PROMPT);
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
