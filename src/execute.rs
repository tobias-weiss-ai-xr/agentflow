//! Execute: one task attempt — worktree → prompt → agent → gate → merge
//! (spec: lifecycle). Retries & status transitions are decided by the run
//! loop (`run.rs`); this module performs a single attempt and reports the
//! outcome.

use crate::config::{Config, Settings, Worker};
use crate::state::{Receipt, Store, now_ts};
use crate::{gate, worktree};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

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

pub fn execute_task(ctx: &ExecCtx, worker: &Worker, id: &str, attempt: u32, log_path: &Path) -> Outcome {
    let start = Instant::now();
    let task = match ctx.cfg.by_id.get(id) {
        Some(t) => t.clone(),
        None => return Outcome::Failed(format!("unknown task {id}")),
    };

    let repo = ctx.st.repo_dir.clone();
    let wt = match worktree::create(
        &repo,
        &ctx.st.worktree_root,
        id,
        &ctx.st.branch_prefix.clone(),
    ) {
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
    append(&format!("== attempt {attempt} on worker {} ({}) ==", worker.name, worker.model));

    // 1) Render + write prompt.
    let prompt = render_prompt(&ctx.st, &task, worker);
    let prompt_path = ctx.store.prompt_dir().join(format!("{id}.md"));
    let _ = fs::create_dir_all(ctx.store.prompt_dir());
    if let Err(e) = fs::write(&prompt_path, &prompt) {
        cleanup(&repo, &wt, &ctx.st);
        return Outcome::Failed(format!("cannot write prompt: {e}"));
    }

    // 2) Agent CLI (external, OpenAI-compatible): --provider P --model M -p @file
    let agent_args = vec![
        "--provider".to_string(),
        worker.provider.clone(),
        "--model".to_string(),
        worker.model.clone(),
        "-p".to_string(),
        format!("@{}", prompt_path.display()),
    ];
    append("-- agent --");
    let agent_out = crate::subprocess::run(
        &worker.cli,
        &agent_args,
        Some(&wt_path),
        &[],
        Duration::from_secs(ctx.st.agent_timeout_s),
    );
    let mut out_lines = agent_out.combined();
    if !out_lines.is_empty() {
        append(&out_lines);
    }
    if !agent_out.passed() {
        let (kind, code) = (format!("{:?}", agent_out.kind), agent_out.code.map(|c| c.to_string()).unwrap_or_default());
        cleanup(&repo, &wt, &ctx.st);
        let _ = (kind, code);
        return Outcome::Failed(format!(
            "agent exited {} (code {})",
            format!("{:?}", agent_out.kind),
            agent_out.code.map(|c| c.to_string()).unwrap_or_else(|| "-".into())
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
                cleanup(&repo, &wt, &ctx.st);
                return Outcome::Failed(format!(
                    "acceptance gate failed (exit {}): {}",
                    gate_out.code.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
                    gate_out.combined().trim()
                ));
            }
        }
    }

    // 4) Merge (serialized per repo; never force-push).
    let msg = format!("af: {} — {}", task.id, task.title);
    if let Err(e) = worktree::merge(&repo, &wt.branch, &ctx.merge_locks, &msg) {
        append(&format!("-- merge failed: {e}"));
        cleanup(&repo, &wt, &ctx.st);
        return Outcome::Failed(e);
    }
    append("-- merged --");
    let wall = start.elapsed().as_secs_f64();

    let _ = ctx.store.append_receipt(&Receipt {
        task: id.to_string(),
        attempt,
        worker: worker.name.clone(),
        model: worker.model.clone(),
        wall_clock_s: wall,
        tokens: None,
        ts: now_ts(),
    });

    cleanup(&repo, &wt, &ctx.st);
    Outcome::Merged
}

fn cleanup(repo: &std::path::PathBuf, wt: &worktree::Worktree, st: &Settings) {
    worktree::remove(repo, wt);
    let _ = st.worktree_root;
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

Work on TASK_ID only. Do not touch files outside the allowed scope.
When done, make sure the acceptance criteria hold and your changes are
committed on the current branch."#;

fn render_prompt(st: &Settings, task: &crate::config::Task, worker: &Worker) -> String {
    let template = fs::read_to_string(&st.prompt_file).unwrap_or_else(|_| DEFAULT_PROMPT.to_string());
    let scope = if task.scope.is_empty() {
        "*".to_string()
    } else {
        task.scope.join("\n")
    };
    let acceptance = task
        .acceptance_prose
        .clone()
        .unwrap_or_else(|| "(declared acceptance gate command)".to_string());
    let map = [
        ("{{TASK_ID}}", task.id.as_str()),
        ("{{TASK_TITLE}}", task.title.as_str()),
        ("{{SCOPE}}", &scope),
        ("{{ACCEPTANCE}}", &acceptance),
        ("{{MODEL}}", worker.model.as_str()),
        ("{{PROVIDER}}", worker.provider.as_str()),
    ];
    let mut out = template;
    for (k, v) in map {
        out = out.replace(k, v);
    }
    out
}
