//! Run: the dispatch loop (poll → reap → dispatch) and query commands used
//! by the CLI (spec: cli, scheduling, state).

use crate::config::{Config, Settings, TaskState};
use crate::execute::{self, ExecCtx, Outcome};
use crate::scheduler;
use crate::state::{Store, TaskStatus};
use crate::worktree;
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub once: bool,
    pub dry_run: bool,
    pub worker_filter: Option<String>,
    pub task_filter: Option<String>,
    pub poll_secs: Option<u64>,
}

enum Msg {
    Done(String, String, Outcome), // (task id, worker name, outcome)
}

fn pick_worker(
    cfg: &Config,
    busy: &Mutex<HashMap<String, bool>>,
    filter: Option<&str>,
) -> Option<crate::config::Worker> {
    let busy = busy.lock().unwrap();
    for w in &cfg.workers {
        if !w.enabled {
            continue;
        }
        if let Some(f) = filter {
            if &w.name != f {
                continue;
            }
        }
        if !busy.get(&w.name).copied().unwrap_or(false) {
            return Some(w.clone());
        }
    }
    None
}

/// `af run`: execute tasks until all done or deadlock. Returns process exit code.
pub fn run_loop(cfg: &Config, st: &Settings, opts: &RunOptions) -> i32 {
    if opts.dry_run {
        return dry_run(cfg, st);
    }

    let store = Store::new(st.state_dir.clone());
    let poll = opts.poll_secs.unwrap_or(st.poll_secs);
    let max_attempts = cfg.defaults.max_attempts;

    // --- self-heal: remove orphan worktrees from dead attempts ---
    let mut status = store.load();
    let stale_running: Vec<String> = status
        .iter()
        .filter(|(_, s)| s.state == TaskState::Running)
        .map(|(k, _)| k.clone())
        .collect();
    worktree::heal(&st.repo_dir, &st.worktree_root, &st.branch_prefix, &stale_running);
    for id in &stale_running {
        if let Some(s) = status.get_mut(id) {
            s.state = TaskState::Ready; // previous owner died mid-run
            s.last_error = Some("previous run interrupted".into());
        }
    }
    let _ = store.save(&status);

    let log_dir = store.log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let _ = std::fs::create_dir_all(&store.prompt_dir());

    let (tx, rx) = mpsc::channel::<Msg>();
    let merge_locks = worktree::MergeLocks::new();
    let worker_busy: Arc<Mutex<HashMap<String, bool>>> = Arc::new(Mutex::new(HashMap::new()));
    let running: Arc<Mutex<HashMap<String, ()>>> = Arc::new(Mutex::new(HashMap::new()));

    let enabled = cfg.workers.iter().filter(|w| w.enabled).count();
    let max_parallel = if st.max_parallel == 0 { enabled } else { st.max_parallel }.max(1);

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

    // --- dispatch loop ---
    loop {
        // Reap finished tasks.
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Done(id, worker_name, outcome) => {
                    running.lock().unwrap().remove(&id);
                    worker_busy.lock().unwrap().insert(worker_name, false);
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
                    let _ = store.save(&status);
                }
            }
        }

        // Terminal conditions.
        let all_done = cfg
            .tasks
            .iter()
            .all(|t| matches!(status.get(&t.id).map(|s| &s.state), Some(TaskState::Done)));
        if all_done {
            println!("\nAll tasks done.");
            println!("{}", board_of(cfg, &status));
            return 0;
        }
        let running_ids: Vec<String> = running.lock().unwrap().keys().cloned().collect();
        if running_ids.is_empty() {
            if let Some(blocked) = scheduler::find_deadlock(cfg, &status, &[]) {
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
            let mut dispatched = false;
            for t in ready {
                if let Some(f) = &opts.task_filter {
                    if &t.id != f {
                        continue;
                    }
                }
                if running.lock().unwrap().len() >= max_parallel {
                    break;
                }
                let worker = match pick_worker(cfg, &worker_busy, opts.worker_filter.as_deref()) {
                    Some(w) => w,
                    None => break, // no free worker this round
                };
                // Mark Running + persist BEFORE spawning (crash-safety).
                let attempt = {
                    let s = status.entry(t.id.clone()).or_default();
                    s.state = TaskState::Running;
                    s.attempts + 1
                };
                let _ = store.save(&status);

                worker_busy.lock().unwrap().insert(worker.name.clone(), true);
                running.lock().unwrap().insert(t.id.clone(), ());
                let log_path = log_dir.join(format!("{}.log", t.id));
                let tx2 = tx.clone();
                let ctx2 = ctx.clone();
                let id = t.id.clone();
                let wname = worker.name.clone();
                let wclone = worker.clone();
                let model = worker.model.clone();
                println!("  → {id} dispatch on {} ({model}) [attempt {attempt}]", worker.name);
                std::thread::spawn(move || {
                    let out = execute::execute_task(&ctx2, &wclone, &id, attempt, &log_path);
                    let _ = tx2.send(Msg::Done(id, wname, out));
                });
                dispatched = true;
            }
            if opts.once {
                // --once: dispatch no more after this round; wait for in-flight.
                let _ = dispatched;
                while !running.lock().unwrap().is_empty() {
                    while let Ok(m) = rx.try_recv() {
                        let Msg::Done(id, wname, outcome) = m;
                        running.lock().unwrap().remove(&id);
                        worker_busy.lock().unwrap().insert(wname, false);
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
                                s.state = if s.attempts >= max_attempts {
                                    TaskState::Failed
                                } else {
                                    TaskState::Ready
                                };
                                println!("  ✗ {id} finished: {err}");
                            }
                        }
                        let _ = store.save(&status);
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                println!("{}", board_of(cfg, &status));
                return 0;
            }
        }

        std::thread::sleep(Duration::from_secs(poll.max(1)));
    }
}

pub fn dry_run(cfg: &Config, st: &Settings) -> i32 {
    let depths = scheduler::compute_depths(cfg).unwrap_or_default();
    let status = Store::new(st.state_dir.clone()).load();
    let ready = scheduler::ready_tasks(cfg, &status, &[], cfg.defaults.max_attempts);
    println!(
        "Dry run — dispatch plan ({} tasks, {} workers):",
        cfg.tasks.len(),
        cfg.workers.len()
    );
    for t in &ready {
        let depth = depths.get(&t.id).copied().unwrap_or(0);
        let worker = cfg
            .workers
            .iter()
            .find(|w| w.enabled)
            .map(|w| w.name.as_str())
            .unwrap_or("-");
        println!(
            "  {:<12} depth={}  worker={}  scope={}",
            t.id,
            depth,
            worker,
            if t.scope.is_empty() {
                "*".to_string()
            } else {
                t.scope.join(",")
            }
        );
    }
    println!("  ({} tasks ready now; nothing was created or changed)", ready.len());
    0
}

fn board_of(cfg: &Config, status: &HashMap<String, TaskStatus>) -> String {
    let mut rows = vec![format!("{:<12} {:<9} {:<8} {}", "TASK", "STATE", "ATTEMPTS", "LAST ERROR")];
    for t in &cfg.tasks {
        let s = status.get(&t.id).cloned().unwrap_or_default();
        let err = s
            .last_error
            .as_deref()
            .map(|e| if e.len() > 48 { format!("{}…", &e[..48]) } else { e.to_string() })
            .unwrap_or_default();
        rows.push(format!("{:<12} {:<9?} {:<8} {}", t.id, s.state, s.attempts, err));
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
    let mut lines = format!("{:<12} {:<9} {:<10} {}", "TASK", "ATTEMPTS", "WALL_S", "MODEL");
    let mut total: f64 = 0.0;
    for t in &cfg.tasks {
        if let Some(f) = task_filter {
            if &t.id != f {
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
    lines
}

/// `af attach <id>`: tail a task's log until its terminal state.
pub fn attach(cfg: &Config, st: &Settings, id: &str) -> i32 {
    let _ = cfg;
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
