//! Run: the dispatch loop (poll → reap → dispatch) and query commands used
//! by the CLI (spec: cli, scheduling, state).

use crate::config::{Config, Settings, TaskState};
use crate::execute::{self, ExecCtx, Outcome};
use crate::gate;
use crate::router::Router;
use crate::scheduler;
use crate::state::{resume_action, AttemptPhase, ResumeAction, Store, TaskStatus};
use crate::subprocess::{self, EnvMode};
use crate::worktree;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub once: bool,
    pub dry_run: bool,
    pub worker_filter: Option<String>,
    pub task_filter: Option<String>,
    pub poll_secs: Option<u64>,
}

/// Reap finished attempts from the channel: worker busy-ness, routing
/// stats, status transitions + persistence. Shared by the main loop and
/// `--once` (single source of truth for the attempt bookkeeping).
fn reap(
    rx: &mpsc::Receiver<(String, String, Outcome)>,
    running: &Arc<Mutex<HashMap<String, ()>>>,
    worker_busy: &Arc<Mutex<HashMap<String, bool>>>,
    router: &mut Router,
    status: &mut HashMap<String, TaskStatus>,
    store: &Store,
    max_attempts: u32,
) {
    while let Ok((id, worker_name, outcome)) = rx.try_recv() {
        running.lock().unwrap().remove(&id);
        worker_busy
            .lock()
            .unwrap()
            .insert(worker_name.clone(), false);
        router.record(&worker_name, matches!(outcome, Outcome::Merged));
        // Journal reconciliation: attempt threads persist phase boundaries
        // via load-modify-save (see `record_phase`), so this in-memory map
        // has not seen them. Copy every entry's freshest persisted boundary
        // over before the whole-map save below — otherwise reaping one task
        // would clobber a concurrent attempt's journal (or this attempt's
        // own final boundary) with the stale pre-attempt copy.
        for (k, persisted) in &store.load() {
            if let Some(s) = status.get_mut(k) {
                s.phase = persisted.phase;
            }
        }
        let s = status.entry(id.clone()).or_default();
        s.attempts += 1;
        match outcome {
            Outcome::Merged => {
                s.state = TaskState::Done;
                s.last_error = None;
                println!("  ✓ {id} done (attempt {})", s.attempts);
            }
            Outcome::Failed(err) => {
                s.last_error = Some(err.clone());
                if s.attempts >= max_attempts {
                    s.state = TaskState::Failed;
                    println!(
                        "  ✗ {id} failed (attempt {}/{}): {}",
                        s.attempts, max_attempts, err
                    );
                } else {
                    s.state = TaskState::Ready;
                    println!("  ✗ {id} attempt {} failed, retrying: {}", s.attempts, err);
                }
            }
        }
        let _ = store.save(status);
    }
}

/// UCB1 selection among the free, enabled, filter-matching workers.
pub fn pick_worker<'a>(
    cfg: &'a Config,
    busy: &Mutex<HashMap<String, bool>>,
    router: &Router,
    filter: Option<&str>,
) -> Option<&'a crate::config::Worker> {
    let busy = busy.lock().unwrap();
    let eligible: Vec<&crate::config::Worker> = cfg
        .workers
        .iter()
        .filter(|w| w.enabled)
        .filter(|w| match filter {
            Some(f) => w.name == f,
            None => true,
        })
        .filter(|w| !busy.get(&w.name).copied().unwrap_or(false))
        .collect();
    router.pick(eligible)
}

/// `af run`: execute tasks until all done or deadlock. Returns process exit code.
pub fn run_loop(cfg: &Config, st: &Settings, opts: &RunOptions) -> i32 {
    if opts.dry_run {
        return dry_run(cfg, st);
    }

    let store = Store::new(st.state_dir.clone());
    // Single writer per TF_STATE_DIR: a second `af run` must not clobber
    // run-state.json. Held (and released on drop) for the whole run.
    let _lock = match store.acquire_lock() {
        Ok(g) => g,
        Err(err) => {
            eprintln!("config error: {err}");
            return 2;
        }
    };
    let poll = opts.poll_secs.unwrap_or(st.poll_secs);
    let max_attempts = cfg.defaults.max_attempts;

    // --- self-heal: remove orphan worktrees from dead attempts ---
    let mut status = store.load();
    let stale_running: Vec<String> = status
        .iter()
        .filter(|(_, s)| s.state == TaskState::Running)
        .map(|(k, _)| k.clone())
        .collect();
    // Multi-repo (ADR-11): heal tries every configured repo + the default.
    let mut repo_list: Vec<(String, PathBuf)> = cfg
        .repos
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    repo_list.push(("(default)".to_string(), st.repo_dir.clone()));
    worktree::heal(
        &repo_list,
        &st.worktree_root,
        &st.branch_prefix,
        &stale_running,
    );
    // Effect-sandwich resume (pi-durable): for each stale `running` task,
    // finish only the effects its dead attempt had not yet committed — a
    // crash must never re-run the expensive, non-replayable agent step.
    for id in &stale_running {
        heal_stale_attempt(cfg, st, &mut status, id, max_attempts);
    }
    let _ = store.save(&status);

    let log_dir = store.log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let _ = std::fs::create_dir_all(store.prompt_dir());

    let (tx, rx) = mpsc::channel::<(String, String, Outcome)>();
    let merge_locks = worktree::MergeLocks::new();
    let worker_busy: Arc<Mutex<HashMap<String, bool>>> = Arc::new(Mutex::new(HashMap::new()));
    let running: Arc<Mutex<HashMap<String, ()>>> = Arc::new(Mutex::new(HashMap::new()));

    let enabled = cfg.workers.iter().filter(|w| w.enabled).count();
    let max_parallel = if st.max_parallel == 0 {
        enabled
    } else {
        st.max_parallel
    }
    .max(1);

    let ctx = ExecCtx {
        cfg: cfg.clone(),
        st: st.clone(),
        store: store.clone(),
        merge_locks: merge_locks.clone(),
    };

    println!(
        "agentflow run: {} tasks, {enabled} enabled worker(s), max_parallel={max_parallel}",
        cfg.tasks.len()
    );

    // A --worker filter naming no enabled worker could never dispatch → hang.
    // Fail fast instead.
    if let Some(f) = &opts.worker_filter {
        let ok = cfg.workers.iter().any(|w| w.enabled && &w.name == f);
        if !ok {
            eprintln!("config error: --worker '{f}' matches no enabled worker");
            return 2;
        }
    }

    // In-scope tasks: all tasks, or only the --task filter target. Completion
    // and deadlock are judged on this scope so out-of-scope work never blocks.
    let in_scope: Vec<&crate::config::Task> = cfg
        .tasks
        .iter()
        .filter(|t| match &opts.task_filter {
            Some(f) => &t.id == f,
            None => true,
        })
        .collect();
    if let Some(f) = &opts.task_filter {
        if in_scope.is_empty() {
            eprintln!("config error: --task '{f}' matches no task");
            return 2;
        }
    }

    // --- dispatch loop ---
    // Measured routing (ADR-12): replay receipts into per-worker stats.
    let mut router = Router::from_receipts(&store.load_receipts());
    loop {
        // Reap finished tasks.
        reap(
            &rx,
            &running,
            &worker_busy,
            &mut router,
            &mut status,
            &store,
            max_attempts,
        );

        // Terminal conditions (judged on the in-scope tasks only).
        let all_done = in_scope
            .iter()
            .all(|t| matches!(status.get(&t.id).map(|s| &s.state), Some(TaskState::Done)));
        if all_done {
            println!("\nAll tasks done.");
            println!("{}", board_of(cfg, &status));
            return 0;
        }
        let running_ids: Vec<String> = running.lock().unwrap().keys().cloned().collect();
        if running_ids.is_empty() {
            // Nothing in flight: progress is possible only if something is
            // ready. Otherwise this is a deadlock (failed/absent deps, or a
            // --task target waiting on out-of-scope work).
            let ready_in_scope = scheduler::ready_tasks(cfg, &status, &[], max_attempts)
                .into_iter()
                .filter(|t| in_scope.iter().any(|s| s.id == t.id))
                .count();
            if ready_in_scope == 0 {
                let blocked = scheduler::find_deadlock_in(cfg, &status, &[], &in_scope)
                    .unwrap_or_else(|| in_scope.iter().map(|t| t.id.clone()).collect());
                eprintln!(
                    "DEADLOCK: no task can make progress. Blocked: {}",
                    blocked.join(", ")
                );
                eprintln!("{}", board_of(cfg, &status));
                return 2;
            }
        }

        // Dispatch a round.
        if running.lock().unwrap().len() < max_parallel {
            let running_now = running.lock().unwrap().keys().cloned().collect::<Vec<_>>();
            let ready = scheduler::ready_tasks(cfg, &status, &running_now, max_attempts);
            for t in ready {
                if let Some(f) = &opts.task_filter {
                    if &t.id != f {
                        continue;
                    }
                }
                if running.lock().unwrap().len() >= max_parallel {
                    break;
                }
                let worker =
                    match pick_worker(cfg, &worker_busy, &router, opts.worker_filter.as_deref()) {
                        Some(w) => w.clone(),
                        None => break, // no free worker this round
                    };
                // Mark Running + persist BEFORE spawning (crash-safety).
                let attempt = {
                    let s = status.entry(t.id.clone()).or_default();
                    s.state = TaskState::Running;
                    // Fresh attempt: the journal restarts at Spawned (the
                    // worker commits it before spawning the agent). A crash
                    // in that window resumes as RerunAgent — always safe.
                    s.phase = None;
                    s.attempts + 1
                };
                let _ = store.save(&status);

                worker_busy
                    .lock()
                    .unwrap()
                    .insert(worker.name.clone(), true);
                running.lock().unwrap().insert(t.id.clone(), ());
                let log_path = log_dir.join(format!("{}.log", t.id));
                let tx2 = tx.clone();
                let ctx2 = ctx.clone();
                let id = t.id.clone();
                let wname = worker.name.clone();
                let wclone = worker.clone();
                let model = worker.model.clone();
                println!(
                    "  → {id} dispatch on {} ({model}) [attempt {attempt}]",
                    worker.name
                );
                std::thread::spawn(move || {
                    let out = execute::execute_task(&ctx2, &wclone, &id, attempt, &log_path);
                    let _ = tx2.send((id, wname, out));
                });
            }
            if opts.once {
                // --once: dispatch no more after this round; wait for in-flight.
                while !running.lock().unwrap().is_empty() {
                    reap(
                        &rx,
                        &running,
                        &worker_busy,
                        &mut router,
                        &mut status,
                        &store,
                        max_attempts,
                    );
                    std::thread::sleep(Duration::from_millis(200));
                }
                println!("{}", board_of(cfg, &status));
                return 0;
            }
        }

        std::thread::sleep(Duration::from_secs(poll.max(1)));
    }
}

/// git helper for the resume paths (same trusted-child policy as
/// worktree.rs's own git operations: full inherited env, hard timeout).
fn git_run(repo: &Path, args: &[&str]) -> subprocess::CmdOut {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    subprocess::run(
        "git",
        &args,
        Some(repo),
        &[],
        EnvMode::Inherit,
        Duration::from_secs(300),
    )
}

/// Does the attempt branch still exist in the repo? (The agent's durable
/// work lives there.)
fn branch_exists(repo: &Path, branch: &str) -> bool {
    git_run(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .passed()
}

/// Is `branch` already fully contained in the repo's checked-out base?
fn branch_merged_into_head(repo: &Path, branch: &str) -> bool {
    git_run(repo, &["merge-base", "--is-ancestor", branch, "HEAD"]).passed()
}

/// Is this task's af merge commit already in the base's history? Covers the
/// crash window AFTER a successful merge whose cleanup already deleted the
/// branch — the task's work is in the base, so "record merged" is exact.
fn merge_commit_in_base(repo: &Path, id: &str) -> bool {
    let out = git_run(
        repo,
        &[
            "log",
            "--format=%H",
            "-1",
            "--grep",
            &format!("^af: {id} — "),
            "HEAD",
        ],
    );
    out.passed() && !out.stdout.trim().is_empty()
}

/// Keep/repair the dead attempt's worktree so the gate can re-run inside
/// it: a valid worktree is reused as-is; anything else (missing dir, or a
/// raw leftover directory without git metadata) is rebuilt from the
/// EXISTING branch (`git worktree add <path> <branch>` — no new branch, so
/// the committed agent work is never discarded). None when git cannot
/// materialize it → caller falls back to RerunAgent.
fn ensure_worktree_on_branch(
    repo: &Path,
    wt_root: &Path,
    id: &str,
    branch: &str,
) -> Option<PathBuf> {
    let wt_path = wt_root.join(id);
    // A live worktree carries a `.git` file/dir; a stale plain directory
    // does not and must be cleared before `worktree add`.
    if wt_path.join(".git").exists() {
        return Some(wt_path);
    }
    let _ = std::fs::remove_dir_all(&wt_path);
    std::fs::create_dir_all(wt_root).ok()?;
    // Drop stale registrations left by raw-deleted worktree dirs (same
    // reason as worktree::create) so the branch is free to check out.
    let _ = git_run(repo, &["worktree", "prune"]);
    let out = git_run(repo, &["worktree", "add", wt_path.to_str()?, branch]);
    if out.passed() {
        Some(wt_path)
    } else {
        None
    }
}

/// Reset a resumed-and-failed attempt exactly like a normally failed one:
/// terminal Failed at max attempts, else Ready for a fresh agent attempt.
fn mark_resumed_failure(
    status: &mut HashMap<String, TaskStatus>,
    id: &str,
    max_attempts: u32,
    err: &str,
) {
    if let Some(s) = status.get_mut(id) {
        s.last_error = Some(err.to_string());
        s.phase = None; // next attempt journals from Spawned again
        s.state = if s.attempts >= max_attempts {
            TaskState::Failed
        } else {
            TaskState::Ready
        };
    }
}

/// Shared tail of both resume paths: land the attempt branch in the base
/// IDEMPOTENTLY (merge — or just record merged when the branch is already
/// an ancestor / the task's merge commit is already in the base), mark the
/// task Done, and drop the attempt's worktree + branch. Returns false only
/// when the task is unknown (caller falls back to RerunAgent).
fn finish_resume_merge(
    cfg: &Config,
    st: &Settings,
    status: &mut HashMap<String, TaskStatus>,
    id: &str,
    max_attempts: u32,
    repo: &Path,
    branch: &str,
) -> bool {
    let Some(task) = cfg.by_id.get(id) else {
        return false;
    };
    let already_merged = branch_merged_into_head(repo, branch) || merge_commit_in_base(repo, id);
    let merged = if already_merged {
        Ok(()) // idempotent: record merged without a second merge
    } else {
        worktree::merge(
            repo,
            branch,
            &worktree::MergeLocks::new(),
            &format!("af: {} — {}", task.id, task.title),
        )
    };
    match merged {
        Ok(()) => {
            if let Some(s) = status.get_mut(id) {
                s.state = TaskState::Done;
                s.last_error = None;
                s.phase = Some(AttemptPhase::GatePassed);
            }
            // Merge consumed the attempt — drop its worktree + branch
            // (best-effort, exactly like execute_attempt's cleanup).
            worktree::remove(
                repo,
                &worktree::Worktree {
                    path: st.worktree_root.join(id),
                    branch: branch.to_string(),
                },
            );
            println!("  ✓ {id} resumed to done (agent not re-run)");
            true
        }
        Err(e) => {
            mark_resumed_failure(
                status,
                id,
                max_attempts,
                &format!("resume merge failed: {e}"),
            );
            true
        }
    }
}

/// RerunGate resume (phase = AgentDone): the agent already committed its
/// change on `<prefix>/<id>` — re-run ONLY the acceptance gate in a
/// kept/repaired worktree, then merge on success. Never re-invokes the
/// agent. Returns false when the resume artifacts are gone (branch/worktree
/// cannot be recovered) → caller falls back to RerunAgent (always safe).
fn resume_gate_only(
    cfg: &Config,
    st: &Settings,
    status: &mut HashMap<String, TaskStatus>,
    id: &str,
    max_attempts: u32,
) -> bool {
    let Some(task) = cfg.by_id.get(id) else {
        return false;
    };
    let repo = cfg.repo_dir_for(task, &st.repo_dir);
    let branch = format!("{}/{}", st.branch_prefix, id);
    if !branch_exists(&repo, &branch) {
        return false; // the agent's durable work is gone — only a re-run recovers
    }
    let Some(wt_path) = ensure_worktree_on_branch(&repo, &st.worktree_root, id, &branch) else {
        return false;
    };
    // Re-run only the gate. Manual/gateless attempts have nothing to check —
    // the agent step was their last pre-merge effect.
    let gate_ok = if !task.manual {
        match &task.accept {
            Some(accept) => gate::run_accept(
                accept,
                &wt_path,
                &st.gate_env,
                Duration::from_secs(cfg.defaults.accept_timeout_s),
                task.gate_replay,
            )
            .passed(),
            None => true,
        }
    } else {
        true
    };
    if !gate_ok {
        mark_resumed_failure(status, id, max_attempts, "resumed acceptance gate failed");
        return true;
    }
    // Journal the boundary in the in-memory map (persisted by the heal's
    // single save): gate passed — only the merge remains.
    if let Some(s) = status.get_mut(id) {
        s.phase = Some(AttemptPhase::GatePassed);
    }
    finish_resume_merge(cfg, st, status, id, max_attempts, &repo, &branch)
}

/// MergeOnly resume (phase = GatePassed): gate already passed — land the
/// branch idempotently (or record merged when it already is). Never invokes
/// agent or gate. Falls back to RerunAgent only when there is NO evidence of
/// the work: no branch AND no merge commit in the base.
fn resume_merge_only(
    cfg: &Config,
    st: &Settings,
    status: &mut HashMap<String, TaskStatus>,
    id: &str,
    max_attempts: u32,
) -> bool {
    let Some(task) = cfg.by_id.get(id) else {
        return false;
    };
    let repo = cfg.repo_dir_for(task, &st.repo_dir);
    let branch = format!("{}/{}", st.branch_prefix, id);
    if !branch_exists(&repo, &branch) && !merge_commit_in_base(&repo, id) {
        return false; // → RerunAgent
    }
    finish_resume_merge(cfg, st, status, id, max_attempts, &repo, &branch)
}

/// Startup heal for one stale `running` task — the effect-sandwich resume.
/// RerunAgent: previous behavior (reset to Ready; the stale worktree is
/// replaced when the retry is dispatched). RerunGate / MergeOnly: finish
/// only the remaining effects, never re-invoking the agent.
fn heal_stale_attempt(
    cfg: &Config,
    st: &Settings,
    status: &mut HashMap<String, TaskStatus>,
    id: &str,
    max_attempts: u32,
) {
    let action = resume_action(status.get(id).and_then(|s| s.phase));
    let handled = match action {
        ResumeAction::RerunGate => resume_gate_only(cfg, st, status, id, max_attempts),
        ResumeAction::MergeOnly => resume_merge_only(cfg, st, status, id, max_attempts),
        ResumeAction::RerunAgent => false, // handled by the reset below
    };
    if !handled {
        // RerunAgent, or a resume that lost its artifacts (always safe):
        // fresh attempt from Ready; the stale worktree is dropped by
        // worktree::create when the retry is dispatched.
        if let Some(s) = status.get_mut(id) {
            s.state = TaskState::Ready;
            s.last_error = Some("previous run interrupted".into());
            s.phase = None;
        }
    }
}

/// `af clean [--dry-run]`: remove orphaned worktrees + branches left behind
/// by crashed runs. Tasks still marked Running are preserved. `--dry-run`
/// reports what would go and changes nothing. Returns the process exit code.
pub fn clean(cfg: &Config, st: &Settings, dry_run: bool) -> i32 {
    let status = Store::new(st.state_dir.clone()).load();
    let running: Vec<String> = status
        .iter()
        .filter(|(_, s)| s.state == TaskState::Running)
        .map(|(k, _)| k.clone())
        .collect();
    // Multi-repo (ADR-11): try every configured repo + the default, exactly
    // like the startup self-heal.
    let mut repo_list: Vec<(String, PathBuf)> = cfg
        .repos
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    repo_list.push(("(default)".to_string(), st.repo_dir.clone()));

    let ids = worktree::clean(
        &repo_list,
        &st.worktree_root,
        &st.branch_prefix,
        &running,
        dry_run,
    );
    if ids.is_empty() {
        println!(
            "clean: no orphaned worktrees under {}",
            st.worktree_root.display()
        );
        return 0;
    }
    for id in &ids {
        let path = st.worktree_root.join(id);
        let branch = format!("{}/{}", st.branch_prefix, id);
        if dry_run {
            println!("would remove {} (branch {branch})", path.display());
        } else {
            println!("removed {} (branch {branch})", path.display());
        }
    }
    if dry_run {
        println!(
            "dry run: {} orphan(s) would be removed; nothing changed",
            ids.len()
        );
    } else {
        println!("clean: removed {} orphan(s)", ids.len());
    }
    0
}

pub fn dry_run(cfg: &Config, st: &Settings) -> i32 {
    let depths = scheduler::compute_depths(cfg).unwrap_or_default();
    let status = Store::new(st.state_dir.clone()).load();
    // Full plan, not just the first ready batch: every task in dispatch-wave
    // order (depth ascending, then config order) with its readiness, so a
    // deep DAG is visible end-to-end before anything is dispatched.
    let mut indexed: Vec<(usize, &crate::config::Task)> = cfg.tasks.iter().enumerate().collect();
    indexed.sort_by(|(ia, a), (ib, b)| {
        let da = depths.get(&a.id).copied().unwrap_or(0);
        let db = depths.get(&b.id).copied().unwrap_or(0);
        da.cmp(&db).then(ia.cmp(ib))
    });
    let worker = cfg
        .workers
        .iter()
        .find(|w| w.enabled)
        .map(|w| w.name.as_str())
        .unwrap_or("-");
    println!(
        "Dry run — dispatch plan ({} tasks, {} workers):",
        cfg.tasks.len(),
        cfg.workers.len()
    );
    println!(
        "  {:<12} {:<5} {:<24} {:<8} SCOPE",
        "TASK", "DEPTH", "READINESS", "WORKER"
    );
    let max_attempts = cfg.defaults.max_attempts;
    let mut ready_now = 0usize;
    for (_, t) in &indexed {
        let depth = depths.get(&t.id).copied().unwrap_or(0);
        let readiness = scheduler::readiness_of(t, &status, max_attempts);
        if readiness == scheduler::Readiness::Ready {
            ready_now += 1;
        }
        println!(
            "  {:<12} {:<5} {:<24} {:<8} {}",
            t.id,
            depth,
            readiness.label(),
            worker,
            if t.scope.is_empty() {
                "*".to_string()
            } else {
                t.scope.join(",")
            }
        );
    }
    println!("  ({ready_now} ready now; nothing was created or changed)");
    0
}

fn board_of(cfg: &Config, status: &HashMap<String, TaskStatus>) -> String {
    let mut rows = vec![format!(
        "{:<12} {:<9} {:<8} {}",
        "TASK", "STATE", "ATTEMPTS", "LAST ERROR"
    )];
    for t in &cfg.tasks {
        let s = status.get(&t.id).cloned().unwrap_or_default();
        let err = s
            .last_error
            .as_deref()
            .map(|e| e.chars().take(48).collect::<String>()) // chars, not bytes: UTF-8 safe
            .unwrap_or_default();
        rows.push(format!(
            "{:<12} {:<9?} {:<8} {}",
            t.id, s.state, s.attempts, err
        ));
    }
    rows.join("\n")
}

/// `af status` / `af api status` (human board).
pub fn status_board(cfg: &Config, st: &Settings) -> String {
    let status = Store::new(st.state_dir.clone()).load();
    board_of(cfg, &status)
}

/// `af api status --json`.
pub fn status_json(cfg: &Config, st: &Settings) -> String {
    let status = Store::new(st.state_dir.clone()).load();
    let mut map = serde_json::Map::new();
    for t in &cfg.tasks {
        let s = status.get(&t.id).cloned().unwrap_or_default();
        map.insert(
            t.id.clone(),
            serde_json::json!({
                "id": t.id,
                "state": format!("{:?}", s.state).to_lowercase(),
                "attempts": s.attempts,
                "last_error": s.last_error,
            }),
        );
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_default()
}

/// `af cost`: aggregate receipts (wall-clock truth, ADR-9).
pub fn cost(cfg: &Config, st: &Settings, task_filter: Option<&str>) -> String {
    let store = Store::new(st.state_dir.clone());
    let receipts = store.load_receipts();
    let mut lines = format!(
        "{:<12} {:<9} {:<10} {}",
        "TASK", "ATTEMPTS", "WALL_S", "MODEL"
    );
    let mut total: f64 = 0.0;
    for t in &cfg.tasks {
        if let Some(f) = task_filter {
            if t.id != f {
                continue;
            }
        }
        let rs: Vec<_> = receipts.iter().filter(|r| r.task == t.id).collect();
        let wall: f64 = rs.iter().map(|r| r.wall_clock_s).sum();
        total += wall;
        if !rs.is_empty() {
            lines.push_str(&format!(
                "\n{:<12} {:<9} {:<10.1} {}",
                t.id,
                rs.len(),
                wall,
                rs[0].model
            ));
        }
    }
    lines.push_str(&format!(
        "\nTOTAL: {:.1}s across {} receipt(s)",
        total,
        receipts.len()
    ));
    // Per-worker trust (ADR-12): measured win rate from receipt outcomes.
    let mut by_worker: Vec<(String, u64, u64)> = Vec::new();
    for r in &receipts {
        let e = by_worker.iter_mut().find(|(n, _, _)| n == &r.worker);
        let won = r.outcome == "merged";
        match e {
            Some((_, w, n)) => {
                *n += 1;
                if won {
                    *w += 1;
                }
            }
            None => by_worker.push((r.worker.clone(), u64::from(won), 1)),
        }
    }
    if !by_worker.is_empty() {
        by_worker.sort();
        lines.push_str(&format!(
            "\n\n{:<14} {:<11} {}",
            "WORKER", "WINS/TOTAL", "TRUST"
        ));
        for (name, w, n) in by_worker {
            lines.push_str(&format!(
                "\n{:<14} {:<11} {:.2}",
                name,
                format!("{w}/{n}"),
                w as f64 / n as f64
            ));
        }
    }
    lines
}

/// `af attach <id>`: tail a task's log until its terminal state.
pub fn attach(st: &Settings, id: &str) -> i32 {
    let store = Store::new(st.state_dir.clone());
    let log_path = store.log_dir().join(format!("{id}.log"));
    let mut pos: u64 = 0;
    loop {
        if let Ok(meta) = std::fs::metadata(&log_path) {
            let len = meta.len();
            if len > pos {
                if let Ok(mut f) = std::fs::File::open(&log_path) {
                    use std::io::{Read, Seek, SeekFrom};
                    let _ = f.seek(SeekFrom::Start(pos));
                    let mut buf = Vec::new();
                    let _ = f.read_to_end(&mut buf);
                    pos = len;
                    let _ = std::io::stdout().write_all(&buf);
                    let _ = std::io::stdout().flush();
                }
            }
        }
        match store.load().get(id).map(|s| &s.state) {
            Some(TaskState::Done) => return 0,
            Some(TaskState::Failed) => return 1,
            _ => std::thread::sleep(Duration::from_millis(300)),
        }
    }
}

/// `af api results --task <id>`: status + last log lines.
pub fn results(cfg: &Config, st: &Settings, id: &str) -> String {
    if !cfg.by_id.contains_key(id) {
        return format!("no such task: {id}");
    }
    let store = Store::new(st.state_dir.clone());
    let mut out = String::new();
    if let Some(s) = store.load().get(id) {
        out.push_str(&format!(
            "task {id}: state={:?}, attempts={}\n",
            s.state, s.attempts
        ));
        if let Some(e) = &s.last_error {
            out.push_str(&format!("result: {e}\n"));
        }
    }
    if let Ok(content) = std::fs::read_to_string(store.log_dir().join(format!("{id}.log"))) {
        let all: Vec<&str> = content.lines().collect();
        out.push_str("--- last log lines ---\n");
        for l in all[all.len().saturating_sub(30)..].iter() {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Task, WorkerDefaults};
    use crate::state::Receipt;
    use std::collections::BTreeMap;
    use std::process::Command;

    // --- hermetic fixtures -------------------------------------------------

    /// Unique per-test base directory. Tests derive their repo/state/worktree
    /// paths from this, so parallel tests never share a parent and cannot
    /// delete each other's scratch state.
    fn unique_base(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("af-run-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git_cmd(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn scratch_repo() -> PathBuf {
        let base = unique_base("repo");
        let dir = base.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        git_cmd(&dir, &["init", "-b", "main"]);
        // Repo-local identity: af's merge commits need one and parallel tests
        // must not race on the process environment.
        git_cmd(&dir, &["config", "user.name", "af test"]);
        git_cmd(&dir, &["config", "user.email", "af@test"]);
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        git_cmd(&dir, &["add", "."]);
        git_cmd(&dir, &["commit", "-m", "init"]);
        dir
    }

    fn cleanup(repo: &Path) {
        if let Some(base) = repo.parent() {
            let _ = std::fs::remove_dir_all(base);
        }
    }

    fn task(id: &str) -> Task {
        Task {
            id: id.into(),
            title: format!("task {id}"),
            accept: Some("true".into()),
            ..Default::default()
        }
    }

    fn cfg_with(tasks: Vec<Task>) -> Config {
        let by_id = tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();
        Config {
            tasks,
            workers: vec![],
            defaults: WorkerDefaults::default(),
            by_id,
            repos: BTreeMap::new(),
            warnings: vec![],
        }
    }

    fn settings(state_dir: PathBuf) -> Settings {
        Settings {
            repo_dir: state_dir.join("repo"),
            worktree_root: state_dir.join("worktrees"),
            state_dir,
            max_parallel: 1,
            branch_prefix: "tf".into(),
            poll_secs: 1,
            gate_env: vec![],
            tasks_file: PathBuf::new(),
            workers_file: PathBuf::new(),
            prompt_file: PathBuf::new(),
            agent_timeout_s: 3600,
            sandbox_cmd: vec![],
        }
    }

    fn running(attempts: u32, phase: Option<AttemptPhase>) -> TaskStatus {
        TaskStatus {
            state: TaskState::Running,
            attempts,
            last_error: None,
            phase,
        }
    }

    // --- git helpers -------------------------------------------------------

    #[test]
    fn branch_exists_and_merged_into_head_track_git_state() {
        let repo = scratch_repo();
        assert!(
            !branch_exists(&repo, "tf/T1"),
            "branch absent before creation"
        );
        assert!(!branch_merged_into_head(&repo, "tf/T1"));

        git_cmd(&repo, &["checkout", "-b", "tf/T1"]);
        std::fs::write(repo.join("f.txt"), "feature\n").unwrap();
        git_cmd(&repo, &["commit", "-am", "feature work"]);
        assert!(
            branch_exists(&repo, "tf/T1"),
            "branch visible after creation"
        );

        git_cmd(&repo, &["checkout", "main"]);
        assert!(
            !branch_merged_into_head(&repo, "tf/T1"),
            "feature branch is not an ancestor of main before merge"
        );
        git_cmd(&repo, &["merge", "--no-ff", "tf/T1", "-m", "merge tf/T1"]);
        assert!(
            branch_merged_into_head(&repo, "tf/T1"),
            "feature branch is an ancestor of HEAD after merge"
        );

        cleanup(&repo);
    }

    #[test]
    fn merge_commit_in_base_matches_only_this_tasks_af_merge_commit() {
        let repo = scratch_repo();
        assert!(!merge_commit_in_base(&repo, "T1"));

        // A sibling task's af merge commit must not satisfy T1.
        git_cmd(
            &repo,
            &["commit", "--allow-empty", "-m", "af: T2 — other work"],
        );
        assert!(!merge_commit_in_base(&repo, "T1"));
        assert!(merge_commit_in_base(&repo, "T2"));

        // T1's own merge commit is detected — the crash-after-merge window
        // where cleanup already deleted the branch but the work is in base.
        git_cmd(
            &repo,
            &["commit", "--allow-empty", "-m", "af: T1 — do the thing"],
        );
        assert!(merge_commit_in_base(&repo, "T1"));

        cleanup(&repo);
    }

    // --- status board / JSON ----------------------------------------------

    #[test]
    fn board_of_shows_every_task_state_attempts_and_blanks_missing_error() {
        let cfg = cfg_with(vec![task("A"), task("B"), task("C")]);
        let mut status = HashMap::new();
        status.insert(
            "A".into(),
            TaskStatus {
                state: TaskState::Done,
                attempts: 3,
                last_error: None,
                phase: None,
            },
        );
        status.insert(
            "B".into(),
            TaskStatus {
                state: TaskState::Failed,
                attempts: 2,
                last_error: Some("boom".into()),
                phase: None,
            },
        );
        let board = board_of(&cfg, &status);

        let cells = |id: &str| -> Vec<String> {
            board
                .lines()
                .find(|l| l.split_whitespace().next() == Some(id))
                .unwrap_or_else(|| panic!("no row for {id} in:\n{board}"))
                .split_whitespace()
                .map(str::to_string)
                .collect()
        };
        // Done row shows its attempt count and leaves LAST ERROR blank.
        assert_eq!(cells("A"), ["A", "Done", "3"]);
        // Failed row shows the error text.
        assert_eq!(cells("B"), ["B", "Failed", "2", "boom"]);
        // A task absent from the status map renders as the Ready default.
        assert_eq!(cells("C"), ["C", "Ready", "0"]);
    }

    #[test]
    fn status_json_serializes_task_fields_from_the_store() {
        let base = unique_base("json");
        let st = settings(base.clone());
        let cfg = cfg_with(vec![task("A"), task("B")]);
        let mut status = HashMap::new();
        status.insert(
            "A".into(),
            TaskStatus {
                state: TaskState::Failed,
                attempts: 2,
                last_error: Some("nope".into()),
                phase: None,
            },
        );
        Store::new(st.state_dir.clone()).save(&status).unwrap();

        let out = status_json(&cfg, &st);
        let v: serde_json::Value = serde_json::from_str(&out).expect("status_json is valid JSON");
        assert_eq!(v["A"]["id"], "A");
        assert_eq!(v["A"]["state"], "failed");
        assert_eq!(v["A"]["attempts"], 2);
        assert_eq!(v["A"]["last_error"], "nope");
        // Absent task falls back to the defaults.
        assert_eq!(v["B"]["state"], "ready");
        assert_eq!(v["B"]["attempts"], 0);
        assert!(v["B"]["last_error"].is_null());

        let _ = std::fs::remove_dir_all(&base);
    }

    // --- startup heal / resume --------------------------------------------

    #[test]
    fn heal_stale_attempt_reruns_unproven_agents_from_ready() {
        let base = unique_base("heal-reset");
        let st = settings(base.clone());
        let cfg = cfg_with(vec![task("A"), task("B")]);
        let mut status = HashMap::new();
        // None (legacy file / inter-attempt gap) and Spawned (no durable agent
        // outcome) both resume as a fresh attempt — never a gate/merge resume.
        status.insert("A".into(), running(1, None));
        status.insert("B".into(), running(2, Some(AttemptPhase::Spawned)));

        heal_stale_attempt(&cfg, &st, &mut status, "A", 3);
        heal_stale_attempt(&cfg, &st, &mut status, "B", 3);

        for id in ["A", "B"] {
            let s = &status[id];
            assert_eq!(s.state, TaskState::Ready, "{id} rewound to ready");
            assert_eq!(s.phase, None, "{id} journals from Spawned again");
            assert_eq!(
                s.last_error.as_deref(),
                Some("previous run interrupted"),
                "{id} records why it was rewound"
            );
        }
        assert_eq!(status["A"].attempts, 1, "attempt count is preserved");
        assert_eq!(status["B"].attempts, 2);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn heal_stale_attempt_agent_done_merges_without_rerunning_agent() {
        let repo = scratch_repo();
        let base = repo.parent().unwrap().to_path_buf();
        // The agent's durable work: a commit on the attempt branch.
        git_cmd(&repo, &["checkout", "-b", "tf/T1"]);
        std::fs::write(repo.join("f.txt"), "agent work\n").unwrap();
        git_cmd(&repo, &["commit", "-am", "agent work"]);
        git_cmd(&repo, &["checkout", "main"]);

        let mut st = settings(base.join("state"));
        st.repo_dir = repo.clone();
        let cfg = cfg_with(vec![task("T1")]);
        let mut status = HashMap::new();
        status.insert("T1".into(), running(1, Some(AttemptPhase::AgentDone)));

        heal_stale_attempt(&cfg, &st, &mut status, "T1", 3);

        // MergeOnly-of-GatePassed: gate re-run, merge landed, no agent re-run.
        assert_eq!(status["T1"].state, TaskState::Done);
        assert_eq!(status["T1"].phase, Some(AttemptPhase::GatePassed));
        assert_eq!(status["T1"].last_error, None);
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "agent work\n",
            "the committed agent work reached the base branch"
        );
        assert!(
            !branch_exists(&repo, "tf/T1"),
            "branch consumed by the merge"
        );

        cleanup(&repo);
    }

    #[test]
    fn mark_resumed_failure_is_terminal_only_at_max_attempts() {
        let mut status = HashMap::new();
        status.insert("A".into(), running(3, Some(AttemptPhase::GatePassed)));
        status.insert("B".into(), running(1, Some(AttemptPhase::GatePassed)));

        mark_resumed_failure(&mut status, "A", 3, "resumed acceptance gate failed");
        mark_resumed_failure(&mut status, "B", 3, "resumed acceptance gate failed");

        assert_eq!(
            status["A"].state,
            TaskState::Failed,
            "max attempts is terminal"
        );
        assert_eq!(status["B"].state, TaskState::Ready, "below max retries");
        assert_eq!(status["A"].phase, None, "next attempt restarts at Spawned");
        assert_eq!(
            status["A"].last_error.as_deref(),
            Some("resumed acceptance gate failed")
        );
    }

    // --- cost --------------------------------------------------------------

    #[test]
    fn cost_aggregates_wall_clock_and_filters_by_task() {
        let base = unique_base("cost");
        let st = settings(base.clone());
        let cfg = cfg_with(vec![task("A"), task("B")]);
        let store = Store::new(st.state_dir.clone());
        let receipt = |task: &str, wall: f64, outcome: &str| Receipt {
            task: task.into(),
            attempt: 1,
            worker: "w1".into(),
            model: "m".into(),
            wall_clock_s: wall,
            tokens: None,
            ts: 1,
            outcome: outcome.into(),
            error: None,
        };
        store.append_receipt(&receipt("A", 10.0, "merged")).unwrap();
        store.append_receipt(&receipt("A", 5.5, "failed")).unwrap();
        store.append_receipt(&receipt("B", 2.0, "merged")).unwrap();

        let all = cost(&cfg, &st, None);
        assert!(all.contains("TOTAL: 17.5s across 3 receipt(s)"), "{all}");

        let only_a = cost(&cfg, &st, Some("A"));
        assert!(
            only_a.contains("TOTAL: 15.5s across 3 receipt(s)"),
            "{only_a}"
        );
        assert!(
            !only_a
                .lines()
                .any(|l| l.split_whitespace().next() == Some("B")),
            "task B is filtered out:\n{only_a}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
