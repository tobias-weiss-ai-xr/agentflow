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
    assert_eq!(cfg.defaults.retry_delay_s, 30);

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
    assert_eq!(wd_absent.defaults.retry_delay_s, 30);
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
        (600, 3, 30, 3600)
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
