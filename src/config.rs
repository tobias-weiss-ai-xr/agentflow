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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub deps: Vec<String>,
    pub scope: Vec<String>,
    pub accept: Option<String>,
    pub acceptance_prose: Option<String>,
    pub manual: bool,
    pub priority: Priority,
    pub repo: String,
}

impl Default for Task {
    fn default() -> Self {
        Task {
            id: String::new(),
            title: String::new(),
            deps: Vec::new(),
            scope: Vec::new(),
            accept: None,
            acceptance_prose: None,
            manual: false,
            priority: Priority::default(),
            repo: String::new(),
        }
    }
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerDefaults {
    pub accept_timeout_s: u64,
    pub max_attempts: u32,
    pub retry_delay_s: u64,
    pub agent_timeout_s: u64,
}

impl Default for WorkerDefaults {
    fn default() -> Self {
        WorkerDefaults {
            accept_timeout_s: 600,
            max_attempts: 3,
            retry_delay_s: 30,
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
    let tf: TasksFile =
        serde_json::from_str(&tasks_json).map_err(|e| format!("parse {}: {e}", tasks_path.display()))?;

    let workers_json = std::fs::read_to_string(workers_path)
        .map_err(|e| format!("read {}: {e}", workers_path.display()))?;
    let wf: WorkersFile = serde_json::from_str(&workers_json)
        .map_err(|e| format!("parse {}: {e}", workers_path.display()))?;

    let mut warnings = validate(&tf.tasks, &wf.workers)?;

    let by_id: HashMap<String, Task> = tf
        .tasks
        .iter()
        .map(|t| (t.id.clone(), t.clone()))
        .collect();

    // Dangling deps are non-fatal (compat: configs reference tasks merged in
    // from sibling files). Report them; deadlock detection will surface any
    // dep that truly never resolves.
    for t in &tf.tasks {
        for d in &t.deps {
            if !by_id.contains_key(d) {
                warnings.push(format!("task '{}': dep '{}' not in this file (assumed merged elsewhere)", t.id, d));
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

    for t in tasks {
        // No gate and not manual: legacy corpus runs these gate-less; warn.
        if t.accept.is_none() && !t.manual {
            warnings.push(format!(
                "task '{}': no acceptance gate and not manual — gate will be skipped",
                t.id
            ));
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
            return Err(format!("worker '{}': provider and model are required", w.name));
        }
        if !wnames.insert(w.name.as_str()) {
            return Err(format!("duplicate worker name: {}", w.name));
        }
        if w.enabled {
            enabled += 1;
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
        let repo_dir = PathBuf::from(env_or("TF_REPO_DIR", "."));
        let state_dir = PathBuf::from(env_or("TF_STATE_DIR", "state"));
        let cfg = Settings {
            repo_dir: repo_dir.clone(),
            state_dir: state_dir.clone(),
            worktree_root: PathBuf::from(env_or(
                "TF_WORKTREE_ROOT",
                &state_dir.join("worktrees").to_string_lossy(),
            )),
            max_parallel: env_or_int("TF_MAX_PARALLEL", 0) as usize,
            branch_prefix: env_or("TF_BRANCH_PREFIX", "tf"),
            poll_secs: env_or_int("TF_POLL", 15),
            gate_env: parse_gate_env(&env_or("TF_GATE_ENV", "")),
            tasks_file: PathBuf::from(env_or("TF_TASKS_JSON", "config/tasks.json")),
            workers_file: PathBuf::from(env_or("TF_WORKERS_JSON", "config/workers.json")),
            prompt_file: PathBuf::from("prompts/worker.md"),
            agent_timeout_s: env_or_int("TF_AGENT_TIMEOUT_S", 3600),
            sandbox_cmd: env_or("TF_SANDBOX_CMD", "")
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        };
        let _ = repo_dir;
        cfg
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
    Ok(rf.repos
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
            if !t.repo.is_empty()
                && t.repo != "main"
                && !self.repos.contains_key(&t.repo)
            {
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

/// Parse `TF_GATE_ENV` ("K=V K2=V2 ...") into (key, value) pairs.
pub fn parse_gate_env(s: &str) -> Vec<(String, String)> {
    s.split_whitespace()
        .filter_map(|tok| {
            let (k, v) = tok.split_once('=')?;
            Some((k.to_string(), v.to_string()))
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
            c.warnings.iter().any(|w| w.contains("gate will be skipped")),
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
        let p = write(
            &base.join("config"),
            "repos.json",
            r#"{"repos": {"main": "..", "docs": "../docs-site", "abs": "C:/abs/path"}}"#,
        );
        let map = load_repos(&p).unwrap();
        assert_eq!(map["main"], base);
        assert_eq!(map["docs"], base.join("docs-site"));
        assert_eq!(map["abs"], PathBuf::from("C:/abs/path"));
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
        assert_eq!(cfg.repo_dir_for(&mk("main"), &default), PathBuf::from("/r/main"));
        assert_eq!(cfg.repo_dir_for(&mk("docs"), &default), PathBuf::from("/r/docs"));
        assert_eq!(cfg.repo_dir_for(&mk(""), &default), default);
    }

    #[test]
    fn repo_warnings_flag_unknown_but_not_main_or_empty() {
        let default = PathBuf::from("/default/repo");
        let mut cfg = Config {
            tasks: vec![
                Task { id: "a".into(), title: "a".into(), repo: "".into(), ..Default::default() },
                Task { id: "b".into(), title: "b".into(), repo: "main".into(), ..Default::default() },
                Task { id: "c".into(), title: "c".into(), repo: "docs".into(), ..Default::default() },
            ],
            workers: vec![],
            defaults: WorkerDefaults::default(),
            by_id: HashMap::new(),
            repos: BTreeMap::new(),
            warnings: vec![],
        };
        cfg.by_id = cfg.tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();
        let w = cfg.repo_warnings(&default);
        assert_eq!(w.len(), 1, "only 'docs' warns: {w:?}");
        assert!(w[0].contains("task 'c'") && w[0].contains("docs"));
    }
}
