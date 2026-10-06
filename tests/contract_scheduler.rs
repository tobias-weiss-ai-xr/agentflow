//! Contract tests for the scheduler's public semantics
//! ([`agentflow::scheduler`]): depths, readiness, deadlock detection, and
//! scope-overlap routing.
//!
//! `src/scheduler.rs` has the most unit tests in the crate, but they are
//! implementation-adjacent: they build configs by writing JSON fixtures to
//! temp dirs and exercise whatever slice the implementation was changing at
//! the time. WHAT CALLERS MAY RELY ON lived nowhere in one readable place.
//! This file is that place. It builds `Config`/`Task` values directly (no
//! temp dirs — these are pure functions) and pins the contract through the
//! public API only.
//!
//! These assertions were read from `src/scheduler.rs`, not guessed:
//!
//! * `compute_depths`: 0 for roots, `1 + max(dep depths)` otherwise, the
//!   same mapping on every call, and `Err("dependency cycle involving {id}")`
//!   naming a concrete cycle member when the DAG is cyclic
//!   (spec: scheduling/Dependency DAG);
//! * `scope_overlap` is a conservative prefix test (`a == b`, or either
//!   wildcard-free prefix starts with the other, in BOTH orders) — in
//!   particular an empty pattern overlaps EVERYTHING, because every string
//!   starts with `""` (spec: scheduling/Scope contention avoidance);
//! * `ready_tasks` dispatches tasks whose deps are all `Done`, skips tasks
//!   with pending deps, never dispatches a task behind a terminally-`Failed`
//!   dep (that is a deadlock, not a wait), and excludes terminal
//!   (done/failed) and attempt-exhausted tasks;
//! * `readiness_of` classifies a task into exactly one `Readiness` variant:
//!   `Done`, `StaleRunning`, `Blocked(pending deps)`, `Failed`,
//!   `Exhausted(attempts, max)`, or `Ready`;
//! * `find_deadlock` reports the blocked closure when nothing in flight can
//!   progress — deps naming absent tasks count as blocked; a cycle whose
//!   member terminally failed drags the whole cycle in; a pure healthy-status
//!   cycle is NOT its job (that is `compute_depths`/config validation's);
//!   a DAG with any runnable task returns `None`
//!   (spec: scheduling/Deadlock detection).
//!
//! Uses only std + the crate; no temp dirs, no new dependencies.

use agentflow::config::{Config, Priority, Task, TaskState};
use agentflow::router::Router;
use agentflow::scheduler::{
    compute_depths, find_deadlock, find_deadlock_in, readiness_of, ready_tasks, scope_overlap,
    tasks_overlap, Readiness,
};
use agentflow::state::TaskStatus;
use agentflow::Worker;
use std::collections::{BTreeMap, HashMap};

/// A task with the given id and deps; everything else default.
fn task(id: &str, deps: &[&str]) -> Task {
    Task {
        id: id.into(),
        title: format!("task {id}"),
        deps: deps.iter().map(|d| d.to_string()).collect(),
        accept: Some("true".into()),
        ..Default::default()
    }
}

/// A Config assembled directly from tasks (bypasses `config::load`, so no
/// file I/O and — deliberately — no cycle/warning validation).
fn cfg_of(tasks: Vec<Task>) -> Config {
    let by_id = tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();
    Config {
        tasks,
        workers: vec![],
        defaults: Default::default(),
        by_id,
        repos: BTreeMap::new(),
        warnings: vec![],
    }
}

/// A persisted status map: `(id, state, attempts)`.
fn status_with(pairs: &[(&str, TaskState, u32)]) -> HashMap<String, TaskStatus> {
    pairs
        .iter()
        .map(|(id, state, attempts)| {
            (
                id.to_string(),
                TaskStatus {
                    state: state.clone(),
                    attempts: *attempts,
                    last_error: None,
                    phase: None,
                    attempt_started_ts: None,
                    attempt_worker: None,
                },
            )
        })
        .collect()
}

fn ids(ready: &[Task]) -> Vec<&str> {
    ready.iter().map(|t| t.id.as_str()).collect()
}

/// The end-to-end story a caller may rely on: a runnable DAG yields depths,
/// readiness classification, and a dispatch list that agree with each other
/// at every step, and a healthy run never reports a deadlock.
#[test]
fn scheduler_readiness_and_dag_contract() {
    let cfg = cfg_of(vec![
        task("a", &[]),
        task("b", &["a"]),
        task("c", &["b"]),
        task("d", &[]),
    ]);

    // Depths: chain a -> b -> c is 0/1/2; independent d stays 0.
    let depths = compute_depths(&cfg).unwrap();
    assert_eq!(depths["a"], 0);
    assert_eq!(depths["b"], 1);
    assert_eq!(depths["c"], 2);
    assert_eq!(depths["d"], 0);

    // Nothing run yet: roots dispatchable (equal depth + priority → original
    // order), dependents blocked, no deadlock.
    let empty = status_with(&[]);
    assert_eq!(ids(&ready_tasks(&cfg, &empty, &[], 3)), vec!["a", "d"]);
    assert_eq!(
        readiness_of(&cfg.by_id["b"], &empty, 3),
        Readiness::Blocked(vec!["a".to_string()])
    );
    assert_eq!(
        readiness_of(&cfg.by_id["c"], &empty, 3),
        Readiness::Blocked(vec!["b".to_string()])
    );
    assert!(find_deadlock(&cfg, &empty, &[]).is_none());

    // a done → b ready (and dispatched before the shallower d: critical path
    // first). Still no deadlock while anything can progress.
    let a_done = status_with(&[("a", TaskState::Done, 1)]);
    assert_eq!(ids(&ready_tasks(&cfg, &a_done, &[], 3)), vec!["b", "d"]);
    assert_eq!(readiness_of(&cfg.by_id["b"], &a_done, 3), Readiness::Ready);
    assert!(find_deadlock(&cfg, &a_done, &[]).is_none());

    // b, c done → only d left, and the done tasks never re-dispatch.
    let most_done = status_with(&[
        ("a", TaskState::Done, 1),
        ("b", TaskState::Done, 1),
        ("c", TaskState::Done, 1),
    ]);
    assert_eq!(ids(&ready_tasks(&cfg, &most_done, &[], 3)), vec!["d"]);
    assert!(find_deadlock(&cfg, &most_done, &[]).is_none());

    // All done: nothing ready (all terminal), still no deadlock.
    let all_done = status_with(&[
        ("a", TaskState::Done, 1),
        ("b", TaskState::Done, 1),
        ("c", TaskState::Done, 1),
        ("d", TaskState::Done, 1),
    ]);
    assert!(ready_tasks(&cfg, &all_done, &[], 3).is_empty());
    assert_eq!(readiness_of(&cfg.by_id["a"], &all_done, 3), Readiness::Done);
    assert!(find_deadlock(&cfg, &all_done, &[]).is_none());
}

// spec: scheduling/critical-path-priority#priority-breaks-ties-among-equally-deep-ready-tasks
#[test]
fn priority_breaks_ties_among_equally_deep_ready_tasks() {
    // Half 1: among ready tasks at the SAME dependency depth, the strictly
    // higher `priority` rank dispatches first — regardless of where the task
    // sits in the config file. Here `low` is configured first but ranks last,
    // and `high` is configured last but ranks first.
    let mut low = task("low", &[]);
    low.priority = Priority::Str("LOW".into()); // rank -10
    let mut mid = task("mid", &[]);
    mid.priority = Priority::Num(5); // rank 5
    let mut high = task("high", &[]);
    high.priority = Priority::Str("HIGH".into()); // rank 10
    let cfg = cfg_of(vec![low, mid, high]);
    let empty = status_with(&[]);
    assert_eq!(
        ids(&ready_tasks(&cfg, &empty, &[], 3)),
        vec!["high", "mid", "low"],
        "higher priority rank dispatches first among equal-depth ready tasks"
    );

    // Half 2: tasks of EQUAL rank keep their original config order (the
    // documented stable fallback). Numeric and string forms that map to the
    // same rank count as equal — the tie-break compares rank, not the
    // representation.
    let mut first = task("first", &[]);
    first.priority = Priority::Num(0); // rank 0
    let mut second = task("second", &[]);
    second.priority = Priority::Str("MEDIUM".into()); // rank 0
    let mut third = task("third", &[]);
    third.priority = Priority::Str("NORMAL".into()); // rank 0
    let cfg = cfg_of(vec![first, second, third]);
    assert_eq!(
        ids(&ready_tasks(&cfg, &empty, &[], 3)),
        vec!["first", "second", "third"],
        "equal priority rank preserves the original config order"
    );
}

// ---------------------------------------------------------------------------
// Worker selection (Router::pick): cost as a tie-break, never a score.
//
// The task-level tie-breaks above decide WHICH TASK dispatches; these
// decide WHICH WORKER takes it. The UCB1 score (measured trust +
// exploration) always decides; the workers' DECLARED cost basis
// (`params_b` / `price_per_mtok_usd`, see src/cost.rs) is consulted only
// when two scores tie, and only when the two bases are comparable.
// ---------------------------------------------------------------------------

/// A worker with the given name and declared cost basis; everything else
/// default (enabled, no extra args, text output) — the same construction
/// style as `src/router.rs`'s own unit tests.
fn worker_of(name: &str, params_b: Option<f64>, price_per_mtok_usd: Option<f64>) -> Worker {
    Worker {
        name: name.to_string(),
        output: "text".into(),
        args: Vec::new(),
        params_b,
        price_per_mtok_usd,
        ..Default::default()
    }
}

/// A Router whose named workers all carry IDENTICAL stats (`wins` merged
/// out of `attempts` each), so their UCB1 scores tie exactly (same mean,
/// same n, hence the same exploration term).
fn router_with_tied_stats(names: &[&str], wins: u64, attempts: u64) -> Router {
    let mut r = Router::default();
    for name in names {
        for i in 0..attempts {
            r.record(name, i < wins);
        }
    }
    r
}

// spec: scheduling/ucb1-worker-selection#cheaper-worker-wins-a-score-tie
#[test]
fn the_router_prefers_the_cheaper_worker_only_when_trust_ties() {
    // Identical stats (2/2 each): the UCB1 scores tie exactly. The worker
    // declaring the SMALLER params_b wins the tie — even though it is
    // configured SECOND, so the old config-order tie-break would have
    // picked "big" (the test passes by accident otherwise).
    let r = router_with_tied_stats(&["big", "small"], 2, 2);
    let pool = [
        worker_of("big", Some(235.0), None),
        worker_of("small", Some(8.0), None),
    ];
    assert_eq!(
        r.pick(pool.iter()).unwrap().name,
        "small",
        "on a score tie, the smaller declared params_b wins regardless of config order"
    );

    // Two declared PRICES behave the same way: the cheaper $/Mtok wins the
    // tie, again from second position in the pool.
    let pool = [
        worker_of("dear", None, Some(5.0)),
        worker_of("bargain", None, Some(0.5)),
    ];
    assert_eq!(
        r.pick(pool.iter()).unwrap().name,
        "bargain",
        "on a score tie, the cheaper declared price wins regardless of config order"
    );
}

// spec: scheduling/ucb1-worker-selection#a-strictly-better-score-beats-a-cheaper-competitor
#[test]
fn a_strictly_better_trust_score_beats_a_cheaper_competitor() {
    // The most important property: cost NEVER overrides measured trust.
    // "expensive" is 3/3 merged, "cheap" is 0/3 — equal counts, different
    // means, so the score is STRICTLY higher. "cheap" is deliberately
    // FIRST in the pool: config order and the cost tie-break would both
    // favour it, so only the strictly-better score can explain the pick.
    let mut r = Router::default();
    for _ in 0..3 {
        r.record("cheap", false); // 0/3
        r.record("expensive", true); // 3/3
    }
    let pool = [
        worker_of("cheap", Some(8.0), None),
        worker_of("expensive", Some(235.0), None),
    ];
    assert_eq!(
        r.pick(pool.iter()).unwrap().name,
        "expensive",
        "a strictly better win rate beats both config order and the cheaper basis"
    );

    // A narrower margin (3/3 vs 2/3 at equal counts) still strictly wins
    // over the cheaper competitor.
    let mut r = Router::default();
    for i in 0..3 {
        r.record("cheap", i < 2); // 2/3
        r.record("expensive", true); // 3/3
    }
    assert_eq!(
        r.pick(pool.iter()).unwrap().name,
        "expensive",
        "any strict score difference outranks the declared cost basis"
    );
}

// spec: scheduling/ucb1-worker-selection#incomparable-bases-keep-config-order
#[test]
fn incomparable_cost_bases_keep_config_order() {
    // One priced, one only sized: a $/Mtok price and a parameter count are
    // incommensurable (`cost::compare` refuses to order them), so a score
    // tie never triggers a swap — the FIRST worker keeps the tie...
    let r = router_with_tied_stats(&["priced", "sized"], 1, 2);
    let pool = [
        worker_of("priced", None, Some(0.1)),
        worker_of("sized", Some(8.0), None),
    ];
    assert_eq!(r.pick(pool.iter()).unwrap().name, "priced");
    // ...and in the reverse order the other one keeps it (the rule is
    // "no swap", not "priced wins").
    let pool = [
        worker_of("sized", Some(8.0), None),
        worker_of("priced", None, Some(0.1)),
    ];
    assert_eq!(r.pick(pool.iter()).unwrap().name, "sized");

    // A worker declaring NOTHING can neither be displaced by a tie-break
    // nor displace one: either side missing its basis makes `compare`
    // return None, so config order decides — in both orders.
    let r2 = router_with_tied_stats(&["declared", "neutral"], 2, 4);
    let pool = [
        worker_of("declared", Some(8.0), None),
        worker_of("neutral", None, None),
    ];
    assert_eq!(r2.pick(pool.iter()).unwrap().name, "declared");
    let pool = [
        worker_of("neutral", None, None),
        worker_of("declared", Some(8.0), None),
    ];
    assert_eq!(r2.pick(pool.iter()).unwrap().name, "neutral");

    // Two workers declaring nothing: pure config order (the legacy
    // behaviour, unchanged).
    let r3 = router_with_tied_stats(&["x", "y"], 1, 1);
    let pool = [worker_of("x", None, None), worker_of("y", None, None)];
    assert_eq!(r3.pick(pool.iter()).unwrap().name, "x");

    // Equal comparable declarations are not "cheaper" (`Some(Equal)`):
    // config order still decides.
    let r4 = router_with_tied_stats(&["twin1", "twin2"], 3, 3);
    let pool = [
        worker_of("twin1", Some(70.0), None),
        worker_of("twin2", Some(70.0), None),
    ];
    assert_eq!(r4.pick(pool.iter()).unwrap().name, "twin1");
}

// spec: scheduling/Dependency DAG
#[test]
fn compute_depths_chain_independent_and_determinism() {
    let cfg = cfg_of(vec![
        task("a", &[]),
        task("b", &["a"]),
        task("c", &["b"]),
        task("solo", &[]),
    ]);
    let depths = compute_depths(&cfg).unwrap();
    assert_eq!(depths["a"], 0, "no deps → depth 0");
    assert_eq!(depths["b"], 1, "1 + max(dep) = 1 + 0");
    assert_eq!(depths["c"], 2, "1 + max(dep) = 1 + 1");
    assert_eq!(depths["solo"], 0, "independent task stays depth 0");

    // Same Config → the SAME mapping on repeated calls (pure function).
    let again = compute_depths(&cfg).unwrap();
    assert_eq!(depths, again);
}

// spec: scheduling/Dependency DAG — "cycle rejected"
#[test]
fn compute_depths_cycle_error_names_a_cycle_member() {
    let two = cfg_of(vec![task("a", &["b"]), task("b", &["a"])]);
    let err = compute_depths(&two).unwrap_err();
    assert!(err.contains("cycle"), "error should say cycle: {err}");
    assert!(
        err.contains('a') || err.contains('b'),
        "error must name a cycle member: {err}"
    );

    // A self-dependency is the smallest cycle and is also rejected.
    let self_dep = cfg_of(vec![task("a", &["a"])]);
    let err = compute_depths(&self_dep).unwrap_err();
    assert!(
        err.contains("cycle") && err.contains('a'),
        "self-dep: {err}"
    );
}

// spec: scheduling/Scope contention avoidance
#[test]
fn scope_overlap_contract() {
    // Identical patterns overlap (whatever the pattern shape).
    assert!(scope_overlap("src/a.rs", "src/a.rs"));

    // A directory prefix overlaps its children — in BOTH orders. This is
    // what makes the contention check symmetric: it does not matter whether
    // the directory or the file pattern is the running task's.
    assert!(scope_overlap("tests/", "tests/x.rs"));
    assert!(scope_overlap("tests/x.rs", "tests/"));

    // Disjoint subtrees do not overlap and may run in parallel.
    assert!(!scope_overlap("src/a.rs", "docs/b.md"));
    assert!(!scope_overlap("src/", "docs/"));

    // The test is deliberately conservative about globs: only the
    // wildcard-free prefix is compared, so `src/*.rs` vs `src/*.py` shares
    // the `src/` prefix → overlaps (a false positive is the safe direction:
    // it defers work rather than corrupting it).
    assert!(scope_overlap("src/*.rs", "src/*.py"));
    assert!(!scope_overlap("a*glob", "b*glob"));

    // PIN: an empty pattern DOES overlap everything. Every string starts
    // with "", so `"".starts_with`-style prefix comparison makes the empty
    // pattern a prefix of any path. That is the code's actual behaviour and
    // it is the conservative one (an empty scope may touch anything), so we
    // pin it rather than wish it away.
    assert!(scope_overlap("", "any/path/at/all"));
    assert!(scope_overlap("src/a.rs", ""));
}

#[test]
fn tasks_overlap_any_pair_generalisation() {
    // Any overlapping pair between the two scope lists → overlap.
    let a: Vec<String> = vec!["src/a.rs".into()];
    let b: Vec<String> = vec!["docs/b.md".into(), "src/a.rs".into()];
    assert!(tasks_overlap(&a, &b));
    assert!(
        tasks_overlap(&b, &a),
        "generalisation is symmetric in the lists"
    );

    // No overlapping pair anywhere → disjoint, even for non-empty lists.
    let c: Vec<String> = vec!["docs/".into(), "README.md".into()];
    assert!(!tasks_overlap(&a, &c));

    // An empty scope list can never conflict.
    assert!(!tasks_overlap(&[], &c));
}

// spec: scheduling/Dependency DAG, scheduling/Deadlock detection
#[test]
fn ready_tasks_dep_lifecycle() {
    // Deps all Done → dependent becomes ready.
    let cfg = cfg_of(vec![task("a", &[]), task("b", &["a"])]);
    let a_done = status_with(&[("a", TaskState::Done, 1)]);
    assert_eq!(ids(&ready_tasks(&cfg, &a_done, &[], 3)), vec!["b"]);

    // A pending dep (absent from the status map, or not yet Done) → the
    // dependent waits: not in the dispatch list.
    let empty = status_with(&[]);
    assert!(ready_tasks(&cfg, &empty, &[], 3)
        .iter()
        .all(|t| t.id != "b"));

    // A terminally-FAILED dep is not a wait: `Failed` never becomes `Done`,
    // so the dependent can NEVER dispatch. The proof that it is permanently
    // stuck (not merely waiting) is that the deadlock detector fires: with
    // nothing else runnable the run halts with the blocked closure.
    let a_failed = status_with(&[("a", TaskState::Failed, 3)]);
    assert!(ready_tasks(&cfg, &a_failed, &[], 3)
        .iter()
        .all(|t| t.id != "b"));
    let blocked = find_deadlock(&cfg, &a_failed, &[]).expect("failed dep ⇒ deadlock");
    assert!(blocked.contains(&"a".to_string()) && blocked.contains(&"b".to_string()));
}

#[test]
fn ready_tasks_excludes_terminal_and_exhausted_tasks() {
    let cfg = cfg_of(vec![task("a", &[])]);

    // A Done task never re-dispatches.
    let done = status_with(&[("a", TaskState::Done, 1)]);
    assert!(ready_tasks(&cfg, &done, &[], 3).is_empty());

    // A Failed task is terminal and excluded.
    let failed = status_with(&[("a", TaskState::Failed, 1)]);
    assert!(ready_tasks(&cfg, &failed, &[], 3).is_empty());

    // Attempt budget exhausted (attempts >= max_attempts) excludes the task
    // even though its state is Ready and it has no deps: the deadlock logic
    // owns surfacing it.
    let exhausted = status_with(&[("a", TaskState::Ready, 3)]);
    assert!(ready_tasks(&cfg, &exhausted, &[], 3).is_empty());
    // Budget still available at max-1 → dispatchable.
    let budget_left = status_with(&[("a", TaskState::Ready, 2)]);
    assert_eq!(ids(&ready_tasks(&cfg, &budget_left, &[], 3)), vec!["a"]);
}

#[test]
fn readiness_of_returns_exact_variants() {
    let cfg = cfg_of(vec![task("a", &[]), task("b", &["a"])]);

    // Done task → Done (merged in a previous run).
    let done = status_with(&[("b", TaskState::Done, 1)]);
    assert_eq!(readiness_of(&cfg.by_id["b"], &done, 3), Readiness::Done);

    // Persisted running from an interrupted run → StaleRunning (not Ready:
    // `af run` self-heals it before dispatch).
    let running = status_with(&[("b", TaskState::Running, 1)]);
    assert_eq!(
        readiness_of(&cfg.by_id["b"], &running, 3),
        Readiness::StaleRunning
    );

    // Unmet dependency → Blocked, listing the pending dep ids.
    let empty = status_with(&[]);
    assert_eq!(
        readiness_of(&cfg.by_id["b"], &empty, 3),
        Readiness::Blocked(vec!["a".to_string()])
    );

    // Terminal failure → Failed.
    let failed = status_with(&[("b", TaskState::Failed, 1)]);
    assert_eq!(readiness_of(&cfg.by_id["b"], &failed, 3), Readiness::Failed);

    // Attempt budget exhausted → Exhausted(attempts, max), a distinct
    // terminal-blocked classification from Failed.
    let exhausted = status_with(&[("b", TaskState::Ready, 3)]);
    assert_eq!(
        readiness_of(&cfg.by_id["b"], &exhausted, 3),
        Readiness::Exhausted(3, 3)
    );

    // Deps done, budget left → Ready.
    let a_done = status_with(&[("a", TaskState::Done, 1)]);
    assert_eq!(readiness_of(&cfg.by_id["b"], &a_done, 3), Readiness::Ready);
}

// spec: scheduling/Deadlock detection — "absent dependency is a deadlock"
#[test]
fn find_deadlock_reports_absent_dependency() {
    // Built directly (config::load would only warn about GHOST — compat).
    let cfg = cfg_of(vec![task("a", &["GHOST"])]);
    let blocked =
        find_deadlock(&cfg, &status_with(&[]), &[]).expect("absent dep can never resolve");
    assert!(
        blocked.contains(&"a".to_string()),
        "the dependent is reported: {blocked:?}"
    );
}

// spec: scheduling/Deadlock detection — "all tasks blocked by failure"
#[test]
fn find_deadlock_reports_cycle_with_its_members() {
    // a <-> b with a terminally failed: the closure pulls BOTH cycle members
    // into the blocked set, so the report names the whole cycle.
    let cfg = cfg_of(vec![task("a", &["b"]), task("b", &["a"])]);
    let a_failed = status_with(&[("a", TaskState::Failed, 3)]);
    let blocked =
        find_deadlock(&cfg, &a_failed, &[]).expect("cycle behind a terminal failure ⇒ deadlock");
    assert!(blocked.contains(&"a".to_string()));
    assert!(blocked.contains(&"b".to_string()));
}

// spec: scheduling/Dependency DAG — division of labour
#[test]
fn find_deadlock_ignores_pure_cycle_compute_depths_catches_it() {
    // PIN: a cycle with healthy statuses is NOT reported by find_deadlock.
    // It models *runtime* terminal blocking (failed/absent deps), not static
    // DAG validity; nothing has terminally failed, so there is nothing to
    // report. A pure cycle is caught statically — `compute_depths` errors
    // naming a member and config validation rejects it on load — and at run
    // time the loop's fallback (report all in-scope ids when nothing is
    // ready) surfaces it. Both halves of that contract are pinned here.
    let cfg = cfg_of(vec![task("a", &["b"]), task("b", &["a"])]);
    assert_eq!(find_deadlock(&cfg, &status_with(&[]), &[]), None);
    let err = compute_depths(&cfg).unwrap_err();
    assert!(err.contains("cycle") && (err.contains('a') || err.contains('b')));
}

// spec: scheduling/Deadlock detection — "no deadlock while progress possible"
#[test]
fn find_deadlock_healthy_dag_is_none() {
    let cfg = cfg_of(vec![
        task("a", &[]),
        task("b", &["a"]),
        task("c", &[]),
        task("d", &["c"]),
    ]);
    // Fresh DAG: roots can always run.
    assert_eq!(find_deadlock(&cfg, &status_with(&[]), &[]), None);
    // Mid-run with a failure: c/d still independent of the failed branch.
    let a_failed = status_with(&[("a", TaskState::Failed, 3)]);
    assert_eq!(find_deadlock(&cfg, &a_failed, &[]), None);
    // Anything in flight suppresses the check entirely (progress is being
    // made; deadlock can only be declared when nothing is running).
    assert_eq!(
        find_deadlock(
            &cfg,
            &status_with(&[("a", TaskState::Failed, 3)]),
            &["c".to_string()]
        ),
        None
    );
    // All done: nothing remaining, nothing blocked.
    let all_done = status_with(&[
        ("a", TaskState::Done, 1),
        ("b", TaskState::Done, 1),
        ("c", TaskState::Done, 1),
        ("d", TaskState::Done, 1),
    ]);
    assert_eq!(find_deadlock(&cfg, &all_done, &[]), None);
}

// spec: scheduling/Deadlock detection — scoped variant (the --task filter)
#[test]
fn find_deadlock_in_restricts_the_blocked_closure_to_scope() {
    let cfg = cfg_of(vec![task("a", &[]), task("b", &["a"]), task("c", &[])]);
    // a failed: the full config can still progress via c → no deadlock.
    let a_failed = status_with(&[("a", TaskState::Failed, 3)]);
    assert_eq!(find_deadlock(&cfg, &a_failed, &[]), None);
    // But scoped to {a, b} (e.g. `--task b`), nothing in scope can progress
    // → the closure is reported.
    let scope: Vec<&Task> = vec![&cfg.by_id["a"], &cfg.by_id["b"]];
    let blocked = find_deadlock_in(&cfg, &a_failed, &[], &scope)
        .expect("scoped view deadlocks even though the full DAG does not");
    assert!(blocked.contains(&"a".to_string()) && blocked.contains(&"b".to_string()));
    // Scoped to the runnable task → healthy.
    let healthy_scope: Vec<&Task> = vec![&cfg.by_id["c"]];
    assert_eq!(find_deadlock_in(&cfg, &a_failed, &[], &healthy_scope), None);
}
