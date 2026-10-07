//! Run: the dispatch loop (poll → reap → dispatch) and query commands used
//! by the CLI (spec: cli, scheduling, state).

/// The `worker` recorded on a receipt when the worker genuinely is not
/// knowable. The startup heal writes it for an attempt that a killed
/// orchestrator left `running`: the task state does not persist which worker
/// was running, and inventing one would be a lie in the cost ledger. It is
/// agentflow admitting ignorance — NOT a worker name — so the cost report
/// must never treat it as a worker that went missing from the config.
pub const UNKNOWN_WORKER: &str = "unknown";

use crate::config::{Config, Settings, TaskState};
use crate::cost::{
    attempt_expense, basis as declared_basis, estimate_usd, size_ratio, Basis, Expense,
};
use crate::execute::{self, ExecCtx, Outcome};
use crate::gate;
use crate::router::Router;
use crate::scheduler;
use crate::state::{
    resume_action, AttemptPhase, Receipt, ResumeAction, Store, TaskStatus, OUTCOME_INTERRUPTED,
};
use crate::subprocess::{self, EnvMode};
use crate::worktree;
use std::collections::{BTreeMap, HashMap, HashSet};
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
///
/// `wait` is the wake-on-completion bound: when `Some(d)` the FIRST
/// receive blocks up to `d` (`recv_timeout`), so the dispatcher is woken
/// the moment an attempt finishes instead of idling through a poll
/// interval — a timeout still returns, so the loop's deadline
/// re-evaluation (deadlock detection, self-heal) keeps its schedule.
/// `None` keeps the drain-only semantics (`--once`'s fast 200ms poll).
///
/// The parameter list is deliberately a flat bundle of the attempt
/// bookkeeping shared verbatim by both call sites (main loop + `--once`);
/// grouping them into a context struct would cost more indirection than
/// the one extra argument saves.
#[allow(clippy::too_many_arguments)] // flat bookkeeping bundle, see above
fn reap(
    rx: &mpsc::Receiver<(String, String, Outcome, f64)>,
    running: &Arc<Mutex<HashMap<String, ()>>>,
    worker_busy: &Arc<Mutex<HashMap<String, bool>>>,
    router: &mut Router,
    status: &mut HashMap<String, TaskStatus>,
    store: &Store,
    max_attempts: u32,
    wait: Option<Duration>,
) {
    let first = match wait {
        Some(d) => rx.recv_timeout(d).ok(),
        None => rx.try_recv().ok(),
    };
    for (id, worker_name, outcome, wall_s) in first
        .into_iter()
        .chain(std::iter::from_fn(|| rx.try_recv().ok()))
    {
        running.lock().unwrap().remove(&id);
        worker_busy
            .lock()
            .unwrap()
            .insert(worker_name.clone(), false);
        // Every reaped attempt is a verdict (merged/failed) WITH its
        // measured wall-clock — the same span `execute_task` records on
        // the receipt — so the live duration stat matches a replay from
        // receipts.
        router.record(
            &worker_name,
            matches!(outcome, Outcome::Merged),
            Some(wall_s),
        );
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

/// Operator escape hatch for pre-dispatch reuse: `TF_NO_REUSE=1` (also
/// accepts `true`/`yes`) makes `af run` buy fresh agent work even when a
/// scope-compatible archived branch exists, for an operator who wants a
/// clean re-run. Anything else (including unset) leaves reuse ON — that is
/// the point of the optimisation.
fn reuse_disabled() -> bool {
    matches!(
        std::env::var("TF_NO_REUSE").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Why a pre-dispatch reuse candidate did not let the run skip the agent.
/// Every outcome except `Reused` falls through to the normal dispatch; the
/// variants exist so the loop can log the decision without re-deriving it.
enum ReuseOutcome {
    /// The archived branch re-passed scope + gate and is now merged.
    Reused,
    /// The newest archived branch changes files the CURRENT scope forbids.
    OutOfScope,
    /// The newest archived branch failed the acceptance gate.
    GateFailed,
    /// A git/worktree error prevented re-validation.
    Error(String),
}

/// Re-validate ONE archived rejected branch against the task's CURRENT scope
/// and gate and merge it when it passes — the pre-dispatch twin of
/// [`recover`]. The gate remains the SOLE arbiter, so this can only ever save
/// money, never accept work a fresh attempt would have had to redo.
///
/// On success the task is marked `Done`/`GatePassed` in `status` (the run
/// loop's in-memory map is the source of truth, so the caller's later saves
/// cannot clobber it) and the archived branch is consumed. On every failure
/// the temporary worktree is removed but the archived branch is LEFT ALONE
/// for `af clean` or a later `af recover`. No receipt is written: the
/// attempt that produced the archive already has one, and reuse is not a new
/// attempt (round 12's precedent).
fn reuse_archived_branch(
    cfg: &Config,
    st: &Settings,
    merge_locks: &worktree::MergeLocks,
    task: &crate::config::Task,
    branch: &str,
    status: &mut HashMap<String, TaskStatus>,
) -> ReuseOutcome {
    let repo = cfg.repo_dir_for(task, &st.repo_dir);
    let base_branch = match worktree::current_branch(&repo) {
        Ok(b) => b,
        Err(e) => return ReuseOutcome::Error(e),
    };
    let wt = match worktree::attach_existing(&repo, &st.worktree_root, &task.id, branch) {
        Ok(w) => w,
        Err(e) => return ReuseOutcome::Error(e),
    };
    // (a) SCOPE: every path the archived branch changes relative to the merge
    // base must be covered by the task's CURRENT scope (the operator widened
    // it). Reuses the SAME helpers the attempt path enforces with.
    let changed = match execute::changed_paths(&wt.path, &base_branch) {
        Ok(c) => c,
        Err(e) => {
            worktree::remove_worktree_only(&repo, &wt);
            return ReuseOutcome::Error(e);
        }
    };
    if !execute::scope_violations(&changed, &task.scope).is_empty() {
        worktree::remove_worktree_only(&repo, &wt);
        return ReuseOutcome::OutOfScope;
    }
    // (b) GATE: unless the task is manual, re-run the acceptance gate exactly
    // as the attempt path does (including the task's `gate_replay`). Nothing
    // is merged that the CURRENT gate does not pass.
    if !task.manual {
        if let Some(accept) = &task.accept {
            let gate_out = gate::run_accept(
                accept,
                &wt.path,
                &st.gate_env,
                Duration::from_secs(cfg.defaults.accept_timeout_s),
                task.gate_replay,
            );
            if !gate_out.passed() {
                worktree::remove_worktree_only(&repo, &wt);
                return ReuseOutcome::GateFailed;
            }
        }
    }
    // Success: merge with the standard message, mark the task done, persist,
    // and consume the archived branch (its content now lives in the base).
    let msg = format!("af: {} \u{2014} {}", task.id, task.title);
    if let Err(e) = worktree::merge(&repo, branch, merge_locks, &msg) {
        worktree::remove_worktree_only(&repo, &wt);
        return ReuseOutcome::Error(e);
    }
    let store = Store::new(st.state_dir.clone());
    let s = status.entry(task.id.clone()).or_default();
    s.state = TaskState::Done;
    s.last_error = None;
    s.phase = Some(AttemptPhase::GatePassed);
    let _ = store.save(status);
    worktree::remove(&repo, &wt);
    ReuseOutcome::Reused
}

/// Pre-dispatch reuse decision for one ready task: `true` means the task's
/// work was recovered from an archived branch and the caller must skip the
/// agent dispatch. At most ONE decision is made per task per run: an archive
/// created BY a failed attempt in this same run is the gate-only retry
/// machinery's business (scheduling spec), and this also keeps the WHY line
/// to one per attempt so a long campaign log stays readable.
fn reuse_before_dispatch(
    cfg: &Config,
    st: &Settings,
    merge_locks: &worktree::MergeLocks,
    task: &crate::config::Task,
    status: &mut HashMap<String, TaskStatus>,
    considered: &mut HashSet<String>,
) -> bool {
    if matches!(
        status.get(&task.id).map(|s| &s.state),
        Some(TaskState::Done)
    ) {
        // Defensive: a done task is never re-dispatched or reused.
        return false;
    }
    if !considered.insert(task.id.clone()) {
        // Already judged this task this run: never retry an archive, and
        // never re-log the reason.
        return false;
    }
    let repo = cfg.repo_dir_for(task, &st.repo_dir);
    let branches = worktree::archived_branches(&repo, &st.branch_prefix);
    let Some(branch) = worktree::newest_archived_branch(&branches, &st.branch_prefix, &task.id)
    else {
        println!(
            "  \u{21ba} {} no archived branch to reuse (searched {}/{}-rejected-<ts>); dispatching agent",
            task.id, st.branch_prefix, task.id
        );
        return false;
    };
    match reuse_archived_branch(cfg, st, merge_locks, task, &branch, status) {
        ReuseOutcome::Reused => {
            println!(
                "\u{21ba} {} reused archived branch {} (scope allows it; gate re-run, agent not re-run)",
                task.id, branch
            );
            true
        }
        ReuseOutcome::OutOfScope => {
            println!(
                "  \u{21ba} {} archived branch {} rejected: out of scope; dispatching agent",
                task.id, branch
            );
            false
        }
        ReuseOutcome::GateFailed => {
            println!(
                "  \u{21ba} {} archived branch {} rejected: gate failed; dispatching agent",
                task.id, branch
            );
            false
        }
        ReuseOutcome::Error(e) => {
            println!(
                "  \u{21ba} {} archived branch {} not reused: {}; dispatching agent",
                task.id, branch, e
            );
            false
        }
    }
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
    let retry_delay_s = cfg.defaults.retry_delay_s;
    // --once dispatches no second attempt, so it never paces one either
    // (the wait would only delay the exit).
    let pace_retries = !opts.once;

    // --- self-heal: remove orphan worktrees from dead attempts ---
    // A corrupt ledger is an ERROR, never a fresh campaign: refusing here is
    // what stops a torn state file from silently re-buying every task.
    let mut status = match store.load_checked() {
        Ok(m) => m,
        Err(err) => {
            eprintln!("config error: {err}");
            return 2;
        }
    };
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

    let (tx, rx) = mpsc::channel::<(String, String, Outcome, f64)>();
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
    // Pre-dispatch archived-branch reuse (r13-auto-recover): ON by default,
    // switchable off with TF_NO_REUSE=1. `reuse_considered` holds the task
    // ids already judged this run, so a task's archive is considered at most
    // once, a rejected candidate is never retried, and its reason is logged
    // at most once.
    let reuse_enabled = !reuse_disabled();
    let mut reuse_considered: HashSet<String> = HashSet::new();
    // Measured routing (ADR-12): replay receipts into per-worker stats.
    // Interrupted attempts are NOT worker verdicts (r9-interrupted): a kill
    // that says nothing about the worker must not dent its rate, so filter
    // them out at the call site before they reach the router.
    let verdicts: Vec<Receipt> = store
        .load_receipts()
        .into_iter()
        .filter(Receipt::counts_as_verdict)
        .collect();
    let mut router = Router::from_receipts(&verdicts);
    loop {
        // Reap finished tasks — BLOCKING while attempts are in flight, so
        // a completion wakes the dispatcher immediately instead of it
        // idling through the poll interval. poll_secs stays the UPPER
        // BOUND on how long the loop may idle before re-evaluating
        // (deadlock detection, self-heal and edge cases still re-check on
        // schedule). With nothing in flight no completion can arrive (an
        // attempt is registered in `running` before its thread is
        // spawned), so drain without waiting and re-evaluate at once.
        let wait = if running.lock().unwrap().is_empty() {
            None
        } else {
            Some(Duration::from_secs(poll.max(1)))
        };
        reap(
            &rx,
            &running,
            &worker_busy,
            &mut router,
            &mut status,
            &store,
            max_attempts,
            wait,
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
        // Cost ceiling (r8-budget-cap): measured fresh from the receipts on
        // every round so the meter sees prior runs and restarts too. 0
        // disables the ceiling entirely — the legacy behaviour.
        let measured = if st.max_wall_clock_s == 0 {
            0.0
        } else {
            consumed_wall_clock_s(&store.load_receipts(), &in_scope)
        };
        let over_budget = wall_clock_budget_exhausted(st, measured);

        let running_ids: Vec<String> = running.lock().unwrap().keys().cloned().collect();
        if running_ids.is_empty() {
            // Nothing in flight: progress is possible only if something is
            // ready. Otherwise this is a deadlock (failed/absent deps, or a
            // --task target waiting on out-of-scope work).
            let ready_in_scope = scheduler::ready_tasks(cfg, &status, &[], max_attempts)
                .into_iter()
                .filter(|t| in_scope.iter().any(|s| s.id == t.id))
                .count();
            // Budget stop TERMINATES here: a ready-but-undispatched task
            // would otherwise keep the "progress is possible" check happy
            // forever, spinning the loop. Completion above already won, so
            // reaching this point means work genuinely remains.
            if over_budget && ready_in_scope > 0 {
                let not_started = in_scope
                    .iter()
                    .filter(|t| {
                        matches!(
                            status.get(&t.id).map(|s| &s.state),
                            None | Some(TaskState::Ready)
                        )
                    })
                    .count();
                eprintln!(
                    "wall-clock budget exhausted: spent {measured:.1}s of {}s cap; {not_started} in-scope task(s) not started",
                    st.max_wall_clock_s
                );
                return 3;
            }
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

        // Dispatch a round — never while the ceiling is exhausted: attempts
        // already in flight are allowed to finish, only NEW ones are blocked.
        if !over_budget && running.lock().unwrap().len() < max_parallel {
            let running_now = running.lock().unwrap().keys().cloned().collect::<Vec<_>>();
            let ready = scheduler::ready_tasks(cfg, &status, &running_now, max_attempts);
            for t in ready {
                if let Some(f) = &opts.task_filter {
                    if &t.id != f {
                        continue;
                    }
                }
                if matches!(status.get(&t.id).map(|s| &s.state), Some(TaskState::Done)) {
                    continue;
                }
                if running.lock().unwrap().len() >= max_parallel {
                    break;
                }
                // PRE-DISPATCH REUSE: a rejected attempt's work is already on
                // disk under an archived branch, so re-buying it from a fresh
                // agent is pure waste. Checked before a worker is picked
                // (reuse consumes no worker slot); the gate still decides.
                if reuse_enabled
                    && reuse_before_dispatch(
                        cfg,
                        st,
                        &merge_locks,
                        &t,
                        &mut status,
                        &mut reuse_considered,
                    )
                {
                    continue;
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
                    // Attempt identity (r11-attempt-identity): same save,
                    // same window. A crash after this point still leaves
                    // behind WHO ran the attempt and WHEN it started, so
                    // the startup heal can name the worker and measure an
                    // upper-bound duration instead of a placeholder.
                    s.attempt_started_ts = Some(crate::state::now_ts());
                    s.attempt_worker = Some(worker.name.clone());
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
                    // The attempt's wall-clock, measured around the same
                    // span `execute_task` records on its receipt
                    // (`wall_clock_s`), so the router's live duration stat
                    // is the same statistic a replay from receipts would
                    // build. The retry-pacing wait below is deliberately
                    // OUTSIDE the measurement — it is post-attempt backoff,
                    // not work.
                    let t0 = std::time::Instant::now();
                    let out = execute::execute_task(&ctx2, &wclone, &id, attempt, &log_path);
                    let wall_s = t0.elapsed().as_secs_f64();
                    // Retry pacing (`retry_delay_s`, default 0): after a
                    // FAILED attempt that will be retried — `attempt <
                    // max_attempts`; reap will count this attempt and send
                    // the task back to Ready — wait the configured backoff
                    // before reporting the outcome, so the next attempt
                    // cannot start until it has elapsed. The wait lives in
                    // THIS attempt's thread, on the retry path ONLY:
                    //   * a first attempt is never delayed (it starts on
                    //     dispatch, and a task that merges first try never
                    //     reaches this branch), and
                    //   * the dispatcher loop never sleeps for it — round 8
                    //     removed the global poll sleep, and a global wait
                    //     would stall every worker slot and re-introduce
                    //     that idle time. With several workers in flight
                    //     the wait belongs to the retrying task, so the
                    //     other slots keep dispatching.
                    if pace_retries
                        && retry_delay_s > 0
                        && attempt < max_attempts
                        && matches!(out, Outcome::Failed(_))
                    {
                        // Observable: one clear line naming the task and
                        // the seconds, in the run log and the task's own
                        // log, so a puzzling pause is explainable.
                        println!(
                            "  \u{23f8} {id} attempt {attempt} failed — pacing retry: waiting {retry_delay_s}s (retry_delay_s)"
                        );
                        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log_path)
                        {
                            let _ = writeln!(
                                f,
                                "af: pacing retry — waiting {retry_delay_s}s (retry_delay_s) before attempt {}",
                                attempt + 1
                            );
                        }
                        std::thread::sleep(Duration::from_secs(retry_delay_s));
                    }
                    let _ = tx2.send((id, wname, out, wall_s));
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
                        None,
                    );
                    std::thread::sleep(Duration::from_millis(200));
                }
                println!("{}", board_of(cfg, &status));
                return 0;
            }
        }

        // No trailing sleep: the next iteration's reap blocks on the
        // completion channel while work is in flight (waking the instant
        // an attempt finishes) and bounds its wait by poll_secs otherwise.
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
/// `pub(crate)` so `execute_attempt`'s belt-and-braces merge verification
/// ("only report merged when the base really contains the work") reuses the
/// exact same ancestor check as the resume machinery.
pub(crate) fn branch_merged_into_head(repo: &Path, branch: &str) -> bool {
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
/// The budget check sees a fully-consumed count: `heal_stale_attempt` has
/// already counted the interrupted attempt before dispatching the resume.
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
/// RerunAgent: fresh attempt from Ready (the stale worktree is replaced
/// when the retry is dispatched). RerunGate / MergeOnly: finish only the
/// remaining effects, never re-invoking the agent.
///
/// The interrupted attempt consumes the retry budget. `attempts` counts
/// STARTED attempts, and exactly two writers keep that true: `reap`
/// increments when an attempt reports its outcome, and this heal increments
/// for an attempt that was started (dispatch persisted `running` with
/// attempt number `attempts + 1`) but died before `reap` could run. Each
/// started attempt is counted exactly once because a reaped attempt is no
/// longer `running` — only one of the two writers can ever fire for it.
/// Without this, healing rewound the task to Ready for free, so a crash
/// loop (OOM kill, reboot, kill -9) could re-buy the agent forever without
/// ever reaching `max_attempts`. Receipt numbering stays coherent: the
/// dead attempt's receipts (if any) carry the number it was dispatched
/// with, and the next dispatch computes `attempts + 1` on the incremented
/// count — a strictly greater number — so `af cost` never double-counts
/// one started attempt under two numbers.
fn heal_stale_attempt(
    cfg: &Config,
    st: &Settings,
    status: &mut HashMap<String, TaskStatus>,
    id: &str,
    max_attempts: u32,
) {
    // Count the interrupted attempt BEFORE any resume decision, so every
    // resume outcome — the reset below, and `mark_resumed_failure`'s
    // budget check inside the gate/merge resumes — sees the budget as
    // already consumed by the interrupted attempt.
    if let Some(s) = status.get_mut(id) {
        s.attempts += 1;
    }
    let action = resume_action(status.get(id).and_then(|s| s.phase));
    let handled = match action {
        ResumeAction::RerunGate => resume_gate_only(cfg, st, status, id, max_attempts),
        ResumeAction::MergeOnly => resume_merge_only(cfg, st, status, id, max_attempts),
        ResumeAction::RerunAgent => false, // handled by the reset below
    };
    if !handled {
        // Record the lost attempt (r9-interrupted): heal could not resume
        // its durable agent outcome, so it produced no verdict and the
        // process died. Attempt identity (r11-attempt-identity): dispatch
        // persisted WHO ran the attempt and WHEN it started, so the
        // receipt tells the truth when they are known — the persisted
        // worker and its configured model, plus a duration measured from
        // dispatch. That duration is an UPPER BOUND, not a measurement of
        // the attempt: the exit time is unknowable, only the start is
        // durable. The receipt keeps the attempt visible in `af cost`
        // (interrupted, not a worker loss). An older state file (or a
        // crash before the first dispatch save) has neither field and
        // keeps the exact pre-change placeholder: `unknown`/0.0s.
        // The attempt number is the one dispatch persisted
        // (`attempts + 1`), exactly the incremented count above.
        // NOTE: a successful gate/merge resume is NOT lost — its agent work
        // is durable and finishes as merged — so it records no receipt here.
        let store = Store::new(st.state_dir.clone());
        let attempt = status.get(id).map(|s| s.attempts).unwrap_or(0);
        let identity = status
            .get(id)
            .map(|s| (s.attempt_started_ts, s.attempt_worker.clone()));
        let (started_ts, worker_name) = identity.unwrap_or((None, None));
        // The worker: the one dispatch chose, when that choice survived.
        let worker = worker_name
            .clone()
            .unwrap_or_else(|| UNKNOWN_WORKER.to_string());
        // The model: ONLY the configured worker's — a healed attempt must
        // never claim a model nobody recorded (a worker since removed from
        // the config keeps the placeholder for the model).
        let model = worker_name
            .as_deref()
            .and_then(|name| cfg.workers.iter().find(|w| w.name == name))
            .map(|w| w.model.clone())
            .unwrap_or_else(|| UNKNOWN_WORKER.to_string());
        let (wall_clock_s, error) = match started_ts {
            Some(ts) => (
                crate::state::now_ts().saturating_sub(ts) as f64,
                "orchestrator exited mid-attempt; duration is time since dispatch (the exit time is unknown, so this is an upper bound)"
                    .to_string(),
            ),
            None => (
                0.0,
                "orchestrator exited mid-attempt; attempt duration unknown".to_string(),
            ),
        };
        let _ = store.append_receipt(&Receipt {
            task: id.to_string(),
            attempt,
            worker,
            model,
            wall_clock_s,
            tokens: None,
            ts: crate::state::now_ts(),
            outcome: OUTCOME_INTERRUPTED.to_string(),
            error: Some(error),
            cost_micros: None,
        });
        // RerunAgent, or a resume that lost its artifacts (always safe):
        // apply the budget to the now-counted attempt — Ready for a fresh
        // agent attempt while budget remains (the stale worktree is
        // dropped by worktree::create when the retry is dispatched), or
        // terminal Failed once the crash loop has started `max_attempts`
        // attempts, so it can never re-buy the agent again.
        if let Some(s) = status.get_mut(id) {
            if s.attempts >= max_attempts {
                s.state = TaskState::Failed;
                s.last_error = Some(format!(
                    "previous run interrupted; retry budget exhausted ({}/{max_attempts})",
                    s.attempts
                ));
            } else {
                s.state = TaskState::Ready;
                s.last_error = Some("previous run interrupted".into());
            }
            s.phase = None; // next attempt journals from Spawned again
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
    // Round 11 leaves `<prefix>/<id>-rejected-<ts>[-<n>]` refs behind; sweep
    // them too. The running-task rule is identical to the worktree rule.
    let rejected = worktree::clean_rejected(&repo_list, &st.branch_prefix, &running, dry_run);
    if ids.is_empty() && rejected.is_empty() {
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
    for branch in &rejected {
        if dry_run {
            println!("would remove branch {branch}");
        } else {
            println!("removed branch {branch}");
        }
    }
    if !ids.is_empty() {
        if dry_run {
            println!(
                "dry run: {} orphan(s) would be removed; nothing changed",
                ids.len()
            );
        } else {
            println!("clean: removed {} orphan(s)", ids.len());
        }
    }
    0
}

/// `af recover --task ID [--dry-run]`: re-validate an archived rejected
/// branch — the copy round 11's failure paths keep under
/// `<prefix>/<id>-rejected-<ts>[-<n>]` — against the task's CURRENT scope
/// and gate, and merge it into the base when it passes. The agent is never
/// re-invoked: the operator already paid for this work once; recovery just
/// un-blocks it when the reason it was rejected is gone.
///
/// Selection picks the NEWEST archived branch for `id` by parsing the
/// numeric `<ts>` (then the `-<n>` suffix), never by git order or dates.
///
/// Exit codes: 0 merged / dry-run reported; 1 re-validation failed (scope
/// violation or gate failure), with the archived branch AND its worktree
/// handled so no failure leaves a worktree behind (the archived branch itself
/// survives a rejection); 2 unknown task or no archived branch to recover
/// (nothing to recover is not a failure).
pub fn recover(cfg: &Config, st: &Settings, id: &str, dry_run: bool) -> i32 {
    let Some(task) = cfg.by_id.get(id).cloned() else {
        eprintln!("config error: unknown task '{id}'");
        return 2;
    };
    let repo = cfg.repo_dir_for(&task, &st.repo_dir);
    let branches = worktree::archived_branches(&repo, &st.branch_prefix);
    let Some(branch) = worktree::newest_archived_branch(&branches, &st.branch_prefix, id) else {
        eprintln!("no archived branch for task '{id}' (searched <prefix>/<id>-rejected-<ts>)");
        return 2;
    };

    // --dry-run: selection only — report exactly what recovery WOULD do and
    // touch nothing (no worktree, no gate run, no merge).
    if dry_run {
        println!("would recover {branch} to done (agent not re-run)");
        return 0;
    }

    // Re-validation, in order, before any merge. The archived branch is
    // checked out in a throwaway worktree so both the scope diff and the
    // acceptance gate run against its committed state, exactly as an
    // attempt path would.
    let base_branch = match worktree::current_branch(&repo) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let wt = match worktree::attach_existing(&repo, &st.worktree_root, id, &branch) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: cannot check out {branch}: {e}");
            return 1;
        }
    };
    // (a) SCOPE: every path the archived branch changes relative to the base
    // must be covered by the task's CURRENT scope (the whole point: the
    // operator widened it). An empty scope allows any file.
    let changed = match execute::changed_paths(&wt.path, &base_branch) {
        Ok(c) => c,
        Err(e) => {
            worktree::remove_worktree_only(&repo, &wt);
            eprintln!("error: {e}");
            return 1;
        }
    };
    let violations = execute::scope_violations(&changed, &task.scope);
    if !violations.is_empty() {
        eprintln!(
            "recover {id}: attempt edited files out of scope: {} (allowed: {})",
            violations.join(", "),
            task.scope.join(", ")
        );
        worktree::remove_worktree_only(&repo, &wt);
        return 1;
    }
    // (b) GATE: unless the task is manual, re-run the acceptance gate exactly
    // as the attempt path does (including the task's `gate_replay`). A
    // non-zero gate rejects the recovery.
    if !task.manual {
        if let Some(accept) = &task.accept {
            let gate_out = gate::run_accept(
                accept,
                &wt.path,
                &st.gate_env,
                Duration::from_secs(cfg.defaults.accept_timeout_s),
                task.gate_replay,
            );
            if !gate_out.passed() {
                eprintln!(
                    "acceptance gate failed (exit {}): {}",
                    gate_out
                        .code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "-".into()),
                    gate_out.combined().trim()
                );
                worktree::remove_worktree_only(&repo, &wt);
                return 1;
            }
        }
    }
    // Success path: merge the archived branch into the base with the standard
    // merge message, mark the task Done (phase = GatePassed), persist it, and
    // remove the recovered worktree + the archived branch (its content is now
    // in the base — leaving it behind would be a ghost for `af clean`).
    let msg = format!("af: {} — {}", task.id, task.title);
    if let Err(e) = worktree::merge(&repo, &branch, &worktree::MergeLocks::new(), &msg) {
        worktree::remove_worktree_only(&repo, &wt);
        eprintln!("error: {e}");
        return 1;
    }
    let store = Store::new(st.state_dir.clone());
    let mut status = store.load();
    let s = status.entry(id.to_string()).or_default();
    s.state = TaskState::Done;
    s.last_error = None;
    s.phase = Some(AttemptPhase::GatePassed);
    let _ = store.save(&status);
    worktree::remove(&repo, &wt);
    println!("✓ {id} recovered to done (agent not re-run)");
    0
}

pub fn dry_run(cfg: &Config, st: &Settings) -> i32 {
    let depths = scheduler::compute_depths(cfg).unwrap_or_default();
    let store = Store::new(st.state_dir.clone());
    // Same strict read as the real run: the displayed plan must never
    // disagree with what a run would do, so a corrupt ledger exits 2 too.
    let status = match store.load_checked() {
        Ok(m) => m,
        Err(err) => {
            eprintln!("config error: {err}");
            return 2;
        }
    };
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
    // Budget (r8-budget-cap): surface an exhausted ceiling in the plan too —
    // dry-run still creates nothing and still exits 0.
    if st.max_wall_clock_s > 0 {
        let in_scope: Vec<&crate::config::Task> = cfg.tasks.iter().collect();
        let measured = consumed_wall_clock_s(&store.load_receipts(), &in_scope);
        if wall_clock_budget_exhausted(st, measured) {
            println!(
                "  BUDGET: spent {measured:.1}s of {}s cap (TF_MAX_WALL_CLOCK_S) — a run would stop before dispatching new attempts",
                st.max_wall_clock_s
            );
        }
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
///
/// The status file is loaded loudly by [`Store::load`] already; receipts are
/// not needed for the board but ARE loaded through the checked loader so a
/// torn `*.json` receipt surfaces here too (one `warning:` line per file)
/// instead of staying invisible. History that cannot be parsed never blocks
/// the command.
pub fn status_board(cfg: &Config, st: &Settings) -> String {
    let store = Store::new(st.state_dir.clone());
    let status = store.load();
    let (_, problems) = store.load_receipts_checked();
    let mut out = String::new();
    for p in &problems {
        out.push_str(&format!("warning: {p}\n"));
    }
    out.push_str(&board_of(cfg, &status));
    out
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

/// Selection window for `af cost` (spec: state/cost-receipts — "aggregate
/// receipts (last run, since date, or per task)"). The three fields
/// compose: `since` keeps only receipts at/after the instant, `last` then
/// keeps only each task's most recent surviving receipt, and `task`
/// narrows the table rows. With every field unset the report covers every
/// loaded receipt — byte-identical to the pre-window-flag `af cost`.
#[derive(Debug, Clone, Default)]
pub struct CostFilter {
    pub task: Option<String>,
    pub last: bool,
    pub since: Option<u64>,
}

/// Parse a `--since` value: a bare unix timestamp (`1700000000`) or a
/// `YYYY-MM-DD` date, interpreted as UTC midnight of that day. The result
/// is the inclusive lower bound for [`Receipt::ts`].
pub fn parse_since(value: &str) -> Result<u64, String> {
    let v = value.trim();
    if let Ok(ts) = v.parse::<u64>() {
        return Ok(ts);
    }
    let bad = || format!("cannot parse '{value}' as a unix timestamp or YYYY-MM-DD date");
    let parts: Vec<&str> = v.split('-').collect();
    let (y, m, d) = match parts.as_slice() {
        [y, m, d] => match (y.parse::<i64>(), m.parse::<u32>(), d.parse::<u32>()) {
            (Ok(y), Ok(m), Ok(d)) => (y, m, d),
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    };
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return Err(format!("date {value} is not a valid calendar date"));
    }
    let days = days_from_civil(y, m, d);
    if days < 0 {
        return Err(format!("date {value} is before the unix epoch"));
    }
    Ok(days as u64 * 86_400)
}

/// Days since 1970-01-01 for a civil (calendar) date — Howard Hinnant's
/// `days_from_civil`, valid over the whole proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (i64::from(m) + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Length of month `m` in year `y` (callers validate `m` first; the
/// catch-all answers 31 for January, March, May, July, August, October,
/// December — and any month outside 1-12).
fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(y) => 29,
        2 => 28,
        _ => 31,
    }
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Apply the `--since` / `--last` window to loaded receipts. `since` keeps
/// receipts with `ts >= since` (applied FIRST, so `--last --since D` means
/// "the latest attempt per task since D"); `last` keeps, per task, only
/// the receipt with the greatest `ts` — ties broken by the greater
/// `attempt` — so retries are never double-counted. The `task` filter is
/// deliberately NOT applied here: it narrows the table rows only, exactly
/// as it did before the window flags existed.
fn select_receipts<'a>(receipts: &'a [Receipt], filter: &CostFilter) -> Vec<&'a Receipt> {
    let mut window: Vec<&Receipt> = match filter.since {
        Some(ts) => receipts.iter().filter(|r| r.ts >= ts).collect(),
        None => receipts.iter().collect(),
    };
    if filter.last {
        // `window` is sorted by `ts` (load_receipts), but ties are broken
        // by `attempt`, so selection is independent of scan order.
        let mut latest: HashMap<&str, &Receipt> = HashMap::new();
        for r in &window {
            let keep = match latest.get(r.task.as_str()) {
                Some(cur) => (r.ts, r.attempt) > (cur.ts, cur.attempt),
                None => true,
            };
            if keep {
                latest.insert(r.task.as_str(), r);
            }
        }
        window = latest.into_values().collect();
        window.sort_by_key(|r| r.ts);
    }
    window
}

/// Campaign budget meter (r8-budget-cap): the wall-clock seconds already
/// consumed by the IN-SCOPE tasks — read through the same loader and the
/// same window selection `af cost` uses, so the enforced ceiling is exactly
/// the TOTAL that report shows. The receipts are the persisted counter
/// (they survive restarts), so no second field has to be kept in sync.
fn consumed_wall_clock_s(receipts: &[Receipt], in_scope: &[&crate::config::Task]) -> f64 {
    select_receipts(receipts, &CostFilter::default())
        .into_iter()
        .filter(|r| in_scope.iter().any(|t| t.id == r.task))
        .map(|r| r.wall_clock_s)
        .sum()
}

/// Is the campaign's measured wall-clock spend at or above its ceiling?
/// `max_wall_clock_s == 0` disables the ceiling (unlimited, legacy).
fn wall_clock_budget_exhausted(st: &Settings, measured: f64) -> bool {
    st.max_wall_clock_s > 0 && measured >= st.max_wall_clock_s as f64
}

/// Stable grouping key for a failed attempt's CAUSE: the text before the
/// first `:` in `error`, trimmed, with runs of whitespace collapsed to one
/// space. The full `error` embeds a LIST of files after the colon, so
/// truncating the whole string split one cause across several rows and cut
/// words in half; the prefix groups by cause. An `error` with no colon uses
/// the whole string; `unknown` when the receipt carries no `error` (legacy
/// or failed-without-detail) or nothing precedes the colon.
fn waste_cause(r: &Receipt) -> String {
    match &r.error {
        None => "unknown".to_string(),
        Some(e) => {
            let head = e.split(':').next().unwrap_or("");
            let cause = head.split_whitespace().collect::<Vec<_>>().join(" ");
            if cause.is_empty() {
                "unknown".to_string()
            } else {
                cause
            }
        }
    }
}

/// Per-reason sub-count inside one cause: (full reason, seconds, attempts).
type ReasonCount = (String, f64, usize);
/// One aggregated cause row: (cause, seconds, attempts, per-reason sub-counts).
type CauseAggregate = (String, f64, usize, Vec<ReasonCount>);

/// Every worker in `cfg.workers` paired with its DECLARED basis
/// ([`declared_basis`]): a worker present in the config but declaring
/// nothing keeps an entry (`None`), so it stays distinguishable from a
/// worker ABSENT from the config (no entry at all) — both render `-` in
/// the COST column, but only the absent one is footnoted.
fn worker_bases(cfg: &Config) -> Vec<(String, Option<Basis>)> {
    cfg.workers
        .iter()
        .map(|w| {
            (
                w.name.clone(),
                declared_basis(w.params_b, w.price_per_mtok_usd),
            )
        })
        .collect()
}

/// The DECLARED bases of every worker — the reference set
/// [`size_ratio`] measures a `Sized` worker against. Because it spans
/// EVERY worker in `cfg.workers` (not just those with receipts), the
/// CHEAPEST declaring worker is always exactly `1.00x` in the report,
/// whatever the selected window.
fn declared_bases(bases: &[(String, Option<Basis>)]) -> Vec<Basis> {
    bases.iter().filter_map(|(_, b)| *b).collect()
}

/// The basis a receipt's worker runs on: `None` when the worker declares
/// neither field OR is absent from `cfg.workers` entirely.
fn basis_of(worker: &str, bases: &[(String, Option<Basis>)]) -> Option<Basis> {
    bases
        .iter()
        .find(|(name, _)| name == worker)
        .and_then(|(_, b)| *b)
}

/// The ONE basis line printed above the tables, so a relative proxy can
/// never be misread as money. A MEASURED cost is real money and outranks
/// every declared basis: when any selected receipt carries one, the line
/// names the provider as the source (`provider-reported (USD)`) and counts
/// the attempts that still ride an estimate from a declared price, so a
/// mixed report explains its own `~` markers. With no measured cost the
/// line is chosen from the DECLARATIONS alone — byte-identical to round
/// 10: any worker declaring a price puts the report on the prices basis
/// (with a note when some other worker declares only `params_b`), else the
/// `params_b` proxy, else an honest `none`.
fn basis_line(
    bases: &[(String, Option<Basis>)],
    selected: &[&Receipt],
    all_declared: &[Basis],
) -> String {
    let measured = selected.iter().filter(|r| r.cost_micros.is_some()).count();
    if measured > 0 {
        let mut line = "cost basis: provider-reported (USD)".to_string();
        let estimated = selected
            .iter()
            .filter(|r| {
                matches!(
                    attempt_expense(
                        r.cost_micros,
                        basis_of(&r.worker, bases),
                        r.tokens,
                        all_declared
                    ),
                    Some(Expense::Usd {
                        estimated: true,
                        ..
                    })
                )
            })
            .count();
        if estimated > 0 {
            line.push_str(&format!(
                "; {estimated} of {} attempt(s) estimated from a declared price",
                selected.len()
            ));
        }
        return line;
    }
    let any_priced = bases
        .iter()
        .any(|(_, b)| matches!(b, Some(Basis::Priced(_))));
    let any_sized = bases
        .iter()
        .any(|(_, b)| matches!(b, Some(Basis::Sized(_))));
    if any_priced {
        let mut line = "cost basis: declared prices (USD per 1M tokens)".to_string();
        if any_sized {
            line.push_str("; some workers declare only params_b");
        }
        line
    } else if any_sized {
        "cost basis: params_b proxy (relative; cheapest declared worker = 1.00x)".to_string()
    } else {
        "cost basis: none (no worker declares params_b or price_per_mtok_usd)".to_string()
    }
}

/// The COST cell for one WORKER row. A DECLARED basis always renders
/// something — the rate a worker declares is a property of the WORKER, not
/// of whether its receipts happened to record tokens (the default output
/// mode records none), so a token-less worker must not blank the cell:
///
/// * declared `price_per_mtok_usd` + tokens → real dollars (`$` + 4
///   decimals) for the worker's whole recorded spend, via [`estimate_usd`];
/// * declared `price_per_mtok_usd`, no tokens recorded → the declared RATE,
///   unit-suffixed (`$0.6000/Mtok`) so it can never be misread as a spend;
/// * declared `params_b` → a relative RATE (`N.NNx`, 2 decimals — a PROXY,
///   never a `$`) via [`size_ratio`], whatever the token count;
/// * nothing declared (or absent from `cfg.workers`) → `-`.
fn worker_cost_cell(basis: Option<Basis>, tokens: Option<u64>, all_declared: &[Basis]) -> String {
    match basis {
        Some(Basis::Sized(params)) => {
            size_ratio(params, all_declared).map_or_else(|| "-".into(), |r| format!("{r:.2}x"))
        }
        Some(Basis::Priced(price)) => match tokens {
            Some(t) => format!("${:.4}", estimate_usd(t, price)),
            None => format!("${price:.4}/Mtok"),
        },
        None => "-".into(),
    }
}

/// The COST cell for one TASK row, which spans every selected attempt of
/// the task — possibly on DIFFERENT workers:
///
/// * every attempt `Priced` → the SUM of the per-attempt dollars;
/// * every attempt `Sized` → the TOKEN-WEIGHTED MEAN rate ratio
///   (Σ tokens·ratio ÷ Σ tokens), the blend the row's tokens actually ran;
/// * anything else — a worker declaring nothing, a receipt naming a worker
///   absent from `cfg.workers`, or a receipt with no `tokens` (legacy) —
///   makes the whole row `-`: one unknown term must not silently
///   understate a figure that reads as exact.
fn task_cost_cell(
    rs: &[&Receipt],
    bases: &[(String, Option<Basis>)],
    all_declared: &[Basis],
) -> String {
    let mut dollars = 0.0; // exact only when every attempt is Priced
    let mut weighted = 0.0; // Σ tokens·ratio — exact only when every attempt is Sized
    let mut tokens = 0.0;
    let mut priced = true;
    let mut sized = true;
    for r in rs {
        match basis_of(&r.worker, bases) {
            Some(Basis::Priced(price)) => {
                sized = false;
                match r.tokens {
                    Some(t) => {
                        dollars += estimate_usd(t, price);
                        tokens += t as f64;
                    }
                    None => return "-".into(),
                }
            }
            Some(Basis::Sized(params)) => {
                priced = false;
                match (r.tokens, size_ratio(params, all_declared)) {
                    (Some(t), Some(ratio)) => {
                        weighted += t as f64 * ratio;
                        tokens += t as f64;
                    }
                    _ => return "-".into(),
                }
            }
            None => return "-".into(),
        }
    }
    if priced {
        format!("${dollars:.4}")
    } else if sized && tokens > 0.0 {
        format!("{:.2}x", weighted / tokens)
    } else {
        "-".into()
    }
}

/// A row's folded expense on a report that carries at least one MEASURED
/// cost: dollars when EVERY attempt folds to dollars (measured or
/// estimated), the token-weighted mean ratio when every attempt folds to
/// a ratio. `None` (rendered `-`) when the attempts are incommensurable —
/// dollars and a parameter ratio must never be converted into each other —
/// or any attempt's expense is unknown (no basis, no tokens, an absent
/// worker): one unknown term must not silently understate a figure that
/// reads as exact.
enum RowExpense {
    Usd { micros: u64, estimated: bool },
    Ratio(f64),
}

fn row_expense(
    rs: &[&Receipt],
    basis_of_attempt: impl Fn(&Receipt) -> Option<Basis>,
    all_declared: &[Basis],
) -> Option<RowExpense> {
    let mut micros: u64 = 0;
    let mut estimated = false;
    let mut any_usd = false;
    let mut weighted = 0.0; // Σ tokens·ratio
    let mut tokens = 0.0;
    let mut any_ratio = false;
    for r in rs {
        match attempt_expense(r.cost_micros, basis_of_attempt(r), r.tokens, all_declared)? {
            Expense::Usd {
                micros: m,
                estimated: e,
            } => {
                any_usd = true;
                micros = micros.saturating_add(m);
                estimated |= e;
            }
            // The ladder guarantees tokens here; a defensive 0 weight can
            // only understate a proxy, never invent one.
            Expense::Ratio(x) => {
                any_ratio = true;
                let t = r.tokens.unwrap_or(0) as f64;
                weighted += t * x;
                tokens += t;
            }
        }
    }
    // A measured dollar amount and a parameter RATIO are incommensurable.
    if any_usd && any_ratio {
        return None;
    }
    if any_usd {
        Some(RowExpense::Usd { micros, estimated })
    } else if tokens > 0.0 {
        Some(RowExpense::Ratio(weighted / tokens))
    } else {
        None
    }
}

/// Render one folded [`RowExpense`] as a COST cell: measured dollars carry
/// no marker; dollars containing ANY estimated term carry `~` so an
/// assumption is never presented as a measurement; a ratio is `N.NNx`,
/// never a `$`.
fn render_row_expense(e: &RowExpense) -> String {
    match e {
        RowExpense::Usd { micros, estimated } => format!(
            "{}${:.4}",
            if *estimated { "~" } else { "" },
            *micros as f64 / 1_000_000.0
        ),
        RowExpense::Ratio(r) => format!("{r:.2}x"),
    }
}

/// The COST cell for one TASK row on a measured report — same position and
/// placeholder rules as [`task_cost_cell`], but folded through the
/// truthfulness ladder ([`row_expense`]) so a provider-reported cost
/// outranks the declared basis for its own attempt.
fn measured_task_cell(
    rs: &[&Receipt],
    bases: &[(String, Option<Basis>)],
    all_declared: &[Basis],
) -> String {
    row_expense(rs, |r| basis_of(&r.worker, bases), all_declared)
        .as_ref()
        .map_or_else(|| "-".into(), render_row_expense)
}

/// The COST cell for one WORKER row on a measured report. Same ladder as
/// the task rows — a worker's measured receipts sum as dollars, mixed with
/// an estimate they are `~`-marked — with one worker-specific fallback: a
/// worker none of whose receipts carries a measured cost and none of which
/// folds keeps round 10's DECLARED-RATE display (a declared rate is a
/// property of the worker, not of the tokens that happened to be
/// recorded — see [`worker_cost_cell`]).
fn measured_worker_cell(
    rs: &[&Receipt],
    basis: Option<Basis>,
    tokens: Option<u64>,
    all_declared: &[Basis],
) -> String {
    match row_expense(rs, |_| basis, all_declared) {
        Some(e) => render_row_expense(&e),
        None => {
            if rs.iter().all(|r| r.cost_micros.is_none()) {
                worker_cost_cell(basis, tokens, all_declared)
            } else {
                "-".into()
            }
        }
    }
}

/// The MEAN_S cell for one WORKER row: the mean wall-clock seconds over
/// the worker's VERDICT attempts in the selection — the same statistic the
/// router's duration tie-break reads ([`Router::mean_duration_s`]),
/// printed so the choice is inspectable. ONE decimal; `-` when the worker
/// has no verdict attempt in the selection (no duration is never a zero
/// duration — a row itself only exists once one does).
fn mean_duration_cell(verdicts: &[&Receipt]) -> String {
    if verdicts.is_empty() {
        "-".into()
    } else {
        format!(
            "{:.1}",
            verdicts.iter().map(|r| r.wall_clock_s).sum::<f64>() / verdicts.len() as f64
        )
    }
}

/// `af cost`: aggregate receipts (wall-clock truth, ADR-9). The table rows
/// honor `filter.task`; the TOTAL line and the per-worker trust block are
/// computed over the window-selected receipts only — with no window flags
/// that is every loaded receipt, byte-identical to the legacy report.
pub fn cost(cfg: &Config, st: &Settings, filter: &CostFilter) -> String {
    let store = Store::new(st.state_dir.clone());
    // Checked loader: a torn receipt is reported (one `warning:` line per
    // unreadable file, naming it and the parse problem) but never blocks the
    // report — the readable history is still totaled. With no torn files the
    // output is byte-identical to the legacy report.
    let (receipts, problems) = store.load_receipts_checked();
    let selected = select_receipts(&receipts, filter);
    let mut lines = String::new();
    for p in &problems {
        lines.push_str(&format!("warning: {p}\n"));
    }
    // Cost basis (r10-cost-report): one line ABOVE the tables states the
    // units of the COST column before any number is read, so a relative
    // proxy can never be misread as money. `all_declared` spans EVERY
    // worker in cfg.workers, so the cheapest declaring worker is always
    // exactly `1.00x` on the proxy scale. (r11-measured-cost) when any
    // selected receipt carries a provider-reported cost, the same flag
    // switches BOTH tables' cells to the truthfulness ladder — measured
    // money first — and the basis line names the measured source; with no
    // measured cost anywhere the report stays byte-identical to round 10.
    let bases = worker_bases(cfg);
    let all_declared = declared_bases(&bases);
    let report_measured = selected.iter().any(|r| r.cost_micros.is_some());
    lines.push_str(&basis_line(&bases, &selected, &all_declared));
    lines.push('\n');
    lines.push_str(&format!(
        "{:<12} {:<9} {:<10} {:<9} {:<8} {}",
        "TASK", "ATTEMPTS", "WALL_S", "TOKENS", "COST", "MODEL"
    ));
    let mut total: f64 = 0.0;
    for t in &cfg.tasks {
        if let Some(f) = &filter.task {
            if &t.id != f {
                continue;
            }
        }
        let rs: Vec<_> = selected
            .iter()
            .copied()
            .filter(|r| r.task == t.id)
            .collect();
        let wall: f64 = rs.iter().map(|r| r.wall_clock_s).sum();
        total += wall;
        if !rs.is_empty() {
            // Sum the tokens actually recorded; legacy receipts (every one
            // of which carries `None`) show `-` so the legacy report reads
            // exactly as it did before the column existed.
            let mut token_sum: Option<u64> = None;
            for r in &rs {
                if let Some(t) = r.tokens {
                    token_sum = Some(token_sum.unwrap_or(0) + t);
                }
            }
            let tokens = token_sum
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into());
            // Estimated expense for the whole row (dollars when every
            // attempt is Priced, the token-weighted mean rate when every
            // attempt is Sized, `-` otherwise). On a measured report the
            // same cell is folded through the ladder instead: a
            // provider-reported cost outranks the declared basis for its
            // own attempt.
            let cost_cell = if report_measured {
                measured_task_cell(&rs, &bases, &all_declared)
            } else {
                task_cost_cell(&rs, &bases, &all_declared)
            };
            lines.push_str(&format!(
                "\n{:<12} {:<9} {:<10.1} {:<9} {:<8} {}",
                t.id,
                rs.len(),
                wall,
                tokens,
                cost_cell,
                rs[0].model
            ));
        }
    }
    lines.push_str(&format!(
        "\nTOTAL: {:.1}s across {} receipt(s)",
        total,
        selected.len()
    ));
    // Per-worker trust (ADR-12): measured win rate from the SELECTED
    // receipt VERDICTS. Interrupted receipts (r9-interrupted) are excluded
    // from wins AND total: they carry no information about the worker.
    let mut by_worker: Vec<(String, u64, u64)> = Vec::new();
    // The worker's summed recorded tokens over the same selected VERDICT
    // receipts that form its row — the input to its COST cell. `None`
    // until a receipt actually carries tokens (legacy receipts never do).
    let mut worker_tokens: HashMap<String, Option<u64>> = HashMap::new();
    // The same receipts themselves, grouped per worker: a measured report
    // folds its COST cell attempt-by-attempt (a provider-reported cost is
    // a per-attempt fact), which the summed tokens alone cannot express.
    let mut worker_receipts: HashMap<String, Vec<&Receipt>> = HashMap::new();
    for r in selected.iter().copied().filter(|r| r.counts_as_verdict()) {
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
        let t = worker_tokens.entry(r.worker.clone()).or_insert(None);
        if let Some(v) = r.tokens {
            *t = Some(t.unwrap_or(0) + v);
        }
        worker_receipts.entry(r.worker.clone()).or_default().push(r);
    }
    if !by_worker.is_empty() {
        by_worker.sort();
        lines.push_str(&format!(
            "\n\n{:<14} {:<11} {} {} {}",
            "WORKER", "WINS/TOTAL", "TRUST", "MEAN_S", "COST"
        ));
        for (name, w, n) in by_worker {
            // Estimated expense for the worker's whole recorded token spend
            // on the selected window: dollars from a declared price, a
            // relative rate from `params_b`, `-` when neither is declared,
            // the worker is absent from the config, or no tokens were ever
            // recorded (legacy receipts). On a measured report the cell is
            // folded attempt-by-attempt instead: measured dollars first,
            // `~` when any term is estimated.
            let worker_rs: &[&Receipt] = worker_receipts
                .get(name.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let cost_cell = if report_measured {
                measured_worker_cell(
                    worker_rs,
                    basis_of(&name, &bases),
                    worker_tokens.get(name.as_str()).copied().flatten(),
                    &all_declared,
                )
            } else {
                worker_cost_cell(
                    basis_of(&name, &bases),
                    worker_tokens.get(name.as_str()).copied().flatten(),
                    &all_declared,
                )
            };
            lines.push_str(&format!(
                "\n{:<14} {:<11} {:.2} {} {}",
                name,
                format!("{w}/{n}"),
                w as f64 / n as f64,
                mean_duration_cell(worker_rs),
                cost_cell
            ));
        }
    }
    // Wasted spend (failed attempts): computed from the SAME window-selected
    // receipts as the rest of the report, so `--last` / `--since` / `--task`
    // narrow it too. A failed attempt's wall-clock is otherwise
    // indistinguishable from productive spend — this is the only place it is
    // reported.
    let failed: Vec<&Receipt> = selected
        .iter()
        .copied()
        .filter(|r| r.outcome == "failed")
        .collect();
    let mut wasted: f64 = 0.0;
    for r in &failed {
        wasted += r.wall_clock_s;
    }
    let pct = if selected.is_empty() {
        0.0
    } else {
        failed.len() as f64 / selected.len() as f64 * 100.0
    };
    lines.push_str(&format!(
        "\nWASTED: {:.1}s on {} of {} attempt(s) ({:.1}%)",
        wasted,
        failed.len(),
        selected.len(),
        pct
    ));
    // Interrupted attempts (r9-interrupted): real spend with an UNKNOWN
    // duration (recorded as 0.0s), so they get their own outcome line
    // instead of being buried in the failed subtotal or silently dropped.
    // They stay out of the trust block above — no verdict on the worker.
    let interrupted: Vec<&Receipt> = selected
        .iter()
        .copied()
        .filter(|r| r.outcome == OUTCOME_INTERRUPTED)
        .collect();
    if !interrupted.is_empty() {
        let secs: f64 = interrupted.iter().map(|r| r.wall_clock_s).sum();
        lines.push_str(&format!(
            "\nINTERRUPTED: {:.1}s on {} attempt(s) — duration unknown (orchestrator exited mid-attempt)",
            secs,
            interrupted.len()
        ));
    }
    if !failed.is_empty() {
        // Primary grouping is by CAUSE (text before the first `:`), never by
        // the full reason: the reason embeds a file LIST, so two failures of
        // the same cause used to land in separate rows. Each cause also keeps
        // its distinct full reasons as an indented sub-count — that is where
        // the file lists stay visible without fragmenting the total.
        type SubCounts = HashMap<String, (f64, usize)>;
        let mut by_cause: HashMap<String, (f64, usize, SubCounts)> = HashMap::new();
        for r in &failed {
            let cause = waste_cause(r);
            let full = r.error.clone().unwrap_or_else(|| "unknown".to_string());
            let entry = by_cause
                .entry(cause)
                .or_insert_with(|| (0.0, 0, HashMap::new()));
            entry.0 += r.wall_clock_s;
            entry.1 += 1;
            let sub = entry.2.entry(full).or_insert((0.0, 0));
            sub.0 += r.wall_clock_s;
            sub.1 += 1;
        }
        let mut rows: Vec<CauseAggregate> = by_cause
            .into_iter()
            .map(|(cause, (secs, n, subs))| {
                let mut sub_rows: Vec<ReasonCount> = subs
                    .into_iter()
                    .map(|(reason, (s, c))| (reason, s, c))
                    .collect();
                // Descending seconds; ties broken by the full reason ascending
                // so the sub-counts are deterministic too.
                sub_rows.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.0.cmp(&b.0))
                });
                (cause, secs, n, sub_rows)
            })
            .collect();
        // Descending seconds; ties broken by the cause string ascending.
        rows.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        lines.push_str("\nWASTED BY REASON");
        for (cause, secs, n, subs) in rows {
            lines.push_str(&format!("\n  {:<28} {:.1}s  ({})", cause, secs, n));
            for (reason, rsecs, rn) in subs {
                // When the error carries no colon the cause IS the full
                // reason; a sub-row would only repeat the line above.
                if reason != cause {
                    lines.push_str(&format!("\n    - {} {:.1}s  ({})", reason, rsecs, rn));
                }
            }
        }
    }
    // Footnote (r10-cost-report): receipts naming a worker ABSENT from
    // cfg.workers render as `-`; this line names them (sorted, deduped)
    // with the attempt count. Historical receipts routinely outlive config
    // edits, so this is a NOTE — never an error, never blocking the report.
    let mut unknown: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &selected {
        // `UNKNOWN_WORKER` is the heal's placeholder for "the worker of this
        // killed attempt is unknowable", not a worker that vanished from
        // the config: the INTERRUPTED line already accounts for that
        // attempt. The same holds for an interrupted receipt that names a
        // real worker (r11-attempt-identity: dispatch persists the choice)
        // — an attempt lost to a killed orchestrator is never evidence of
        // a worker missing from the config, measured duration or not.
        if r.counts_as_verdict()
            && r.worker != UNKNOWN_WORKER
            && !bases.iter().any(|(name, _)| name == &r.worker)
        {
            *unknown.entry(r.worker.as_str()).or_insert(0) += 1;
        }
    }
    let unknown_attempts: usize = unknown.values().sum();
    if unknown_attempts > 0 {
        let names = unknown.keys().copied().collect::<Vec<_>>().join(", ");
        lines.push_str(&format!(
            "\nnote: {unknown_attempts} attempt(s) name a worker absent from the config, shown as '-': {names}"
        ));
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

    /// BELT AND BRACES (r13): `Merged` is only trusted after the attempt
    /// branch tip is verified to be an ancestor of the base. Verify the exact
    /// check `execute_attempt` uses: false for a branch with unmerged work,
    /// true once that work is merged.
    // spec: lifecycle/an-attempt-that-produces-no-change-is-not-merged#a-reported-merge-is-verified-in-the-base
    #[test]
    fn branch_merged_into_head_is_true_only_after_the_merge() {
        let repo = scratch_repo();
        // A tip equal to HEAD is already an ancestor (nothing to merge).
        git_cmd(&repo, &["branch", "done"]);
        assert!(
            branch_merged_into_head(&repo, "done"),
            "an equal tip is an ancestor"
        );
        // A branch carrying real work is NOT an ancestor until merged.
        git_cmd(&repo, &["checkout", "-b", "work", "main"]);
        std::fs::write(repo.join("work.txt"), "x\n").unwrap();
        git_cmd(&repo, &["add", "work.txt"]);
        git_cmd(&repo, &["commit", "-m", "work"]);
        git_cmd(&repo, &["checkout", "main"]);
        assert!(
            !branch_merged_into_head(&repo, "work"),
            "unmerged work is not an ancestor"
        );
        // After `git merge --no-ff` the tip IS an ancestor of the base.
        git_cmd(&repo, &["merge", "--no-ff", "work", "-m", "af: T — merge"]);
        assert!(
            branch_merged_into_head(&repo, "work"),
            "a merged branch is an ancestor of the base"
        );
        cleanup(&repo);
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
        cfg_with_workers(tasks, Vec::new())
    }

    /// A config that also DECLARES its workers — the cost report resolves
    /// every COST cell (and the absent-worker footnote) against them.
    fn cfg_with_workers(tasks: Vec<Task>, workers: Vec<crate::config::Worker>) -> Config {
        let by_id = tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();
        Config {
            tasks,
            workers,
            defaults: WorkerDefaults::default(),
            by_id,
            repos: BTreeMap::new(),
            warnings: vec![],
        }
    }

    /// A worker with no declared cost basis (neutral — COST cells `-`).
    fn worker_named(name: &str) -> crate::config::Worker {
        crate::config::Worker {
            name: name.into(),
            ..Default::default()
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
            agent_stall_s: 0,
            max_wall_clock_s: 0,
            sandbox_cmd: vec![],
        }
    }

    fn running(attempts: u32, phase: Option<AttemptPhase>) -> TaskStatus {
        TaskStatus {
            state: TaskState::Running,
            attempts,
            last_error: None,
            phase,
            attempt_started_ts: None,
            attempt_worker: None,
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
                attempt_started_ts: None,
                attempt_worker: None,
            },
        );
        status.insert(
            "B".into(),
            TaskStatus {
                state: TaskState::Failed,
                attempts: 2,
                last_error: Some("boom".into()),
                phase: None,
                attempt_started_ts: None,
                attempt_worker: None,
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
                attempt_started_ts: None,
                attempt_worker: None,
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

        // max_attempts 4 (was 3): the interrupted attempts now consume one
        // attempt of budget each on heal, so the fixture needs a spare slot
        // to keep exercising the rewind-to-Ready path for both entries.
        heal_stale_attempt(&cfg, &st, &mut status, "A", 4);
        heal_stale_attempt(&cfg, &st, &mut status, "B", 4);

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
        assert_eq!(
            status["A"].attempts, 2,
            "the interrupted attempt is counted (1 -> 2)"
        );
        assert_eq!(
            status["B"].attempts, 3,
            "the interrupted attempt is counted (2 -> 3)"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn interrupted_attempt_consumes_the_retry_budget() {
        let base = unique_base("heal-budget");
        let st = settings(base.clone());
        let cfg = cfg_with(vec![task("A"), task("B")]);

        // Budget exhausted: a stale running entry whose attempts already sit
        // at max_attempts (3) heals to terminal Failed, never Ready —
        // otherwise a crash loop re-buys the agent forever.
        let mut status = HashMap::new();
        status.insert("A".into(), running(3, None));
        heal_stale_attempt(&cfg, &st, &mut status, "A", 3);
        assert_eq!(status["A"].state, TaskState::Failed, "no budget left");
        assert_eq!(
            status["A"].attempts, 4,
            "the interrupted attempt is counted"
        );
        let err = status["A"].last_error.as_deref().unwrap_or_default();
        assert!(
            err.contains("interrupted") && err.contains("exhausted"),
            "the error says the run was interrupted and the budget spent: {err}"
        );

        // Budget remaining: the interrupted attempt still counts — healing
        // rewinds to Ready at 2 attempts, not a free rewind to 1.
        status.insert("B".into(), running(1, None));
        heal_stale_attempt(&cfg, &st, &mut status, "B", 3);
        assert_eq!(status["B"].state, TaskState::Ready, "budget remains");
        assert_eq!(
            status["B"].attempts, 2,
            "the interrupted attempt is counted"
        );
        assert_eq!(
            status["B"].last_error.as_deref(),
            Some("previous run interrupted")
        );

        // Convergence: repeatedly crash the same task (set the entry back to
        // running, heal). The count climbs by exactly one per started
        // attempt — 1, 2 — and the budget-exhausting third crash lands on
        // Failed at the budget instead of being re-dispatched indefinitely.
        let mut status = HashMap::new();
        status.insert("B".into(), running(0, None));
        for expected in [1u32, 2] {
            heal_stale_attempt(&cfg, &st, &mut status, "B", 3);
            assert_eq!(
                status["B"].attempts, expected,
                "exactly one counted attempt per crash window"
            );
            assert_eq!(status["B"].state, TaskState::Ready, "budget remains");
            status.insert("B".into(), running(expected, None)); // crash again
        }
        heal_stale_attempt(&cfg, &st, &mut status, "B", 3);
        assert_eq!(
            status["B"].attempts, 3,
            "the count never exceeds max_attempts"
        );
        assert_eq!(
            status["B"].state,
            TaskState::Failed,
            "converges to Failed at the budget, not an infinite re-buy"
        );

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
        let cfg = cfg_with_workers(vec![task("A"), task("B")], vec![worker_named("w1")]);
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
            cost_micros: None,
        };
        store.append_receipt(&receipt("A", 10.0, "merged")).unwrap();
        store.append_receipt(&receipt("A", 5.5, "failed")).unwrap();
        store.append_receipt(&receipt("B", 2.0, "merged")).unwrap();

        // No window flags: byte-identical legacy report — every receipt in
        // the TOTAL count and the trust block, retries double-counted.
        let all = cost(&cfg, &st, &CostFilter::default());
        assert!(all.contains("TOTAL: 17.5s across 3 receipt(s)"), "{all}");

        let only_a = cost(
            &cfg,
            &st,
            &CostFilter {
                task: Some("A".into()),
                ..Default::default()
            },
        );
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

    #[test]
    fn waste_cause_is_text_before_the_first_colon_or_unknown() {
        let r = |error: Option<&str>| Receipt {
            task: "A".into(),
            attempt: 1,
            worker: "w".into(),
            model: "m".into(),
            wall_clock_s: 1.0,
            tokens: None,
            ts: 0,
            outcome: "failed".into(),
            error: error.map(str::to_string),
            cost_micros: None,
        };
        // No error on the receipt → the literal key.
        assert_eq!(waste_cause(&r(None)), "unknown");
        // Text before the first `:`, trimmed, whitespace collapsed: the file
        // list AFTER the colon is not part of the cause.
        assert_eq!(
            waste_cause(&r(Some("  gate failed (exit 1): boom\nextra detail"))),
            "gate failed (exit 1)"
        );
        assert_eq!(
            waste_cause(&r(Some(
                "attempt   edited files out of scope:   src/run.rs, tests/other.rs"
            ))),
            "attempt edited files out of scope"
        );
        // No colon → the whole (trimmed) string, never truncated mid-word.
        let long = "x".repeat(60);
        assert_eq!(waste_cause(&r(Some(&long))), "x".repeat(60));
        // Nothing before the colon → the literal key, not an empty row.
        assert_eq!(waste_cause(&r(Some(": boom"))), "unknown");
    }

    #[test]
    fn parse_since_accepts_timestamps_and_utc_midnight_dates() {
        assert_eq!(parse_since("1700000000").unwrap(), 1_700_000_000);
        assert_eq!(
            parse_since(" 1700000000 ").unwrap(),
            1_700_000_000,
            "surrounding whitespace is tolerated"
        );
        assert_eq!(parse_since("1970-01-01").unwrap(), 0, "the epoch");
        assert_eq!(parse_since("1970-01-02").unwrap(), 86_400);
        assert_eq!(parse_since("2023-01-01").unwrap(), 1_672_531_200);
        assert_eq!(
            parse_since("2024-02-29").unwrap(),
            1_709_164_800,
            "leap day"
        );
        assert_eq!(
            parse_since("2000-02-29").unwrap(),
            951_782_400,
            "400-year leap"
        );
        assert_eq!(
            parse_since("2024-04-30").unwrap(),
            1_714_435_200,
            "30-day month"
        );
    }

    #[test]
    fn parse_since_rejects_garbage_and_impossible_dates() {
        for bad in [
            "",
            "not-a-date",
            "1700000000x",
            "2024-1",
            "2024-13-01",
            "2024-02-30",
            "2023-02-29",
            "1969-12-31",
            "-1",
        ] {
            let err = parse_since(bad).unwrap_err();
            assert!(
                bad.is_empty() || err.contains(bad),
                "error names the bad value: {bad} → {err}"
            );
        }
    }

    #[test]
    fn cost_last_keeps_the_latest_attempt_per_task_without_double_counting() {
        let base = unique_base("cost-last");
        let st = settings(base.clone());
        // w1/w2 are declared (no cost basis) so the report's COST cells are
        // `-` and no absent-worker footnote distracts from the window
        // semantics under test.
        let cfg = cfg_with_workers(
            vec![task("A"), task("B"), task("C")],
            vec![worker_named("w1"), worker_named("w2")],
        );
        let store = Store::new(st.state_dir.clone());
        let rcpt =
            |task: &str, attempt: u32, ts: u64, wall: f64, worker: &str, outcome: &str| Receipt {
                task: task.into(),
                attempt,
                worker: worker.into(),
                model: "m".into(),
                wall_clock_s: wall,
                tokens: None,
                ts,
                outcome: outcome.into(),
                error: None,
                cost_micros: None,
            };
        // A: a failed retry then a merge on another worker. B: a same-ts
        // tie between attempts 1/2 — the greater attempt must win. C: two
        // identical (ts, attempt) receipts — exactly one is kept.
        store
            .append_receipt(&rcpt("A", 1, 100, 30.0, "w1", "failed"))
            .unwrap();
        store
            .append_receipt(&rcpt("A", 2, 200, 12.0, "w2", "merged"))
            .unwrap();
        store
            .append_receipt(&rcpt("B", 1, 300, 7.0, "w1", "merged"))
            .unwrap();
        store
            .append_receipt(&rcpt("B", 2, 300, 3.0, "w1", "failed"))
            .unwrap();
        store
            .append_receipt(&rcpt("C", 1, 400, 2.0, "w1", "merged"))
            .unwrap();
        store
            .append_receipt(&rcpt("C", 1, 400, 2.0, "w1", "merged"))
            .unwrap();

        // No flags: legacy totals — every attempt, trust over every receipt.
        let all = cost(&cfg, &st, &CostFilter::default());
        assert!(all.contains("TOTAL: 56.0s across 6 receipt(s)"), "{all}");
        assert!(all.contains(&format!("{:<12} {:<9}", "A", 2)), "{all}");
        assert!(
            all.contains(&format!("{:<14} {:<11} {:.2}", "w1", "3/5", 0.60)),
            "{all}"
        );

        // --last: ONE receipt per task — greatest ts, ties by greater
        // attempt — so retries are not double-counted.
        let last = cost(
            &cfg,
            &st,
            &CostFilter {
                last: true,
                ..Default::default()
            },
        );
        assert!(last.contains("TOTAL: 17.0s across 3 receipt(s)"), "{last}");
        assert!(
            last.contains(&format!("{:<12} {:<9} {:<10.1}", "A", 1, 12.0)),
            "{last}"
        );
        // B's tie: attempt 2 (3.0s, failed) wins over attempt 1 (7.0s,
        // merged) — the row AND the trust block prove it.
        assert!(
            last.contains(&format!("{:<12} {:<9} {:<10.1}", "B", 1, 3.0)),
            "{last}"
        );
        assert!(
            !last.contains("30.0"),
            "A's retry is not double-counted: {last}"
        );
        // Trust from the SELECTED receipts only: w1 keeps B@2 (failed) and
        // C@1 (merged) → 1/2; w2 keeps A's merge → 1/1.
        assert!(
            last.contains(&format!("{:<14} {:<11} {:.2}", "w1", "1/2", 0.50)),
            "{last}"
        );
        assert!(
            last.contains(&format!("{:<14} {:<11} {:.2}", "w2", "1/1", 1.00)),
            "{last}"
        );

        // --last composes with --task: rows narrow to A, the window still
        // spans the latest-per-task selection (legacy count semantics).
        let last_a = cost(
            &cfg,
            &st,
            &CostFilter {
                task: Some("A".into()),
                last: true,
                ..Default::default()
            },
        );
        assert!(
            last_a.contains("TOTAL: 12.0s across 3 receipt(s)"),
            "{last_a}"
        );
        assert!(
            !last_a
                .lines()
                .any(|l| l.split_whitespace().next() == Some("B")),
            "{last_a}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cost_since_windows_receipts_and_composes_with_last() {
        let base = unique_base("cost-since");
        let st = settings(base.clone());
        let cfg = cfg_with_workers(
            vec![task("A")],
            vec![worker_named("w1"), worker_named("w2")],
        );
        let store = Store::new(st.state_dir.clone());
        let rcpt = |attempt: u32, ts: u64, wall: f64, worker: &str, outcome: &str| Receipt {
            task: "A".into(),
            attempt,
            worker: worker.into(),
            model: "m".into(),
            wall_clock_s: wall,
            tokens: None,
            ts,
            outcome: outcome.into(),
            error: None,
            cost_micros: None,
        };
        store
            .append_receipt(&rcpt(1, 1_600_000_000, 100.0, "w1", "failed"))
            .unwrap(); // 2020-09-13
        store
            .append_receipt(&rcpt(2, 1_700_000_000, 20.0, "w1", "failed"))
            .unwrap(); // 2023-11-14
        store
            .append_receipt(&rcpt(3, 1_750_000_000, 30.0, "w2", "merged"))
            .unwrap();

        // Bare timestamp window: older receipts excluded, newer kept.
        let since = cost(
            &cfg,
            &st,
            &CostFilter {
                since: Some(1_650_000_000),
                ..Default::default()
            },
        );
        assert!(
            since.contains("TOTAL: 50.0s across 2 receipt(s)"),
            "{since}"
        );
        assert!(!since.contains("100.0"), "2020 receipt excluded: {since}");
        assert!(
            since.contains(&format!("{:<14} {:<11} {:.2}", "w1", "0/1", 0.00)),
            "trust reflects the window: {since}"
        );

        // The bound is inclusive: ts >= since.
        let edge = cost(
            &cfg,
            &st,
            &CostFilter {
                since: Some(1_600_000_000),
                ..Default::default()
            },
        );
        assert!(edge.contains("TOTAL: 150.0s across 3 receipt(s)"), "{edge}");

        // --since --last: the latest attempt per task WITHIN the window.
        let both = cost(
            &cfg,
            &st,
            &CostFilter {
                last: true,
                since: Some(1_650_000_000),
                ..Default::default()
            },
        );
        assert!(both.contains("TOTAL: 30.0s across 1 receipt(s)"), "{both}");
        assert!(both.contains(&format!("{:<12} {:<9}", "A", 1)), "{both}");
        assert!(
            both.contains(&format!("{:<14} {:<11} {:.2}", "w2", "1/1", 1.00)),
            "{both}"
        );
        assert!(
            !both.contains("w1"),
            "w1's windowed-out attempts are gone: {both}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // --- cost cells (r10-cost-report) -------------------------------------

    #[test]
    fn mean_duration_cell_formats_the_mean_over_verdict_attempts() {
        let rcpt = |wall: f64, outcome: &str| Receipt {
            task: "A".into(),
            attempt: 1,
            worker: "w1".into(),
            model: "m".into(),
            wall_clock_s: wall,
            tokens: None,
            ts: 1,
            outcome: outcome.into(),
            error: None,
            cost_micros: None,
        };
        // The mean over the verdict attempts, one decimal.
        let merged = rcpt(2.0, "merged");
        let failed = rcpt(6.0, "failed");
        assert_eq!(mean_duration_cell(&[&merged, &failed]), "4.0");
        assert_eq!(mean_duration_cell(&[&merged]), "2.0");
        // No verdict attempt: unknown, never zero.
        assert_eq!(mean_duration_cell(&[]), "-");
    }

    #[test]
    fn worker_cost_cell_renders_dollars_ratios_and_placeholders() {
        // Cheapest declared Sized basis is 4B, so 8B runs at exactly 2x.
        let all = vec![Basis::Sized(4.0), Basis::Sized(8.0)];
        // Real money: 500k tokens at $2/Mtok, four decimals.
        assert_eq!(
            worker_cost_cell(Some(Basis::Priced(2.0)), Some(500_000), &all),
            "$1.0000"
        );
        // Proxy RATE relative to the cheapest declared basis — `x`, never `$`.
        assert_eq!(
            worker_cost_cell(Some(Basis::Sized(4.0)), Some(1), &all),
            "1.00x"
        );
        assert_eq!(
            worker_cost_cell(Some(Basis::Sized(8.0)), Some(1), &all),
            "2.00x"
        );
        // A DECLARED basis is never blanked by a missing token count: the
        // default output mode records none, and the rate is a property of the
        // worker — otherwise a text-mode campaign shows `-` in every cell, the
        // very question the column answers. A zero-token sum keeps its rate
        // too: it is a rate, not a spend.
        assert_eq!(
            worker_cost_cell(Some(Basis::Sized(8.0)), None, &all),
            "2.00x"
        );
        assert_eq!(
            worker_cost_cell(Some(Basis::Sized(8.0)), Some(0), &all),
            "2.00x"
        );
        // With no tokens a declared price shows the declared RATE — with a
        // unit, so it can never be misread as a spend figure.
        assert_eq!(
            worker_cost_cell(Some(Basis::Priced(2.0)), None, &all),
            "$2.0000/Mtok"
        );
        // No declared basis (or a worker absent from the config): `-`.
        assert_eq!(worker_cost_cell(None, Some(1), &all), "-");
        assert_eq!(worker_cost_cell(None, None, &all), "-");
        // No Sized basis to relate to: never invent a reference rate.
        assert_eq!(worker_cost_cell(Some(Basis::Sized(8.0)), Some(1), &[]), "-");
    }

    #[test]
    fn task_cost_cell_sums_dollars_blends_ratios_or_degrades() {
        let bases = vec![
            ("pw".to_string(), Some(Basis::Priced(2.0))),
            ("sw".to_string(), Some(Basis::Sized(8.0))),
            ("cw".to_string(), Some(Basis::Sized(4.0))),
            ("nw".to_string(), None),
        ];
        let all = vec![Basis::Sized(4.0), Basis::Sized(8.0)];
        let rcpt = |worker: &str, tokens: Option<u64>| Receipt {
            task: "A".into(),
            attempt: 1,
            worker: worker.into(),
            model: "m".into(),
            wall_clock_s: 1.0,
            tokens,
            ts: 1,
            outcome: "merged".into(),
            error: None,
            cost_micros: None,
        };
        // Every attempt Priced: the dollars SUM (500k + 250k at $2/Mtok).
        let r1 = rcpt("pw", Some(500_000));
        let r2 = rcpt("pw", Some(250_000));
        assert_eq!(task_cost_cell(&[&r1, &r2], &bases, &all), "$1.5000");
        // Every attempt Sized: the TOKEN-WEIGHTED MEAN rate — 300k at 2x
        // blended with 100k at 1x is (600k + 100k) / 400k = 1.75x.
        let big = rcpt("sw", Some(300_000));
        let small = rcpt("cw", Some(100_000));
        assert_eq!(task_cost_cell(&[&big, &small], &bases, &all), "1.75x");
        // A price and a parameter count are incommensurable: mixed rows are
        // `-`, never a converted figure.
        let r1 = rcpt("pw", Some(1));
        let r2 = rcpt("sw", Some(1));
        assert_eq!(task_cost_cell(&[&r1, &r2], &bases, &all), "-");
        // A worker with no basis, or absent from the config entirely: `-`.
        let r = rcpt("nw", Some(1));
        assert_eq!(task_cost_cell(&[&r], &bases, &all), "-");
        let r = rcpt("ghost", Some(1));
        assert_eq!(task_cost_cell(&[&r], &bases, &all), "-");
        // One legacy receipt (no tokens) makes the row `-` — its expense is
        // unknown, so a figure that reads as exact would understate it.
        let r1 = rcpt("pw", Some(1));
        let r2 = rcpt("pw", None);
        assert_eq!(task_cost_cell(&[&r1, &r2], &bases, &all), "-");
        let r1 = rcpt("sw", Some(1));
        let r2 = rcpt("sw", None);
        assert_eq!(task_cost_cell(&[&r1, &r2], &bases, &all), "-");
        // A row whose tokens sum to zero has no weighted-mean denominator.
        let r = rcpt("sw", Some(0));
        assert_eq!(task_cost_cell(&[&r], &bases, &all), "-");
        // No Sized basis to relate to: never invent a reference rate.
        let r = rcpt("sw", Some(1));
        assert_eq!(task_cost_cell(&[&r], &bases, &[]), "-");
    }

    #[test]
    fn basis_line_names_prices_the_proxy_or_none() {
        let priced = vec![("w".to_string(), Some(Basis::Priced(1.0)))];
        let mixed = vec![
            ("w".to_string(), Some(Basis::Priced(1.0))),
            ("v".to_string(), Some(Basis::Sized(8.0))),
        ];
        let sized = vec![("v".to_string(), Some(Basis::Sized(8.0)))];
        let none = vec![("n".to_string(), None)];
        assert_eq!(
            basis_line(&priced, &[], &[]),
            "cost basis: declared prices (USD per 1M tokens)"
        );
        // Both kinds declared: say so, so the mixed `$` / `N.NNx` cells on
        // one report are explained before they are read.
        assert_eq!(
            basis_line(&mixed, &[], &[]),
            "cost basis: declared prices (USD per 1M tokens); some workers declare only params_b"
        );
        assert_eq!(
            basis_line(&sized, &[], &[]),
            "cost basis: params_b proxy (relative; cheapest declared worker = 1.00x)"
        );
        assert_eq!(
            basis_line(&none, &[], &[]),
            "cost basis: none (no worker declares params_b or price_per_mtok_usd)"
        );
    }

    /// (r11-measured-cost) the basis line names the provider as the source
    /// when a selected receipt carries a measured cost, and counts the
    /// attempts still riding a declared-price estimate; a measured-only
    /// window says just `provider-reported`, and no measured cost anywhere
    /// keeps the round-10 lines byte-for-byte.
    #[test]
    fn basis_line_names_the_measured_source_when_one_exists() {
        let bases = vec![
            ("pw".to_string(), Some(Basis::Priced(2.0))),
            ("sw".to_string(), Some(Basis::Sized(8.0))),
        ];
        let all = vec![Basis::Sized(8.0)];
        let rcpt = |worker: &str, tokens: Option<u64>, cost: Option<u64>| Receipt {
            task: "A".into(),
            attempt: 1,
            worker: worker.into(),
            model: "m".into(),
            wall_clock_s: 1.0,
            tokens,
            ts: 1,
            outcome: "merged".into(),
            error: None,
            cost_micros: cost,
        };
        // One measured + one price-estimated attempt → named source plus
        // the estimate count, exactly the shape the report prints.
        let measured = rcpt("pw", Some(4242), Some(12_300));
        let estimated = rcpt("pw", Some(500_000), None);
        assert_eq!(
            basis_line(&bases, &[&measured, &estimated], &all),
            "cost basis: provider-reported (USD); 1 of 2 attempt(s) estimated from a declared price"
        );
        // A measured-only window: no estimate note.
        assert_eq!(
            basis_line(&bases, &[&measured], &all),
            "cost basis: provider-reported (USD)"
        );
        // Even a measured cost on a worker declaring NOTHING puts the
        // report on the measured basis (real money is not a proxy).
        let nobasis = vec![("n".to_string(), None)];
        assert_eq!(
            basis_line(&nobasis, &[&measured], &all),
            "cost basis: provider-reported (USD)"
        );
        // No measured cost: the round-10 declaration lines, byte-for-byte
        // (this bases set declares a price AND a size, so the mixed note
        // rides along exactly as it did before).
        assert_eq!(
            basis_line(&bases, &[&estimated], &all),
            "cost basis: declared prices (USD per 1M tokens); some workers declare only params_b"
        );
    }

    /// (r11-measured-cost) the measured report's cells: a provider-reported
    /// cost outranks the declared price for its attempt; dollars containing
    /// an estimated term carry `~`; dollars and ratios never mix.
    #[test]
    fn measured_cells_prefer_reported_costs_and_mark_estimates() {
        let bases = vec![
            ("pw".to_string(), Some(Basis::Priced(2.0))),
            ("sw".to_string(), Some(Basis::Sized(8.0))),
            ("cw".to_string(), Some(Basis::Sized(4.0))),
        ];
        let all = vec![Basis::Sized(4.0), Basis::Sized(8.0)];
        let rcpt = |worker: &str, tokens: Option<u64>, cost: Option<u64>| Receipt {
            task: "A".into(),
            attempt: 1,
            worker: worker.into(),
            model: "m".into(),
            wall_clock_s: 1.0,
            tokens,
            ts: 1,
            outcome: "merged".into(),
            error: None,
            cost_micros: cost,
        };
        // Measured beats declared: 500k tokens at $2/Mtok would estimate to
        // $1.0000, but the provider reported $0.0123 — the measurement wins
        // and carries NO `~`.
        let measured = rcpt("pw", Some(500_000), Some(12_300));
        assert_eq!(measured_task_cell(&[&measured], &bases, &all), "$0.0123");
        assert_eq!(
            measured_worker_cell(&[&measured], basis_of("pw", &bases), Some(500_000), &all),
            "$0.0123"
        );
        // Measured + price-estimated: dollars SUM, marked `~` (an estimate
        // must never read as a measurement).
        let estimated = rcpt("pw", Some(500_000), None);
        assert_eq!(
            measured_task_cell(&[&measured, &estimated], &bases, &all),
            "~$1.0123"
        );
        assert_eq!(
            measured_worker_cell(
                &[&measured, &estimated],
                basis_of("pw", &bases),
                Some(1_000_000),
                &all
            ),
            "~$1.0123"
        );
        // Measured + sized: dollars and a ratio are incommensurable — `-`,
        // exactly like a price+sized row.
        let sized = rcpt("sw", Some(1), None);
        assert_eq!(measured_task_cell(&[&measured, &sized], &bases, &all), "-");
        // A measured cost on a worker declaring NOTHING still shows: real
        // money is not a proxy and does not need a declared basis.
        let nobases = vec![("nw".to_string(), None)];
        let m = rcpt("nw", Some(10), Some(12_300));
        assert_eq!(measured_task_cell(&[&m], &nobases, &all), "$0.0123");
        // An unknown attempt (no basis, no tokens) makes the row `-`, never
        // an understated exact-looking figure.
        let unknown = rcpt("nw", None, None);
        assert_eq!(measured_task_cell(&[&m, &unknown], &nobases, &all), "-");
        assert_eq!(
            measured_worker_cell(&[&m, &unknown], None, Some(10), &all),
            "-"
        );
        // All-ratio rows keep the round-10 blend: 300k at 2x + 100k at 1x
        // is a 1.75x weighted mean, and the worker rate is its ratio.
        let big = rcpt("sw", Some(300_000), None);
        let small = rcpt("cw", Some(100_000), None);
        assert_eq!(measured_task_cell(&[&big, &small], &bases, &all), "1.75x");
        assert_eq!(
            measured_worker_cell(&[&big], basis_of("sw", &bases), Some(300_000), &all),
            "2.00x"
        );
        // Worker fallback: a worker with NO measured receipt and nothing
        // foldable keeps the round-10 declared-rate display (a rate is a
        // worker property) — never blanked by the measured report around it.
        let tokenless = rcpt("pw", None, None);
        assert_eq!(
            measured_worker_cell(&[&tokenless], basis_of("pw", &bases), None, &all),
            "$2.0000/Mtok"
        );
        assert_eq!(
            measured_worker_cell(&[&sized], basis_of("sw", &bases), None, &all),
            "2.00x"
        );
    }
}
