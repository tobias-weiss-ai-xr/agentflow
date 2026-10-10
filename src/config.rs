//! Config: task/worker schemas + validation + `TF_*` environment overrides.
//!
//! Schema shapes are compatible with taskfleet's shipped config examples so
//! existing campaign configs work as the integration corpus (ADR-4).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Data models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Ready,
    Running,
    Done,
    Failed,
}

impl TaskState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskState::Done | TaskState::Failed)
    }
}

/// Priority rank: accepts a number or the corpus' string levels (LOW/MEDIUM/
/// HIGH/CRITICAL). Used as a tie-breaker when multiple tasks are ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Priority {
    Num(i64),
    Str(String),
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Num(0)
    }
}

impl Priority {
    pub fn rank(&self) -> i64 {
        match self {
            Priority::Num(n) => *n,
            Priority::Str(s) => match s.to_uppercase().as_str() {
                "LOW" | "MINOR" => -10,
                "MEDIUM" | "NORMAL" | "MED" => 0,
                "HIGH" => 10,
                "CRITICAL" | "URGENT" => 20,
                _ => 0,
            },
        }
    }
}

/// Serde default for `Task::gate_replay`: the flag is opt-out — tasks that
/// omit it parse as `true` (the gate is assumed replay-safe).
fn default_true() -> bool {
    true
}

/// Serde default for `Worker::output`: the field is opt-in — a workers.json
/// that omits it keeps today's behaviour exactly (text mode: raw agent
/// output in the log, no token capture), no migration needed.
fn default_output() -> String {
    "text".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub deps: Vec<String>,
    pub scope: Vec<String>,
    /// The files the operator BELIEVES the agent must edit (round-12).
    /// Optional and purely declarative: absent/empty = the normal case,
    /// no behaviour change at all. Its only consumer is `validate`, which
    /// REJECTS (hard error, so `af validate` exits 2) a `touch` entry that
    /// no `scope` entry covers — such a task is unpassable by construction:
    /// it must edit a file its own scope forbids, so the failure is caught
    /// at config time, before any agent is paid. Coverage is decided by
    /// the SAME matcher enforcement uses (`scheduler::scope_overlap`), so
    /// validation can never accept a config enforcement will later reject.
    #[serde(default)]
    pub touch: Vec<String>,
    pub accept: Option<String>,
    pub acceptance_prose: Option<String>,
    pub manual: bool,
    pub priority: Priority,
    pub repo: String,
    /// Whether the acceptance gate is replay-safe: re-running `accept` after
    /// an interruption is allowed. Defaults to `true`; declare `false` for a
    /// gate with side effects (mirrors pi-durable's `replay: "safe"` marks).
    #[serde(default = "default_true")]
    pub gate_replay: bool,
    /// Per-task turn cap for the builtin harness (`cli: "builtin"`) — the
    /// loop stops (and the attempt fails, archive-preserving) after this many
    /// LLM round-trips on THIS task. Overrides the worker's `max_turns`, then
    /// `TF_AGENT_MAX_TURNS`, then the harness default of 32. Ignored for CLI
    /// workers. Absent = no per-task override.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Investigation-only task: the builtin harness rejects every `write` and
    /// `edit` tool call, and the attempt completes when the agent stops
    /// normally — no acceptance gate, no merge. Validation rejects combining
    /// `readonly` with an `accept` gate (contradictory). Default `false`.
    #[serde(default)]
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Worker {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub api_base: Option<String>,
    /// Env var holding this worker's API key; the agent child gets ONLY this
    /// key from the orchestrator environment (sandbox layer 1).
    pub api_key_env: Option<String>,
    pub enabled: bool,
    /// Agent CLI binary name; default `pi`. Passed `--provider/--model/-p @file`.
    pub cli: String,
    /// Optional full shell command template (`command`). `{prompt}` is
    /// replaced by the absolute prompt-file path; the string runs via the
    /// platform shell (`sh -c` / `cmd /C`) inside the worktree with the
    /// inherited environment (user-authored, same trust class as gates).
    /// Parity with the Go port's GOWORKER feature — this is how CLIs whose
    /// argument shape differs from `cli --provider/--model/-p @file`
    /// (e.g. `opencode run -m M "<prompt>"`) plug in. When absent, the
    /// legacy argv dispatch is used.
    #[serde(default)]
    pub command: Option<String>,
    /// Agent CLI output mode (`output`): `"text"` (the default — the
    /// legacy behaviour: raw stdout/stderr in the task log, no token
    /// capture, receipt `tokens: None`) or `"json"` (the CLI is spawned
    /// with `--mode json` and its JSON Lines transcript is parsed: the
    /// log gets a human-readable rendering and the attempt receipt
    /// carries the transcript's final `totalTokens`; see `transcript`).
    /// Any other value is rejected at load time.
    #[serde(default = "default_output")]
    pub output: String,
    /// Extra arguments for the agent CLI (per worker). Appended to the
    /// argv AFTER `--model M` and BEFORE `-p @file` (see `spawn_argv`) — so
    /// a caller can reach CLI flags agentflow does not model (`--max-turns`,
    /// effort/reasoning settings, `--mode json`, cost/limit flags, …) and
    /// can override anything the earlier argv set, while the `-p @file`
    /// prompt handoff always stays last. Passed through VERBATIM — agentflow
    /// stays CLI-agnostic (ADR-1) and never interprets or whitelists the
    /// entries. Absent field ⇒ empty vector (existing `workers.json`
    /// files keep working unchanged).
    #[serde(default)]
    pub args: Vec<String>,
    /// Model size in BILLIONS of parameters — an operator-DECLARED proxy
    /// for expense, used when the provider reports no price. Absent =
    /// neutral (no opinion), so an existing workers.json keeps working
    /// unchanged. This is a declared assumption, NOT vendor data:
    /// agentflow never guesses a model's size from its name.
    #[serde(default)]
    pub params_b: Option<f64>,
    /// Real price in USD per MILLION tokens, when the operator knows it.
    /// A declared price beats the `params_b` proxy. Also a declared
    /// assumption, not vendor data; absent = neutral.
    #[serde(default)]
    pub price_per_mtok_usd: Option<f64>,
    /// Turn cap for the native harness (`cli: "builtin"`): the loop stops
    /// (and the attempt fails, archive-preserving) after this many LLM
    /// round-trips. Absent = `TF_AGENT_MAX_TURNS` if set (>0), else 32.
    /// Ignored for CLI workers (their flags live in `args`).
    #[serde(default)]
    pub max_turns: Option<u32>,
}

impl Default for Worker {
    fn default() -> Self {
        Worker {
            name: String::new(),
            provider: String::new(),
            model: String::new(),
            api_base: None,
            api_key_env: None,
            enabled: true,
            cli: "pi".to_string(),
            command: None,
            output: default_output(),
            args: Vec::new(),
            params_b: None,
            price_per_mtok_usd: None,
            max_turns: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerDefaults {
    pub accept_timeout_s: u64,
    pub max_attempts: u32,
    /// Backoff BETWEEN attempts of the same task (`retry_delay_s`): after
    /// a failed attempt that will be retried, the task waits this many
    /// seconds before its next attempt starts (see run.rs's dispatch
    /// thread — the wait rides the retry path only, so first attempts,
    /// first-try merges and other workers' dispatches are never delayed).
    ///
    /// Default **0** = no delay, strictly OPT-IN: agentflow's cost is
    /// dominated by the agent run itself, so an unexplained 30s pause per
    /// retry is pure added wall-clock, and backoff only helps against
    /// provider rate limits — a campaign that wants it sets it explicitly.
    pub retry_delay_s: u64,
    pub agent_timeout_s: u64,
}

impl Default for WorkerDefaults {
    fn default() -> Self {
        WorkerDefaults {
            accept_timeout_s: 600,
            max_attempts: 3,
            // Opt-in backoff (see the field doc above): 0 keeps the legacy
            // no-delay behaviour for every existing workers.json that omits
            // the field; one that SETS it keeps meaning what it says.
            retry_delay_s: 0,
            agent_timeout_s: 3600,
        }
    }
}

// ---------------------------------------------------------------------------
// Loaded config
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Config {
    pub tasks: Vec<Task>,
    pub workers: Vec<Worker>,
    pub defaults: WorkerDefaults,
    pub by_id: HashMap<String, Task>,
    /// Named repositories (multi-repo mode, optional). Missing/empty map =
    /// single-repo mode: every task targets `TF_REPO_DIR`.
    pub repos: BTreeMap<String, PathBuf>,
    /// Non-fatal compatibility notes (dangling deps, gate-less tasks) —
    /// taskfleet's corpus relies on these, so they warn instead of fail.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TasksFile {
    #[serde(default)]
    _meta: Option<serde_json::Value>,
    #[serde(default)]
    tasks: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize)]
struct WorkersFile {
    #[serde(default)]
    defaults: WorkerDefaults,
    #[serde(default)]
    workers: Vec<Worker>,
}

pub fn load(tasks_path: &Path, workers_path: &Path) -> Result<Config, String> {
    let tasks_json = std::fs::read_to_string(tasks_path)
        .map_err(|e| format!("read {}: {e}", tasks_path.display()))?;
    let tf: TasksFile = serde_json::from_str(&tasks_json)
        .map_err(|e| format!("parse {}: {e}", tasks_path.display()))?;

    let workers_json = std::fs::read_to_string(workers_path)
        .map_err(|e| format!("read {}: {e}", workers_path.display()))?;
    let wf: WorkersFile = serde_json::from_str(&workers_json)
        .map_err(|e| format!("parse {}: {e}", workers_path.display()))?;

    let mut warnings = validate(&tf.tasks, &wf.workers)?;

    let by_id: HashMap<String, Task> = tf.tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();

    // Dangling deps are non-fatal (compat: configs reference tasks merged in
    // from sibling files). Report them; deadlock detection will surface any
    // dep that truly never resolves.
    for t in &tf.tasks {
        for d in &t.deps {
            if !by_id.contains_key(d) {
                warnings.push(format!(
                    "task '{}': dep '{}' not in this file (assumed merged elsewhere)",
                    t.id, d
                ));
            }
        }
    }

    Ok(Config {
        repos: BTreeMap::new(),
        tasks: tf.tasks,
        workers: wf.workers,
        defaults: wf.defaults,
        by_id,
        warnings,
    })
}

/// A declared cost-basis number is usable only when finite and strictly
/// positive (the same predicate `crate::cost::is_usable` expresses): a NaN
/// or non-positive weight would silently defeat cost comparison, which is
/// why an unusable declaration is a load error rather than a warning.
fn cost_basis_is_usable(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// Returns warnings (non-fatal) or a hard error.
fn validate(tasks: &[Task], workers: &[Worker]) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    // unique ids
    let mut seen: HashSet<&str> = HashSet::new();
    for t in tasks {
        if t.id.is_empty() {
            return Err("task with empty id".into());
        }
        if !seen.insert(t.id.as_str()) {
            return Err(format!("duplicate task id: {}", t.id));
        }
    }

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

    for t in tasks {
        if t.max_turns == Some(0) {
            return Err(format!("task '{}': max_turns must be >= 1", t.id));
        }
        if t.readonly && t.accept.is_some() {
            return Err(format!(
                "task '{}': readonly task cannot declare an accept gate",
                t.id
            ));
        }
        // No gate and not manual: legacy corpus runs these gate-less; warn.
        if t.accept.is_none() && !t.manual && !t.readonly {
            warnings.push(format!(
                "task '{}': no acceptance gate and not manual — gate will be skipped",
                t.id
            ));
        }
    }

    // A declared `touch` entry no `scope` entry covers is a hard error, not
    // a warning: the task would be unpassable by construction (it must edit
    // a file its own scope forbids), so it must fail at config time, before
    // any agent is paid. Coverage uses the SAME matcher as enforcement
    // (`scheduler::scope_overlap`, see `execute::scope_violations`) — never
    // a second glob implementation — so admission control and enforcement
    // agree on what "in scope" means. An EMPTY `scope` means "any file"
    // (existing semantics): every `touch` entry is covered, nothing is
    // rejected. An empty/absent `touch` is the normal case: no output
    // change at all. Tasks are visited in file order, `touch` entries in
    // declaration order, so the first offender is reported deterministically.
    for t in tasks {
        if t.scope.is_empty() {
            continue;
        }
        for entry in &t.touch {
            if !t
                .scope
                .iter()
                .any(|s| crate::scheduler::scope_overlap(entry, s))
            {
                return Err(format!(
                    "task '{}': touch entry '{}' is covered by no scope entry ({}) — widen scope or drop the entry",
                    t.id,
                    entry,
                    t.scope.join(", ")
                ));
            }
        }
    }
    detect_cycle(tasks)?;

    let mut wnames: HashSet<&str> = HashSet::new();
    let mut enabled = 0;
    for w in workers {
        if w.name.is_empty() {
            return Err("worker with empty name".into());
        }
        if w.provider.is_empty() || w.model.is_empty() {
            return Err(format!(
                "worker '{}': provider and model are required",
                w.name
            ));
        }
        // CLI-agnostic (ADR-1): entries are passed through verbatim, so the
        // only shape rule is that each is a non-empty string (an empty
        // entry is near-certainly a config typo, and would surface only as
        // a confusing agent-CLI error at dispatch time).
        if w.args.iter().any(|a| a.is_empty()) {
            return Err(format!(
                "worker '{}': args entries must be non-empty strings",
                w.name
            ));
        }
        // The output mode is a closed vocabulary — a typo ("jsn") must
        // not silently disable token capture: fail at load time, naming
        // the worker and the accepted values.
        if w.output != "text" && w.output != "json" {
            return Err(format!(
                "worker '{}': output must be one of [text, json] (got '{}')",
                w.name, w.output
            ));
        }
        if !wnames.insert(w.name.as_str()) {
            return Err(format!("duplicate worker name: {}", w.name));
        }
        if w.enabled {
            enabled += 1;
        }
        // Declared cost basis: an UNUSABLE declared value (not finite, or
        // <= 0) is a hard error naming the worker and the field — a NaN or
        // non-positive weight would silently defeat cost comparison.
        // Absent is fine (neutral, warned below).
        for (field, value) in [
            ("params_b", &w.params_b),
            ("price_per_mtok_usd", &w.price_per_mtok_usd),
        ] {
            if let Some(v) = value {
                if !cost_basis_is_usable(*v) {
                    return Err(format!(
                        "worker \"{}\": {field} must be a finite positive number",
                        w.name
                    ));
                }
            }
        }
        // No declared basis at all: one warning per worker, so `af
        // validate`'s warning count reflects the neutral estimate. Only
        // ENABLED workers warn — a disabled worker is never dispatched.
        if w.enabled && w.params_b.is_none() && w.price_per_mtok_usd.is_none() {
            warnings.push(format!(
                "worker \"{}\" has no cost basis (params_b or price_per_mtok_usd): cost estimates will be neutral for it",
                w.name
            ));
        }
    }
    if enabled == 0 {
        return Err("no enabled workers (workers.json needs at least one enabled worker)".into());
    }
    Ok(warnings)
}

/// Depth-first cycle detection over task `deps`. Returns the ids forming a cycle.
pub fn detect_cycle(tasks: &[Task]) -> Result<(), String> {
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    const GRAY: u8 = 1;
    const BLACK: u8 = 2;
    let mut color: HashMap<&str, u8> = HashMap::new();
    let mut stack: Vec<&str> = Vec::new();

    fn visit<'a>(
        id: &'a str,
        by_id: &HashMap<&'a str, &'a Task>,
        color: &mut HashMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
    ) -> Result<(), String> {
        color.insert(id, GRAY);
        stack.push(id);
        if let Some(t) = by_id.get(id) {
            for d in &t.deps {
                match color.get(d.as_str()).copied().unwrap_or(0) {
                    BLACK => {}
                    GRAY => {
                        let pos = stack.iter().position(|x| *x == d.as_str()).unwrap_or(0);
                        let mut cyc: Vec<&str> = stack[pos..].to_vec();
                        cyc.push(d);
                        return Err(format!("dependency cycle: {}", cyc.join(" -> ")));
                    }
                    _ => visit(d, by_id, color, stack)?,
                }
            }
        }
        stack.pop();
        color.insert(id, BLACK);
        Ok(())
    }

    for t in tasks {
        if color.get(t.id.as_str()).copied().unwrap_or(0) != BLACK {
            visit(&t.id, &by_id, &mut color, &mut stack)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Environment / paths
// ---------------------------------------------------------------------------

/// Resolved runtime settings. Defaults mirror taskfleet's `TF_*` variables;
/// each can be overridden via the environment.
#[derive(Debug, Clone)]
pub struct Settings {
    pub repo_dir: PathBuf,
    pub state_dir: PathBuf,
    pub worktree_root: PathBuf,
    pub max_parallel: usize,
    pub branch_prefix: String,
    pub poll_secs: u64,
    pub gate_env: Vec<(String, String)>,
    pub tasks_file: PathBuf,
    pub workers_file: PathBuf,
    pub prompt_file: PathBuf,
    pub agent_timeout_s: u64,
    /// Stall watchdog window for the agent CLI (`TF_AGENT_STALL_S`), in
    /// seconds. **0 = DISABLED** (the default): the agent is bounded only
    /// by `agent_timeout_s`, exactly the legacy behaviour. Any non-zero
    /// value kills an agent that has produced no output (stdout or
    /// stderr) for this many seconds — classified `CmdKind::Stalled` —
    /// so a hung agent stops burning the clock (and paid tokens) instead
    /// of sitting out the whole total timeout.
    pub agent_stall_s: u64,
    /// Global native-harness turn cap (`TF_AGENT_MAX_TURNS`). 0 = no global
    /// opinion; the worker's `max_turns` wins, else 32.
    pub agent_max_turns: u32,
    /// Campaign wall-clock spend ceiling (`TF_MAX_WALL_CLOCK_S`), in
    /// seconds. **0 = unlimited** (the default): exactly the legacy
    /// behaviour. Any non-zero value stops NEW dispatches once the receipts
    /// for the in-scope tasks already total at least this many wall-clock
    /// seconds — attempts already in flight still finish — and the run
    /// exits 3 (stopped early) instead of looping. The receipts are the
    /// ledger, so the ceiling survives restarts and composes across runs.
    pub max_wall_clock_s: u64,
    /// Whitespace-split command prefix wrapped around the agent argv
    /// (`TF_SANDBOX_CMD`, e.g. "firejail --net=none"). Empty = no wrapper.
    pub sandbox_cmd: Vec<String>,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_or_int(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl Settings {
    pub fn from_env() -> Settings {
        let state_dir = PathBuf::from(env_or("TF_STATE_DIR", "state"));
        Settings {
            repo_dir: PathBuf::from(env_or("TF_REPO_DIR", ".")),
            worktree_root: PathBuf::from(env_or(
                "TF_WORKTREE_ROOT",
                &state_dir.join("worktrees").to_string_lossy(),
            )),
            state_dir,
            max_parallel: env_or_int("TF_MAX_PARALLEL", 0) as usize,
            branch_prefix: env_or("TF_BRANCH_PREFIX", "tf"),
            poll_secs: env_or_int("TF_POLL", 15),
            gate_env: parse_gate_env(&env_or("TF_GATE_ENV", "")),
            tasks_file: PathBuf::from(env_or("TF_TASKS_JSON", "config/tasks.json")),
            workers_file: PathBuf::from(env_or("TF_WORKERS_JSON", "config/workers.json")),
            prompt_file: PathBuf::from("prompts/worker.md"),
            agent_timeout_s: env_or_int("TF_AGENT_TIMEOUT_S", 3600),
            agent_stall_s: env_or_int("TF_AGENT_STALL_S", 0),
            agent_max_turns: env_or_int("TF_AGENT_MAX_TURNS", 0) as u32,
            max_wall_clock_s: env_or_int("TF_MAX_WALL_CLOCK_S", 0),
            sandbox_cmd: env_or("TF_SANDBOX_CMD", "")
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        }
    }
}

/// Parse `repos.json`: `{"repos": {"<name>": "<path>"}}`. Missing file →
/// empty map (single-repo mode). Relative paths resolve against the
/// repos.json file's directory.
pub fn load_repos(repos_path: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let json = match std::fs::read_to_string(repos_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("read {}: {e}", repos_path.display())),
    };
    #[derive(Deserialize)]
    struct ReposFile {
        #[serde(default)]
        repos: BTreeMap<String, String>,
    }
    let rf: ReposFile =
        serde_json::from_str(&json).map_err(|e| format!("parse {}: {e}", repos_path.display()))?;
    let base = repos_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(rf
        .repos
        .into_iter()
        .map(|(name, path)| {
            let p = PathBuf::from(&path);
            let joined = if p.is_absolute() { p } else { base.join(p) };
            (name, normalize_path(joined))
        })
        .collect())
}

/// Lexical normalization (`a/b/../c` → `a/c`) so logs and warnings show clean
/// paths; no symlink resolution, no existence check.
fn normalize_path(p: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl Config {
    /// Absolute repo dir for a task (multi-repo, ADR-11):
    /// `""` → default repo; `"main"` → repos["main"] if present, else default;
    /// other names → repos[name], falling back to the default repo (unknown
    /// names are warned about at startup, ADR-4 compat).
    pub fn repo_dir_for(&self, task: &Task, default_repo: &Path) -> PathBuf {
        match task.repo.as_str() {
            "" => default_repo.to_path_buf(),
            name => self
                .repos
                .get(name)
                .cloned()
                .unwrap_or_else(|| default_repo.to_path_buf()),
        }
    }

    /// Warn (not fail) about task repo names that resolve to the fallback.
    /// `"main"` is the canonical default name — it falls back silently
    /// (single-repo configs and the taskfleet corpus use it freely).
    pub fn repo_warnings(&self, default_repo: &Path) -> Vec<String> {
        let mut w = Vec::new();
        for t in &self.tasks {
            if !t.repo.is_empty() && t.repo != "main" && !self.repos.contains_key(&t.repo) {
                w.push(format!(
                    "task '{}': repo '{}' not in repos.json — using default repo {}",
                    t.id,
                    t.repo,
                    default_repo.display()
                ));
            }
        }
        w
    }
}

/// Load a workers file from an in-memory JSON string. Test seam + programmatic
/// use; same validation as the file path.
pub fn load_workers_str(json: &str) -> Result<Config, String> {
    let wf: WorkersFile =
        serde_json::from_str(json).map_err(|e| format!("parse workers: {e}"))?;
    let warnings = validate(&[], &wf.workers)?;
    Ok(Config {
        tasks: Vec::new(),
        workers: wf.workers,
        defaults: wf.defaults,
        by_id: HashMap::new(),
        repos: BTreeMap::new(),
        warnings,
    })
}

/// Parse `TF_GATE_ENV` ("K=V K2=V2 ...") into (key, value) pairs.
pub fn parse_gate_env(s: &str) -> Vec<(String, String)> {
    s.split_whitespace()
        .filter_map(|tok| {
            let (k, v) = tok.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Resolve environment variable NAMES to `(name, value)` pairs.
///
/// The "which of these optional overrides did the operator actually set?"
/// primitive: `env_or`/`env_or_int` answer ONE name with a default baked in,
/// while this answers MANY names at once and reports only the ones that are
/// present, in the order they were asked for — absence is information for
/// the caller, not an error and not a silently injected default. The
/// returned shape is the `(String, String)` pair vec the rest of the module
/// already speaks (`Settings::gate_env`), so a resolved set can be handed to
/// any consumer of that type unchanged.
///
/// Names that are unset (or hold non-UTF-8 bytes, which `std::env::var`
/// reports as an error) are skipped silently; duplicates in `names` yield
/// duplicate pairs, because the caller's order is the contract.
pub fn env_lookup_pairs(names: &[&str]) -> Vec<(String, String)> {
    names
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_string(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wt(path: &Path, s: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(s.as_bytes()).unwrap();
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("af-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn valid_config_loads() {
        let d = tmpdir("cfg-valid");
        wt(
            &d.join("tasks.json"),
            r#"{ "_meta": { "project": "x" }, "tasks": [
                {"id":"A","title":"t1","accept":"true"},
                {"id":"B","title":"t2","deps":["A"],"accept":"true","manual":false}
            ]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"openai","model":"gpt-4o","enabled":true}]}"#,
        );
        let cfg = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap();
        assert_eq!(cfg.tasks.len(), 2);
        assert!(cfg.by_id.contains_key("B"));
    }

    #[test]
    fn duplicate_id_rejected() {
        let d = tmpdir("cfg-dupid");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [
                {"id":"A","title":"x","accept":"true"},
                {"id":"A","title":"y","accept":"true"}
            ]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
        assert!(e.contains("duplicate task id: A"), "{e}");
    }

    #[test]
    fn readonly_with_accept_rejected() {
        let d = tmpdir("cfg-readonly");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [
                {"id":"A","title":"x","readonly":true,"accept":"true"}
            ]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
        assert!(e.contains("readonly task cannot declare"), "{e}");
    }

    #[test]
    fn task_max_turns_zero_rejected() {
        let d = tmpdir("cfg-maxturns");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [
                {"id":"A","title":"x","max_turns":0}
            ]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
        assert!(e.contains("max_turns must be >= 1"), "{e}");
    }

    #[test]
    fn dangling_dep_rejected() {
        let d = tmpdir("cfg-dangling");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [{"id":"A","title":"x","deps":["NOPE"],"accept":"true"}]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let c = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap();
        assert!(
            c.warnings.iter().any(|w| w.contains("NOPE")),
            "dangling dep should warn, not fail: {:?}",
            c.warnings
        );
    }

    #[test]
    fn no_gate_and_not_manual_warns() {
        let d = tmpdir("cfg-nogate");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [{"id":"A","title":"x"}]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let c = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap();
        assert!(
            c.warnings
                .iter()
                .any(|w| w.contains("gate will be skipped")),
            "gate-less task should warn, not fail: {:?}",
            c.warnings
        );
    }

    #[test]
    fn priority_accepts_number_and_string() {
        let d = tmpdir("cfg-prio");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [
                {"id":"A","title":"a","priority":3,"accept":"true"},
                {"id":"B","title":"b","priority":"CRITICAL","accept":"true"},
                {"id":"C","title":"c","accept":"true"}
            ]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let c = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap();
        assert_eq!(c.by_id["A"].priority.rank(), 3);
        assert_eq!(c.by_id["B"].priority.rank(), 20);
        assert_eq!(c.by_id["C"].priority.rank(), 0);
    }

    #[test]
    fn zero_enabled_workers_rejected() {
        let d = tmpdir("cfg-noworkers");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [{"id":"A","title":"x","accept":"true"}]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m","enabled":false}]}"#,
        );
        let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
        assert!(e.contains("no enabled workers"), "{e}");
    }

    #[test]
    fn cycle_detected() {
        let d = tmpdir("cfg-cycle");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [
                {"id":"A","deps":["B"],"accept":"true"},
                {"id":"B","deps":["A"],"accept":"true"}
            ]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
        assert!(e.contains("dependency cycle"), "{e}");
    }

    #[test]
    fn manual_task_without_gate_is_ok() {
        let d = tmpdir("cfg-manual");
        wt(
            &d.join("tasks.json"),
            r#"{ "tasks": [{"id":"A","title":"x","manual":true}]}"#,
        );
        wt(
            &d.join("workers.json"),
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        load(&d.join("tasks.json"), &d.join("workers.json")).unwrap();
    }

    #[test]
    fn parse_gate_env_pairs() {
        let pairs = parse_gate_env("A=1 FOO=bar baz=hello world");
        assert_eq!(
            pairs,
            vec![
                ("A".to_string(), "1".to_string()),
                ("FOO".to_string(), "bar".to_string()),
                ("baz".to_string(), "hello".to_string())
            ]
        );
    }
}

#[cfg(test)]
mod repo_tests {
    use super::*;
    use std::path::PathBuf;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn missing_repos_json_is_single_repo_mode() {
        let d = std::env::temp_dir().join(format!("af-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let map = load_repos(&d.join("config").join("repos.json")).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn relative_repo_paths_resolve_against_the_file() {
        let base = std::env::temp_dir().join(format!("af-repos-rel-{}", std::process::id()));
        // Absolute paths pass through on both platforms.
        let abs = if cfg!(windows) {
            "C:/abs/path"
        } else {
            "/abs/path"
        };
        let body =
            format!(r#"{{"repos": {{"main": "..", "docs": "../docs-site", "abs": "{abs}"}}}}"#);
        let p = write(&base.join("config"), "repos.json", &body);
        let map = load_repos(&p).unwrap();
        assert_eq!(map["main"], base);
        assert_eq!(map["docs"], base.join("docs-site"));
        assert_eq!(map["abs"], PathBuf::from(abs));
    }

    #[test]
    fn repo_dir_for_resolution_matrix() {
        let default = PathBuf::from("/default/repo");
        let mut cfg = Config {
            tasks: vec![],
            workers: vec![],
            defaults: WorkerDefaults::default(),
            by_id: HashMap::new(),
            repos: BTreeMap::new(),
            warnings: vec![],
        };
        let mk = |repo: &str| Task {
            id: "t".into(),
            title: "t".into(),
            repo: repo.into(),
            ..Default::default()
        };
        // No repos.json: "" and "main" → default; unknown → default.
        assert_eq!(cfg.repo_dir_for(&mk(""), &default), default);
        assert_eq!(cfg.repo_dir_for(&mk("main"), &default), default);
        assert_eq!(cfg.repo_dir_for(&mk("docs"), &default), default);
        // With repos.json: named + "main" resolve; "" stays default.
        cfg.repos.insert("main".into(), PathBuf::from("/r/main"));
        cfg.repos.insert("docs".into(), PathBuf::from("/r/docs"));
        assert_eq!(
            cfg.repo_dir_for(&mk("main"), &default),
            PathBuf::from("/r/main")
        );
        assert_eq!(
            cfg.repo_dir_for(&mk("docs"), &default),
            PathBuf::from("/r/docs")
        );
        assert_eq!(cfg.repo_dir_for(&mk(""), &default), default);
    }

    #[test]
    fn repo_warnings_flag_unknown_but_not_main_or_empty() {
        let default = PathBuf::from("/default/repo");
        let mut cfg = Config {
            tasks: vec![
                Task {
                    id: "a".into(),
                    title: "a".into(),
                    repo: "".into(),
                    ..Default::default()
                },
                Task {
                    id: "b".into(),
                    title: "b".into(),
                    repo: "main".into(),
                    ..Default::default()
                },
                Task {
                    id: "c".into(),
                    title: "c".into(),
                    repo: "docs".into(),
                    ..Default::default()
                },
            ],
            workers: vec![],
            defaults: WorkerDefaults::default(),
            by_id: HashMap::new(),
            repos: BTreeMap::new(),
            warnings: vec![],
        };
        cfg.by_id = cfg
            .tasks
            .iter()
            .map(|t| (t.id.clone(), t.clone()))
            .collect();
        let w = cfg.repo_warnings(&default);
        assert_eq!(w.len(), 1, "only 'docs' warns: {w:?}");
        assert!(w[0].contains("task 'c'") && w[0].contains("docs"));
    }
}
