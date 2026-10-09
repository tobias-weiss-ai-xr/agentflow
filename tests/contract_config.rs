//! Contract tests for `agentflow::config` — what a campaign config MEANS.
//!
//! `src/config.rs` is where a campaign's meaning is decided: which fields
//! default, how precedence works, and which broken DAGs are rejected. Until
//! now its behavior was only covered by the module's own unit tests, which
//! live beside the implementation and can drift with it. This file pins the
//! PUBLIC contract from the outside — `load`, `detect_cycle`,
//! `parse_gate_env`, `load_repos`, and the public `Config` / `Settings` /
//! `Task` / `Worker` / `WorkerDefaults` / `TaskState` / `Priority` types —
//! so a refactor that changes what callers may rely on goes red.
//!
//! Everything asserted here was read from the implementation first. Where
//! the task brief and the shipped code disagree, the SHIPPED code wins and
//! the divergence is called out at the assertion:
//!
//! * a `Task` has NO worker field (`src/config.rs`), so a task can never
//!   reference an unknown worker at load time; unknown-worker rejection is a
//!   dispatch-time concern (`--worker`, src/run.rs) pinned by
//!   tests/cli.rs::validate_rejects_unknown_worker_filter;
//! * a dangling `dep` is a WARNING, not a load error
//!   (spec: config/task-schema-loading#dangling-dependency-warns); the hard
//!   rejection of an unresolvable id happens when the DAG is resolved for
//!   dispatch (`scheduler::compute_depths` → "unknown task");
//! * a MISSING `repos.json` is `Ok(empty)` — single-repo mode — not an
//!   error (spec: config/optional-repos-json-defines-named-repositories);
//!   only a malformed/unreadable one is `Err`.
//!
//! Uses only std + the crate. Every test gets its own uniquely-named temp
//! directory so the file stays independent under cargo's parallel runner.
//! The one env-reading test (`Settings::from_env`) documents its own race
//! avoidance in place.

use agentflow::config as config;
use agentflow::config::{detect_cycle, load, load_repos, parse_gate_env, Priority};
use agentflow::router::Router;
use agentflow::run::pick_worker;
use agentflow::{Config, Settings, Task, TaskState, Worker, WorkerDefaults};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A tasks.json with one dep-free, gate-bearing task — the minimum valid
/// task list, reused across fixtures.
const ONE_TASK: &str = r#"{ "tasks": [{"id":"A","title":"a","accept":"true"}] }"#;

/// A workers.json with exactly one enabled worker — the minimum valid file.
const ONE_WORKER: &str = r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}] }"#;

/// Fresh, uniquely-named temp directory (one per call), emptied if a stale
/// one exists. Unique across tests via pid + an atomic counter.
fn fresh_dir(label: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "af-contract-config-{label}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create unique temp dir");
    d
}

/// Write `content` to `path` (creating parents) and return the path.
fn write_file(path: &Path, content: &str) -> PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, content).expect("write fixture");
    path.to_path_buf()
}

/// Write `tasks.json` + `workers.json` under `dir` and return their paths.
fn write_config(dir: &Path, tasks_body: &str, workers_body: &str) -> (PathBuf, PathBuf) {
    let tasks = write_file(&dir.join("tasks.json"), tasks_body);
    let workers = write_file(&dir.join("workers.json"), workers_body);
    (tasks, workers)
}

/// Build a `Task` directly (for the pure `detect_cycle` contract).
fn task(id: &str, deps: &[&str]) -> Task {
    Task {
        id: id.to_string(),
        deps: deps.iter().map(|d| d.to_string()).collect(),
        ..Default::default()
    }
}

/// The headline contract: `load` parses a well-formed campaign, applies the
/// documented defaults for absent fields, and rejects — with `Err`, never a
/// panic — the broken shapes callers must survive. The focused tests below
/// expand each area.
// spec: config/task-schema-loading
// spec: config/worker-schema-loading
#[test]
fn config_loading_and_validation_contract() {
    let d = fresh_dir("umbrella");

    // A healthy config loads, and absent fields take their defaults.
    let (tasks, workers) = write_config(&d.join("valid"), ONE_TASK, ONE_WORKER);
    let cfg: Config = load(&tasks, &workers).expect("valid config loads");
    assert_eq!(cfg.tasks.len(), 1);
    assert_eq!(cfg.workers.len(), 1);
    assert!(cfg.by_id.contains_key("A"));
    assert!(cfg.by_id["A"].gate_replay, "gate_replay defaults to true");
    assert_eq!(cfg.by_id["A"].priority.rank(), 0, "priority defaults to 0");
    assert_eq!(cfg.defaults.max_attempts, 3);
    // 0 = opt-in pacing (r9-retry-pacing): an omitted retry_delay_s means
    // no delay between attempts — the legacy behaviour.
    assert_eq!(cfg.defaults.retry_delay_s, 0);

    // An explicit value WINS over the default: the two files differ only in
    // that one field (expanded in defaults_apply_and_explicit_values_win).
    let (tasks_false, workers_false) = write_config(
        &d.join("explicit"),
        r#"{ "tasks": [{"id":"A","title":"a","accept":"true","gate_replay":false}] }"#,
        ONE_WORKER,
    );
    let cfg_false = load(&tasks_false, &workers_false).expect("valid config loads");
    assert!(!cfg_false.by_id["A"].gate_replay, "explicit false wins");
    assert_ne!(cfg.by_id["A"].gate_replay, cfg_false.by_id["A"].gate_replay);

    // Broken inputs are returned as `Err`, never a panic.
    let err = load(&d.join("absent.json"), &workers).unwrap_err();
    assert!(err.contains("read"), "missing file: {err}");
    let bad = write_file(&d.join("bad.json"), "{ not json");
    let err = load(&bad, &workers).unwrap_err();
    assert!(err.contains("parse"), "malformed JSON: {err}");
    let (dup, dup_workers) = write_config(
        &d.join("dup"),
        r#"{ "tasks": [
            {"id":"A","title":"x","accept":"true"},
            {"id":"A","title":"y","accept":"true"}
        ] }"#,
        ONE_WORKER,
    );
    let err = load(&dup, &dup_workers).unwrap_err();
    assert!(err.contains("duplicate task id"), "{err}");

    // A disabled worker does not fail the load; only the enabled set is
    // dispatchable (pinned in disabled_worker_loads_but_is_not_selectable).
    let (t_off, w_off) = write_config(
        &d.join("disabled"),
        ONE_TASK,
        r#"{ "workers": [
            {"name":"on","provider":"p","model":"m","enabled":true},
            {"name":"off","provider":"p","model":"m","enabled":false}
        ] }"#,
    );
    let cfg_off = load(&t_off, &w_off).expect("a disabled worker is not fatal");
    assert_eq!(cfg_off.workers.iter().filter(|w| w.enabled).count(), 1);

    // A dependency cycle is rejected with a message naming a member.
    let (cyclic, cyclic_workers) = write_config(
        &d.join("cycle"),
        r#"{ "tasks": [
            {"id":"A","deps":["B"],"accept":"true"},
            {"id":"B","deps":["A"],"accept":"true"}
        ] }"#,
        ONE_WORKER,
    );
    let err = load(&cyclic, &cyclic_workers).unwrap_err();
    assert!(err.contains("dependency cycle"), "{err}");
}

/// Absent fields get their documented defaults; when the SAME field is
/// present, the explicit value wins. Each pair of fixtures differs only in
/// that one field, so the assertion isolates precedence.
// spec: config/task-schema-loading#string-priority-levels-accepted
#[test]
fn defaults_apply_and_explicit_values_win() {
    let d = fresh_dir("defaults");

    // --- Task::gate_replay: absent => true; explicit false wins.
    let (t_absent, w_absent) = write_config(
        &d.join("gate-absent"),
        r#"{ "tasks": [{"id":"T","title":"t","accept":"true"}] }"#,
        ONE_WORKER,
    );
    let (t_present, w_present) = write_config(
        &d.join("gate-present"),
        r#"{ "tasks": [{"id":"T","title":"t","accept":"true","gate_replay":false}] }"#,
        ONE_WORKER,
    );
    let absent = load(&t_absent, &w_absent).unwrap();
    let present = load(&t_present, &w_present).unwrap();
    assert!(absent.by_id["T"].gate_replay, "absent => true");
    assert!(!present.by_id["T"].gate_replay, "explicit false => false");
    assert_ne!(
        absent.by_id["T"].gate_replay,
        present.by_id["T"].gate_replay
    );

    // --- Task::priority: absent => rank 0; explicit number / string win.
    let (t_num, w_num) = write_config(
        &d.join("prio-num"),
        r#"{ "tasks": [{"id":"T","title":"t","accept":"true","priority":7}] }"#,
        ONE_WORKER,
    );
    let (t_str, w_str) = write_config(
        &d.join("prio-str"),
        r#"{ "tasks": [{"id":"T","title":"t","accept":"true","priority":"CRITICAL"}] }"#,
        ONE_WORKER,
    );
    assert_eq!(absent.by_id["T"].priority.rank(), 0, "absent => rank 0");
    assert_eq!(load(&t_num, &w_num).unwrap().by_id["T"].priority.rank(), 7);
    assert_eq!(load(&t_str, &w_str).unwrap().by_id["T"].priority.rank(), 20);

    // --- WorkerDefaults: absent => shipped defaults; explicit wins.
    let (t_wd_absent, w_wd_absent) = write_config(&d.join("wd-absent"), ONE_TASK, ONE_WORKER);
    let (t_wd_present, w_wd_present) = write_config(
        &d.join("wd-present"),
        ONE_TASK,
        r#"{ "defaults": {"max_attempts":7,"retry_delay_s":9,"accept_timeout_s":11,"agent_timeout_s":13},
             "workers": [{"name":"w1","provider":"p","model":"m"}] }"#,
    );
    let wd_absent = load(&t_wd_absent, &w_wd_absent).unwrap();
    let wd_present = load(&t_wd_present, &w_wd_present).unwrap();
    assert_eq!(wd_absent.defaults.max_attempts, 3);
    assert_eq!(wd_absent.defaults.retry_delay_s, 0);
    assert_eq!(wd_absent.defaults.accept_timeout_s, 600);
    assert_eq!(wd_absent.defaults.agent_timeout_s, 3600);
    assert_eq!(wd_present.defaults.max_attempts, 7);
    assert_eq!(wd_present.defaults.retry_delay_s, 9);
    assert_eq!(wd_present.defaults.accept_timeout_s, 11);
    assert_eq!(wd_present.defaults.agent_timeout_s, 13);
    assert_ne!(
        wd_absent.defaults.max_attempts,
        wd_present.defaults.max_attempts
    );

    // --- The struct Default impls agree with the shipped JSON defaults.
    let wd = WorkerDefaults::default();
    assert_eq!(
        (
            wd.accept_timeout_s,
            wd.max_attempts,
            wd.retry_delay_s,
            wd.agent_timeout_s
        ),
        (600, 3, 0, 3600)
    );
    let w = Worker::default();
    assert!(w.enabled, "a worker defaults to enabled");
    assert_eq!(w.cli, "pi", "the agent CLI defaults to pi");
}

/// `load` reports broken inputs as `Err(String)` — never a panic. Error text
/// is pinned by loose substring so it stays readable.
// spec: config/task-schema-loading#duplicate-id-rejected
// spec: config/worker-schema-loading#zero-enabled-workers-rejected
#[test]
fn load_returns_err_never_panics() {
    let d = fresh_dir("errors");
    let workers = write_file(&d.join("workers.json"), ONE_WORKER);
    let tasks = write_file(&d.join("tasks.json"), ONE_TASK);

    // Missing tasks file.
    let err = load(&d.join("does-not-exist.json"), &workers).unwrap_err();
    assert!(err.contains("read"), "missing file: {err}");
    assert!(err.contains("does-not-exist.json"), "names the file: {err}");

    // Missing workers file.
    let err = load(&tasks, &d.join("no-workers.json")).unwrap_err();
    assert!(err.contains("read"), "missing workers file: {err}");

    // Malformed JSON on either side.
    let bad = write_file(&d.join("bad.json"), "{ not json ]");
    let err = load(&bad, &workers).unwrap_err();
    assert!(err.contains("parse"), "malformed tasks: {err}");
    let err = load(&tasks, &bad).unwrap_err();
    assert!(err.contains("parse"), "malformed workers: {err}");

    // Duplicate task id.
    let (dup, dup_w) = write_config(
        &d.join("dup"),
        r#"{ "tasks": [
            {"id":"A","title":"x","accept":"true"},
            {"id":"A","title":"y","accept":"true"}
        ] }"#,
        ONE_WORKER,
    );
    let err = load(&dup, &dup_w).unwrap_err();
    assert!(err.contains("duplicate task id"), "{err}");
    assert!(err.contains('A'), "names the duplicate: {err}");

    // Empty task id.
    let (empty_id, empty_id_w) = write_config(
        &d.join("empty-id"),
        r#"{ "tasks": [{"id":"","title":"x","accept":"true"}] }"#,
        ONE_WORKER,
    );
    let err = load(&empty_id, &empty_id_w).unwrap_err();
    assert!(err.contains("empty id"), "{err}");

    // Worker-level validation: provider/model required, at least one enabled.
    let (ok_t, bad_w) = write_config(
        &d.join("bad-provider"),
        ONE_TASK,
        r#"{ "workers": [{"name":"w1","provider":"","model":"m"}] }"#,
    );
    let err = load(&ok_t, &bad_w).unwrap_err();
    assert!(err.contains("provider and model are required"), "{err}");

    let (ok_t2, none_on) = write_config(
        &d.join("no-enabled"),
        ONE_TASK,
        r#"{ "workers": [{"name":"w1","provider":"p","model":"m","enabled":false}] }"#,
    );
    let err = load(&ok_t2, &none_on).unwrap_err();
    assert!(err.contains("no enabled workers"), "{err}");

    // The `Task` schema has NO worker field (src/config.rs), so a stray
    // "worker" key cannot make `load` fail: unknown-worker rejection lives
    // at dispatch time (src/run.rs `--worker`, pinned by
    // tests/cli.rs::validate_rejects_unknown_worker_filter). Pin that here
    // so the boundary is explicit rather than assumed.
    let (ghost_t, ghost_w) = write_config(
        &d.join("ghost-worker"),
        r#"{ "tasks": [{"id":"A","title":"a","accept":"true","worker":"ghost"}] }"#,
        ONE_WORKER,
    );
    assert!(
        load(&ghost_t, &ghost_w).is_ok(),
        "Task carries no worker field, so an unknown worker reference is inert \
         at load time"
    );
}

/// A disabled worker is retained in `Config.workers` (so callers can still
/// report it) but is NEVER usable/selectable: load succeeds with at least
/// one enabled worker, and the dispatcher's pick skips disabled ones even
/// when named by a `--worker` filter.
// spec: config/worker-schema-loading#valid-workers-load
#[test]
fn disabled_worker_loads_but_is_not_selectable() {
    let d = fresh_dir("disabled");
    let (tasks, workers) = write_config(
        &d,
        ONE_TASK,
        r#"{ "workers": [
            {"name":"on","provider":"p","model":"m","enabled":true},
            {"name":"off","provider":"p","model":"m","enabled":false}
        ] }"#,
    );
    let cfg = load(&tasks, &workers).expect("a disabled worker must not fail load");

    // Both records survive loading...
    assert_eq!(cfg.workers.len(), 2);
    // ...but only the enabled set is what a caller may dispatch to.
    let enabled: Vec<&str> = cfg
        .workers
        .iter()
        .filter(|w| w.enabled)
        .map(|w| w.name.as_str())
        .collect();
    assert_eq!(enabled, vec!["on"]);

    let busy = Mutex::new(HashMap::new());
    let router = Router::default();
    // Unfiltered selection never returns the disabled worker.
    assert_eq!(pick_worker(&cfg, &busy, &router, None).unwrap().name, "on");
    // Naming it explicitly is still not enough.
    assert!(
        pick_worker(&cfg, &busy, &router, Some("off")).is_none(),
        "a disabled worker must never be selected, even when named"
    );
    assert_eq!(
        pick_worker(&cfg, &busy, &router, Some("on")).unwrap().name,
        "on"
    );
}

/// A worker's cost basis is DECLARED by the operator, never inferred:
/// `params_b` (billions of parameters — a proxy for expense) and
/// `price_per_mtok_usd` (real USD per million tokens; beats the proxy)
/// are optional, absent = neutral, and a workers.json that never
/// mentions them loads byte-for-byte unchanged.
// spec: config/worker-schema-loading#cost-basis-is-declared-and-optional
#[test]
fn worker_cost_basis_is_declared_and_defaults_to_neutral() {
    let d = fresh_dir("cost-basis");

    // One worker declares BOTH fields; the other declares NEITHER.
    let (tasks, workers) = write_config(
        &d,
        ONE_TASK,
        r#"{ "workers": [
            {"name":"big","provider":"p","model":"m","params_b":235,"price_per_mtok_usd":1.25},
            {"name":"plain","provider":"p","model":"m"}
        ] }"#,
    );
    let cfg = load(&tasks, &workers).expect("declared cost basis loads");
    let big = cfg
        .workers
        .iter()
        .find(|w| w.name == "big")
        .expect("big worker present");
    assert_eq!(big.params_b, Some(235.0), "params_b survives loading");
    assert_eq!(
        big.price_per_mtok_usd,
        Some(1.25),
        "price_per_mtok_usd survives loading"
    );
    let plain = cfg
        .workers
        .iter()
        .find(|w| w.name == "plain")
        .expect("plain worker present");
    assert_eq!(plain.params_b, None, "absent params_b => neutral (None)");
    assert_eq!(
        plain.price_per_mtok_usd, None,
        "absent price_per_mtok_usd => neutral (None)"
    );

    // The struct Default agrees: a constructed worker starts neutral.
    let w = Worker::default();
    assert_eq!(w.params_b, None);
    assert_eq!(w.price_per_mtok_usd, None);

    // An OLD workers.json that never mentions the fields still loads
    // unchanged — and its worker is neutral, exactly as before.
    let (old_t, old_w) = write_config(&d.join("legacy"), ONE_TASK, ONE_WORKER);
    let old = load(&old_t, &old_w).expect("legacy workers.json keeps loading");
    assert_eq!(old.workers.len(), 1);
    assert_eq!(old.workers[0].params_b, None);
    assert_eq!(old.workers[0].price_per_mtok_usd, None);
}

/// A declared value that is not usable (not finite, or <= 0) is a CONFIG
/// ERROR naming the worker and the field — a NaN or non-positive weight
/// would silently defeat cost comparison. (JSON cannot spell NaN, so the
/// file-reachable shapes are 0 and negatives; the finite guard also
/// covers programmatic NaN/infinity.)
// spec: config/worker-schema-loading#unusable-cost-basis-values-are-rejected
#[test]
fn unusable_cost_basis_is_rejected_naming_worker_and_field() {
    let d = fresh_dir("cost-basis-bad");
    for (label, workers_body) in [
        (
            "params_b zero",
            r#"{ "workers": [{"name":"zai","provider":"p","model":"m","params_b":0}] }"#,
        ),
        (
            "params_b negative",
            r#"{ "workers": [{"name":"zai","provider":"p","model":"m","params_b":-3}] }"#,
        ),
        (
            "price zero",
            r#"{ "workers": [{"name":"zai","provider":"p","model":"m","price_per_mtok_usd":0}] }"#,
        ),
        (
            "price negative",
            r#"{ "workers": [{"name":"zai","provider":"p","model":"m","price_per_mtok_usd":-0.5}] }"#,
        ),
    ] {
        let (tasks, workers) =
            write_config(&d.join(label.replace(' ', "-")), ONE_TASK, workers_body);
        let err = load(&tasks, &workers).unwrap_err();
        assert!(err.contains("zai"), "{label}: names the worker: {err}");
        assert!(
            err.contains("params_b") || err.contains("price_per_mtok_usd"),
            "{label}: names the field: {err}"
        );
        assert!(
            err.contains("finite positive"),
            "{label}: states the rule: {err}"
        );
    }

    // Control: usable declarations (a declared price, a declared size)
    // load cleanly — only UNUSABLE values are rejected.
    let (tasks, workers) = write_config(
        &d.join("good"),
        ONE_TASK,
        r#"{ "workers": [{"name":"zai","provider":"p","model":"m","params_b":8,"price_per_mtok_usd":0.25}] }"#,
    );
    assert!(load(&tasks, &workers).is_ok(), "usable declarations load");
}

/// An ENABLED worker declaring neither basis warns exactly once (cost
/// estimates will be neutral for it); a fully-declared worker does not,
/// and neither does a DISABLED bare worker — it is never dispatched.
// spec: config/worker-schema-loading#missing-cost-basis-warns
#[test]
fn missing_cost_basis_warns_once_for_enabled_workers_only() {
    let d = fresh_dir("cost-basis-warn");
    let (tasks, workers) = write_config(
        &d,
        ONE_TASK,
        r#"{ "workers": [
            {"name":"bare","provider":"p","model":"m"},
            {"name":"rich","provider":"p","model":"m","params_b":8},
            {"name":"priced","provider":"p","model":"m","price_per_mtok_usd":0.25},
            {"name":"off","provider":"p","model":"m","enabled":false}
        ] }"#,
    );
    let cfg = load(&tasks, &workers).expect("a missing basis warns, never fails");
    let basis_warnings: Vec<&String> = cfg
        .warnings
        .iter()
        .filter(|w| w.contains("no cost basis"))
        .collect();
    assert_eq!(
        basis_warnings.len(),
        1,
        "exactly one warning: {:?}",
        cfg.warnings
    );
    assert!(
        basis_warnings[0].contains("\"bare\"") && basis_warnings[0].contains("neutral"),
        "warning names the bare worker: {}",
        basis_warnings[0]
    );
    assert!(
        !cfg.warnings.iter().any(|w| w.contains("\"rich\"")),
        "a declared params_b silences the warning"
    );
    assert!(
        !cfg.warnings.iter().any(|w| w.contains("\"priced\"")),
        "a declared price silences the warning"
    );
    assert!(
        !cfg.warnings.iter().any(|w| w.contains("\"off\"")),
        "a disabled worker never warns"
    );
}

/// `detect_cycle` accepts every acyclic DAG shape the scheduler relies on
/// (chain, diamond) and rejects a cycle with a message naming a member. A
/// dep naming a NON-EXISTENT id is not a cycle here; it is rejected when the
/// DAG is actually resolved for dispatch (`scheduler::compute_depths` →
/// "unknown task"), while `load` merely warns — compat with configs
/// composed across sibling files.
// spec: config/task-schema-loading#dependency-cycle-rejected
// spec: config/task-schema-loading#dangling-dependency-warns
#[test]
fn detect_cycle_accepts_dags_and_rejects_cycles_and_bad_deps() {
    // Chain A <- B <- C.
    let chain = vec![task("A", &[]), task("B", &["A"]), task("C", &["B"])];
    detect_cycle(&chain).expect("a chain is a valid DAG");

    // Diamond: D depends on B and C, both depend on A.
    let diamond = vec![
        task("A", &[]),
        task("B", &["A"]),
        task("C", &["A"]),
        task("D", &["B", "C"]),
    ];
    detect_cycle(&diamond).expect("a diamond is a valid DAG");

    // A two-node cycle names its members.
    let cycle = vec![task("A", &["B"]), task("B", &["A"])];
    let err = detect_cycle(&cycle).unwrap_err();
    assert!(err.contains("dependency cycle"), "cycle error: {err}");
    assert!(
        err.contains('A') && err.contains('B'),
        "names a member: {err}"
    );

    // Self-dependency is a cycle too.
    let self_cycle = vec![task("S", &["S"])];
    let err = detect_cycle(&self_cycle).unwrap_err();
    assert!(
        err.contains("dependency cycle") && err.contains('S'),
        "{err}"
    );

    // A dep on a non-existent id is NOT a cycle, but it is a hard error
    // once the DAG is resolved: `load` warns, then `compute_depths` refuses
    // with "unknown task". Pin both halves of that contract.
    let d = fresh_dir("dangling");
    let (tasks, workers) = write_config(
        &d,
        r#"{ "tasks": [{"id":"A","title":"a","deps":["NOPE"],"accept":"true"}] }"#,
        ONE_WORKER,
    );
    let cfg = load(&tasks, &workers).expect("a dangling dep warns, it does not fail load");
    assert!(
        cfg.warnings.iter().any(|w| w.contains("NOPE")),
        "load warns about the missing dep: {:?}",
        cfg.warnings
    );
    detect_cycle(&cfg.tasks).expect("a dangling dep is not a cycle");
    let err = agentflow::scheduler::compute_depths(&cfg).unwrap_err();
    assert!(
        err.contains("unknown task") && err.contains("NOPE"),
        "the unresolvable dep is rejected when the DAG is resolved: {err}"
    );
}

/// `parse_gate_env` splits `TF_GATE_ENV` on whitespace and then on the FIRST
/// `=` of each token; tokens without an `=` are dropped. An empty (or
/// whitespace-only) string yields an empty vec.
#[test]
fn parse_gate_env_contract() {
    // Empty / whitespace-only input => empty vec.
    assert!(parse_gate_env("").is_empty());
    assert!(parse_gate_env("   \t\n").is_empty());

    // Space-separated KEY=VALUE pairs.
    assert_eq!(
        parse_gate_env("A=1 FOO=bar baz=hello world"),
        vec![
            ("A".to_string(), "1".to_string()),
            ("FOO".to_string(), "bar".to_string()),
            ("baz".to_string(), "hello".to_string()),
        ],
        "tokens without '=' (world) are dropped"
    );

    // Only the FIRST '=' separates; later '=' characters stay in the value.
    assert_eq!(
        parse_gate_env("URL=https://x/y?a=b"),
        vec![("URL".to_string(), "https://x/y?a=b".to_string())]
    );

    // Empty key and empty value are still emitted as pairs.
    assert_eq!(
        parse_gate_env("=v EMPTY="),
        vec![
            (String::new(), "v".to_string()),
            ("EMPTY".to_string(), String::new()),
        ]
    );

    // Any whitespace separates, not just spaces.
    assert_eq!(
        parse_gate_env("A=1\tB=2\nC=3"),
        vec![
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
            ("C".to_string(), "3".to_string()),
        ]
    );
}

/// `load_repos` maps names to paths (relative paths resolve against the
/// repos.json directory) and returns `Err` for a malformed/unreadable file.
/// A MISSING file is single-repo mode — `Ok(empty)`, not an error.
// spec: config/optional-repos-json-defines-named-repositories#missing-repos-json-is-single-repo-mode
// spec: config/optional-repos-json-defines-named-repositories#relative-repo-paths-resolve-against-the-file
#[test]
fn load_repos_maps_names_to_paths_and_rejects_malformed() {
    let d = fresh_dir("repos");

    // Missing file => Ok(empty) = single-repo mode.
    let missing = load_repos(&d.join("config").join("repos.json")).unwrap();
    assert!(missing.is_empty());

    // Names map to paths; relative paths resolve against the file's dir.
    let abs = if cfg!(windows) {
        "C:/abs/docs"
    } else {
        "/abs/docs"
    };
    let body = format!(r#"{{"repos": {{"main": "../main-repo", "docs": "{abs}"}}}}"#);
    let repos_file = write_file(&d.join("cfg").join("repos.json"), &body);
    let map = load_repos(&repos_file).unwrap();
    assert_eq!(map.len(), 2);
    assert!(map.contains_key("main") && map.contains_key("docs"));
    // cfg/../main-repo normalizes (lexically) to <d>/main-repo.
    assert_eq!(map["main"], d.join("main-repo"));
    assert_eq!(map["docs"], PathBuf::from(abs));

    // Malformed JSON => Err.
    let bad = write_file(&d.join("bad-repos.json"), "{ not json");
    let err = load_repos(&bad).unwrap_err();
    assert!(err.contains("parse"), "{err}");

    // An existing-but-unreadable path (a directory) is a read error, not a
    // silently-empty map.
    let dir_as_file = d.join("a-directory");
    std::fs::create_dir_all(&dir_as_file).unwrap();
    let err = load_repos(&dir_as_file).unwrap_err();
    assert!(err.contains("read"), "{err}");
}

/// A declared `touch` entry that no `scope` entry covers is UNPASSABLE BY
/// CONSTRUCTION — the task must edit a file its own scope forbids — so it
/// is a hard load error (exit 2 from `af validate`), never a warning. The
/// headline round-12 contract: fail fast, before any agent is paid.
// spec: config/task-schema-loading#touch-entry-not-covered-by-scope-is-rejected
#[test]
fn validate_rejects_a_touch_entry_the_scope_does_not_cover() {
    let d = fresh_dir("touch-reject");
    let (tasks, workers) = write_config(
        &d,
        r#"{ "tasks": [{
            "id":"T","title":"t","accept":"true",
            "scope":["src/config.rs","tests/contract_config.rs"],
            "touch":["src/config.rs","src/router.rs"]
        }] }"#,
        ONE_WORKER,
    );
    let err = load(&tasks, &workers)
        .expect_err("an uncovered touch entry is a hard config error, not a warning");
    assert!(err.contains("task 'T'"), "names the task: {err}");
    assert!(err.contains("src/router.rs"), "names the path: {err}");
    assert!(
        err.contains("is covered by no scope entry"),
        "states the rule: {err}"
    );
    // Deterministic message: the scope entries are named, joined with ", ",
    // so the fix (widen scope or drop the entry) is obvious.
    assert!(
        err.contains("(src/config.rs, tests/contract_config.rs)"),
        "names the scope: {err}"
    );
    // The covered entry never appears as the offender — only the uncovered
    // one does (first uncovered entry in declaration order wins).
    let first = err
        .lines()
        .find(|l| l.contains("touch entry"))
        .expect("the error names a touch entry");
    assert!(
        first.contains("src/router.rs"),
        "first offender only: {first}"
    );
}

/// The covered arm: a `touch` entry the scope covers — by an EXACT path,
/// a DIRECTORY PREFIX, or a GLOB — loads cleanly (the declared entries
/// survive loading), because enforcement (`scope_violations`) would accept
/// an edit to each of those paths. Only the uncovered shape is an error.
// spec: config/task-schema-loading#touch-entry-not-covered-by-scope-is-rejected
#[test]
fn touch_entries_covered_by_scope_load() {
    let d = fresh_dir("touch-covered");
    // One scope entry per coverage kind: exact, directory prefix, glob.
    let (tasks, workers) = write_config(
        &d,
        r#"{ "tasks": [{
            "id":"T","title":"t","accept":"true",
            "scope":["src/config.rs","docs/","src/*.rs"],
            "touch":["src/config.rs","docs/deep/nested/guide.md","src/anything.rs"]
        }] }"#,
        ONE_WORKER,
    );
    let cfg = load(&tasks, &workers).expect("fully covered touch entries load");
    // The declaration is data too: it survives loading verbatim.
    assert_eq!(
        cfg.by_id["T"].touch,
        vec![
            "src/config.rs".to_string(),
            "docs/deep/nested/guide.md".to_string(),
            "src/anything.rs".to_string(),
        ],
        "touch survives loading in declaration order"
    );
    // And it adds no warning of its own — the check is silent when happy.
    assert!(
        !cfg.warnings.iter().any(|w| w.contains("touch")),
        "a covered touch never warns: {:?}",
        cfg.warnings
    );
}

/// The inert arms: an absent/empty `touch` is the normal case (all 51
/// existing campaign tasks declare none) — no warning, no error, no output
/// change at all; and an EMPTY `scope` means "any file" (existing
/// semantics), so EVERY `touch` entry is covered and nothing is rejected.
// spec: config/task-schema-loading#touch-entry-not-covered-by-scope-is-rejected
#[test]
fn empty_touch_and_empty_scope_arms_are_inert() {
    let d = fresh_dir("touch-inert");

    // --- Absent touch: byte-for-byte the legacy behaviour.
    let (t_absent, w) = write_config(
        &d.join("absent"),
        r#"{ "tasks": [{"id":"T","title":"t","accept":"true","scope":["src/"]}] }"#,
        ONE_WORKER,
    );
    let cfg = load(&t_absent, &w).expect("absent touch is the normal case");
    assert!(cfg.by_id["T"].touch.is_empty(), "absent => empty vec");
    assert_eq!(
        cfg.warnings.len(),
        1,
        "only the pre-existing cost-basis warning: {:?}",
        cfg.warnings
    );
    assert!(
        !cfg.warnings.iter().any(|x| x.contains("touch")),
        "never warns about touch"
    );

    // --- Explicitly empty touch: identical.
    let (t_empty, w2) = write_config(
        &d.join("empty"),
        r#"{ "tasks": [{"id":"T","title":"t","accept":"true","scope":["src/"],"touch":[]}] }"#,
        ONE_WORKER,
    );
    let cfg = load(&t_empty, &w2).expect("empty touch is the normal case");
    assert!(cfg.by_id["T"].touch.is_empty());
    assert!(
        !cfg.warnings.iter().any(|x| x.contains("touch")),
        "never warns about touch"
    );

    // --- Empty scope = "any file": every touch entry is covered, even one
    // that no literal string could prefix-match. Never rejected.
    let (t_open, w3) = write_config(
        &d.join("open-scope"),
        r#"{ "tasks": [{"id":"T","title":"t","accept":"true","scope":[],"touch":["src/router.rs","anywhere/deep/file.rs"]}] }"#,
        ONE_WORKER,
    );
    let cfg = load(&t_open, &w3).expect("empty scope covers every touch entry");
    assert_eq!(cfg.by_id["T"].touch.len(), 2, "the entries still parse");
    assert!(
        !cfg.warnings.iter().any(|x| x.contains("touch")),
        "never warns about touch"
    );
}

/// `TaskState::is_terminal` is the completion predicate the run loop uses.
#[test]
fn task_state_terminality_contract() {
    assert!(!TaskState::Ready.is_terminal());
    assert!(!TaskState::Running.is_terminal());
    assert!(TaskState::Done.is_terminal());
    assert!(TaskState::Failed.is_terminal());
}

/// `Priority` accepts numbers and the corpus' string levels and ranks them
/// deterministically (tie-break order documented in src/config.rs).
// spec: config/task-schema-loading#string-priority-levels-accepted
#[test]
fn priority_rank_contract() {
    assert_eq!(Priority::default().rank(), 0);
    assert_eq!(Priority::Num(7).rank(), 7);
    assert_eq!(Priority::Num(-3).rank(), -3);
    for (level, rank) in [
        ("LOW", -10),
        ("MINOR", -10),
        ("MEDIUM", 0),
        ("NORMAL", 0),
        ("MED", 0),
        ("HIGH", 10),
        ("CRITICAL", 20),
        ("URGENT", 20),
        ("unknown", 0),
    ] {
        assert_eq!(
            Priority::Str(level.to_string()).rank(),
            rank,
            "rank of {level}"
        );
    }
    // Matching is case-insensitive.
    assert_eq!(Priority::Str("high".to_string()).rank(), 10);
}

/// Saves one env var and restores it (present OR absent) on drop, so the
/// ambient environment survives the Settings test byte-identical even when
/// an assertion panics mid-test.
struct EnvVar {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let prior = std::env::var(key).ok();
        std::env::set_var(key, value);
        EnvVar { key, prior }
    }

    fn remove(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        std::env::remove_var(key);
        EnvVar { key, prior }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match self.prior.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// `Settings::from_env` is the ONLY public constructor, and it reads PROCESS
/// env. cargo runs each integration-test FILE as its own process, and this is
/// the only test in this binary that touches `TF_*` (or constructs
/// `Settings`), so the mutations cannot race a sibling here. Every mutation
/// goes through `EnvVar` (restored on drop) and all assertions for a phase
/// are ordered together inside this single test.
// spec: config/environment-overrides#defaults-when-unset
// spec: config/environment-overrides#overrides-applied
#[test]
fn settings_env_defaults_and_overrides_contract() {
    // Belt-and-braces serialization within THIS binary (see docs above):
    // only this test takes the lock, but it documents the intent.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    const KEYS: [&str; 11] = [
        "TF_REPO_DIR",
        "TF_STATE_DIR",
        "TF_WORKTREE_ROOT",
        "TF_MAX_PARALLEL",
        "TF_BRANCH_PREFIX",
        "TF_POLL",
        "TF_GATE_ENV",
        "TF_TASKS_JSON",
        "TF_WORKERS_JSON",
        "TF_AGENT_TIMEOUT_S",
        "TF_SANDBOX_CMD",
    ];

    // Phase 1: every TF_* unset => documented defaults.
    {
        let _cleared: Vec<EnvVar> = KEYS.into_iter().map(EnvVar::remove).collect();
        let s = Settings::from_env();
        assert_eq!(s.repo_dir, PathBuf::from("."));
        assert_eq!(s.state_dir, PathBuf::from("state"));
        assert_eq!(s.worktree_root, PathBuf::from("state").join("worktrees"));
        assert_eq!(
            s.max_parallel, 0,
            "0 means 'one slot per enabled worker' downstream"
        );
        assert_eq!(s.branch_prefix, "tf");
        assert_eq!(s.poll_secs, 15);
        assert!(s.gate_env.is_empty());
        assert_eq!(s.tasks_file, PathBuf::from("config/tasks.json"));
        assert_eq!(s.workers_file, PathBuf::from("config/workers.json"));
        assert_eq!(s.prompt_file, PathBuf::from("prompts/worker.md"));
        assert_eq!(s.agent_timeout_s, 3600);
        assert!(s.sandbox_cmd.is_empty());
    } // Guards drop here: the ambient env is restored before phase 2.

    // Phase 2: explicit values win.
    let repo = fresh_dir("settings-repo");
    let state = fresh_dir("settings-state");
    {
        let _set = vec![
            // Unset the keys we are NOT overriding, so the phase is
            // deterministic regardless of the host environment.
            EnvVar::remove("TF_WORKTREE_ROOT"),
            EnvVar::remove("TF_TASKS_JSON"),
            EnvVar::remove("TF_WORKERS_JSON"),
            EnvVar::remove("TF_SANDBOX_CMD"),
            EnvVar::set("TF_REPO_DIR", repo.to_str().unwrap()),
            EnvVar::set("TF_STATE_DIR", state.to_str().unwrap()),
            EnvVar::set("TF_BRANCH_PREFIX", "contract"),
            EnvVar::set("TF_MAX_PARALLEL", "5"),
            EnvVar::set("TF_POLL", "3"),
            EnvVar::set("TF_AGENT_TIMEOUT_S", "42"),
            EnvVar::set("TF_GATE_ENV", "A=1 B=hello"),
        ];
        let s = Settings::from_env();
        assert_eq!(s.repo_dir, repo);
        assert_eq!(s.state_dir, state);
        assert_eq!(s.worktree_root, state.join("worktrees"));
        assert_eq!(s.max_parallel, 5);
        assert_eq!(s.branch_prefix, "contract");
        assert_eq!(s.poll_secs, 3);
        assert_eq!(s.agent_timeout_s, 42);
        assert_eq!(
            s.gate_env,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "hello".to_string()),
            ]
        );
    }
}

// spec: docs/superpowers/specs/2026-10-09-native-rust-harness-design.md
// `cli: "builtin"` selects the native harness. Config-time validation must
// catch unusable combos BEFORE any agent is paid.
#[test]
fn builtin_worker_requires_api_base() {
    let err = config::load_workers_str(
        r#"{ "workers": [ { "name": "nat", "provider": "p", "model": "m",
             "cli": "builtin", "api_base": "   ", "enabled": true } ] }"#,
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
fn builtin_worker_with_explicit_max_turns_parses_and_validates() {
    let cfg = config::load_workers_str(
        r#"{ "workers": [ { "name": "nat", "provider": "p", "model": "m",
             "api_base": "http://x/v1", "cli": "builtin", "max_turns": 5,
             "enabled": true } ] }"#,
    )
    .unwrap();
    assert_eq!(cfg.workers[0].max_turns, Some(5));
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
