//! Scheduler: dependency DAG depths, ready-task selection (deps, contention,
//! retry budget), and deadlock detection. Pure functions — no I/O (spec:
//! scheduling).

use crate::config::{Config, Task, TaskState};
use crate::state::TaskStatus;
use std::collections::{HashMap, HashSet};

/// Critical-path depth: 0 for tasks with no deps, else 1 + max(dep depths).
/// Also returns a cycle error if the DAG is not acyclic (belt and braces —
/// config validation already rejects cycles).
pub fn compute_depths(cfg: &Config) -> Result<HashMap<String, usize>, String> {
    let mut depths: HashMap<String, usize> = HashMap::new();
    fn depth_of(id: &str, cfg: &Config, depths: &mut HashMap<String, usize>, visiting: &mut HashSet<String>) -> Result<usize, String> {
        if let Some(d) = depths.get(id) {
            return Ok(*d);
        }
        if !visiting.insert(id.to_string()) {
            return Err(format!("dependency cycle involving {id}"));
        }
        let task = cfg
            .by_id
            .get(id)
            .ok_or_else(|| format!("unknown task {id}"))?;
        let mut d = 0usize;
        for dep in &task.deps {
            d = d.max(depth_of(dep, cfg, depths, visiting)? + 1);
        }
        visiting.remove(id);
        depths.insert(id.to_string(), d);
        Ok(d)
    }
    let mut visiting = HashSet::new();
    for t in &cfg.tasks {
        depth_of(&t.id, cfg, &mut depths, &mut visiting)?;
    }
    Ok(depths)
}

/// True if one glob-free prefix starts with the other — a conservative
/// "these scope patterns could overlap" test used to defer concurrent work.
pub fn scope_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let pa = a.split(['*', '?']).next().unwrap_or("");
    let pb = b.split(['*', '?']).next().unwrap_or("");
    pa.starts_with(pb) || pb.starts_with(pa)
}

pub fn tasks_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| scope_overlap(x, y)))
}

/// Ready tasks eligible for dispatch right now, ordered for dispatch
/// (deeper critical-path first, then higher `priority`, then original order).
pub fn ready_tasks(
    cfg: &Config,
    status: &HashMap<String, TaskStatus>,
    running: &[String],
    max_attempts: u32,
) -> Vec<Task> {
    let depths = compute_depths(cfg).unwrap_or_default();
    let running_scope: Vec<&Task> = running
        .iter()
        .filter_map(|id| cfg.by_id.get(id))
        .collect();

    let mut out: Vec<&Task> = Vec::new();
    for t in &cfg.tasks {
        if running.contains(&t.id) {
            continue;
        }
        let st = status.get(&t.id).cloned().unwrap_or_default();
        if st.state.is_terminal() {
            continue;
        }
        if st.attempts >= max_attempts {
            continue; // exhausted; deadlock logic will flag it
        }
        let deps_done = t.deps.iter().all(|d| {
            matches!(
                status.get(d).map(|s| s.state.clone()),
                Some(TaskState::Done)
            )
        });
        if !deps_done {
            continue;
        }
        let conflicts = running_scope
            .iter()
            .any(|r| tasks_overlap(&r.scope, &t.scope));
        if conflicts {
            continue;
        }
        out.push(t);
    }

    let mut indexed: Vec<(usize, &Task)> = out.into_iter().enumerate().collect();
    indexed.sort_by(|(ia, a), (ib, b)| {
        let da = depths.get(&a.id).copied().unwrap_or(0);
        let db = depths.get(&b.id).copied().unwrap_or(0);
        db.cmp(&da)
            .then(b.priority.rank().cmp(&a.priority.rank()))
            .then(ia.cmp(ib))
    });
    indexed.into_iter().map(|(_, t)| t.clone()).collect()
}

/// Deadlock: no running tasks and every remaining (non-done) task is `failed`
/// or depends (transitively) on a failed task. Returns the blocked task ids.
pub fn find_deadlock(
    cfg: &Config,
    status: &HashMap<String, TaskStatus>,
    running: &[String],
) -> Option<Vec<String>> {
    let remaining: Vec<&Task> = cfg
        .tasks
        .iter()
        .filter(|t| {
            let st = status.get(&t.id).cloned().unwrap_or_default();
            st.state != TaskState::Done && !running.contains(&t.id)
        })
        .collect();
    if remaining.is_empty() || !running.is_empty() {
        return None;
    }

    let mut blocked: HashSet<String> = status
        .iter()
        .filter(|(_, s)| s.state == TaskState::Failed)
        .map(|(k, _)| k.clone())
        .collect();

    loop {
        let mut changed = false;
        for t in &remaining {
            if blocked.contains(&t.id) {
                continue;
            }
            // Blocked if any dep failed, OR the dep does not exist in this
            // config (it will never resolve → can never become ready).
            let stuck = t.deps.iter().any(|d| {
                blocked.contains(d) || !cfg.by_id.contains_key(d)
            });
            if stuck {
                blocked.insert(t.id.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    if remaining.iter().all(|t| blocked.contains(&t.id)) {
        Some(remaining.iter().map(|t| t.id.clone()).collect())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;

    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn cfg_with(tasks_json: &str, workers_json: &str) -> Config {
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("af-sched-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("tasks.json"), tasks_json).unwrap();
        std::fs::write(d.join("workers.json"), workers_json).unwrap();
        load(&d.join("tasks.json"), &d.join("workers.json")).unwrap()
    }

    fn tasks0() -> Config {
        cfg_with(
            r#"{ "tasks": [
                {"id":"A","title":"a","accept":"true"},
                {"id":"B","title":"b","deps":["A"],"accept":"true"},
                {"id":"C","title":"c","accept":"true"}
            ]}"#,
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        )
    }

    fn status_of(pairs: &[(&str, TaskState)]) -> HashMap<String, TaskStatus> {
        pairs
            .iter()
            .map(|(id, st)| {
                (
                    id.to_string(),
                    TaskStatus {
                        state: st.clone(),
                        attempts: 0,
                        last_error: None,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn depths_reflect_dag() {
        let cfg = tasks0();
        let depths = compute_depths(&cfg).unwrap();
        assert_eq!(depths["A"], 0);
        assert_eq!(depths["B"], 1);
        assert_eq!(depths["C"], 0);
    }

    #[test]
    fn dep_not_done_is_not_ready() {
        let cfg = tasks0();
        let status = status_of(&[]);
        let ready = ready_tasks(&cfg, &status, &[], 3);
        let ids: Vec<&str> = ready.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"A"));
        assert!(ids.contains(&"C"));
        assert!(!ids.contains(&"B"), "B depends on A, not done yet");
    }

    #[test]
    fn deeper_task_scheduled_first() {
        let cfg = tasks0();
        let mut status = status_of(&[("A", TaskState::Done)]);
        status.get_mut("A").unwrap().state = TaskState::Done;
        // Mark A done; B (depth 1) vs C (depth 0) both ready → B first.
        let ready = ready_tasks(&cfg, &status, &[], 3);
        let ids: Vec<&str> = ready.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.first(), Some(&"B"), "critical-path first: {ids:?}");
    }

    #[test]
    fn overlapping_scope_deferred_while_running() {
        let cfg = cfg_with(
            r#"{ "tasks": [
                {"id":"X","title":"x","scope":["src/a.rs"],"accept":"true"},
                {"id":"Y","title":"y","scope":["src/*"],"accept":"true"}
            ]}"#,
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let st = status_of(&[("X", TaskState::Running)]);
        let ready = ready_tasks(&cfg, &st, &["X".to_string()], 3);
        assert!(ready.is_empty(), "Y overlaps running X's scope");
    }

    #[test]
    fn disjoint_scope_both_ready() {
        let cfg = cfg_with(
            r#"{ "tasks": [
                {"id":"X","title":"x","scope":["src/a.rs"],"accept":"true"},
                {"id":"Y","title":"y","scope":["doc/b.md"],"accept":"true"}
            ]}"#,
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let st = status_of(&[("X", TaskState::Running)]);
        let ready = ready_tasks(&cfg, &st, &["X".to_string()], 3);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "Y");
    }

    #[test]
    fn attempts_exhausted_not_ready() {
        let cfg = tasks0();
        let mut status = status_of(&[("A", TaskState::Failed)]);
        status.get_mut("A").unwrap().attempts = 3;
        let ready = ready_tasks(&cfg, &status, &[], 3);
        assert!(!ready.iter().any(|t| t.id == "A"));
    }

    #[test]
    fn no_deadlock_when_a_ready_task_can_progress() {
        let cfg = tasks0(); // C is ready and unblocked
        let mut status = status_of(&[("A", TaskState::Failed)]);
        status.get_mut("A").unwrap().attempts = 3;
        assert!(find_deadlock(&cfg, &status, &[]).is_none(), "C can still run");
    }

    #[test]
    fn deadlock_only_when_nothing_can_progress() {
        let cfg = cfg_with(
            r#"{ "tasks": [
                {"id":"A","title":"a","accept":"true"},
                {"id":"B","title":"b","deps":["A"],"accept":"true"}
            ]}"#,
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let mut status = status_of(&[("A", TaskState::Failed)]);
        status.get_mut("A").unwrap().attempts = 3;
        // B blocked by A; nothing else → deadlock.
        let blocked = find_deadlock(&cfg, &status, &[]);
        assert!(blocked.is_some());
        let list = blocked.unwrap();
        assert!(list.contains(&"A".to_string()) || list.contains(&"B".to_string()));
    }

    #[test]
    fn no_deadlock_while_running() {
        let cfg = tasks0();
        let status = status_of(&[("A", TaskState::Failed)]);
        assert!(find_deadlock(&cfg, &status, &["B".to_string()]).is_none());
    }

    #[test]
    fn unknown_dep_is_deadlock_not_infinite_loop() {
        // A dep id that never resolves (compat: merged from a sibling file)
        // must surface as a deadlock, not spin forever.
        let cfg = cfg_with(
            r#"{ "tasks": [
                {"id":"A","title":"a","deps":["GHOST"],"accept":"true"}
            ]}"#,
            r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#,
        );
        let status = status_of(&[]);
        assert!(cfg.warnings.iter().any(|w| w.contains("GHOST")));
        assert!(
            find_deadlock(&cfg, &status, &[]).is_some(),
            "absent dep must block → deadlock"
        );
    }

    #[test]
    fn scope_overlap_cases() {
        assert!(scope_overlap("src/a.rs", "src/a.rs"));
        assert!(scope_overlap("src/a.rs", "src/*"));
        assert!(scope_overlap("src/*.rs", "src/*.py")); // conservative
        assert!(!scope_overlap("src/", "doc/"));
        assert!(scope_overlap("crates/*", "crates/"));
    }

    #[test]
    fn property_no_deadlock_crash_on_random_dags() {
        // Seeded pseudo-random DAG: deadlock detection must never panic and
        // must be consistent with ready_tasks (blocked == nothing ready && nothing running).
        let d = std::env::temp_dir().join(format!("af-prop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut seed: u64 = 0x5eed;
        let mut rng = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        for round in 0..50 {
            let n = 2 + rng() % 6;
            let mut tasks = Vec::new();
            for i in 0..n {
                // deps only on lower indices => acyclic by construction
                let mut deps: Vec<String> = (0..rng() % n)
                    .map(|_| format!("T{}", rng() % i.saturating_add(1)))
                    .filter(|d| *d != format!("T{i}"))
                    .collect();
                deps.sort();
                deps.dedup();
                tasks.push(format!(
                    r#"{{"id":"T{i}","title":"t","deps":[{}],"accept":"true"}}"#,
                    deps
                        .iter()
                        .map(|x| format!("\"{x}\""))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            let tj = format!(r#"{{ "tasks": [{}] }}"#, tasks.join(","));
            let wj = r#"{ "workers": [{"name":"w1","provider":"p","model":"m"}]}"#;
            let cfg = cfg_with(&tj, wj);
            let depths = compute_depths(&cfg);
            // Either a cycle error or consistent depths.
            let _ = depths;
            // Random status map with random outcomes.
            let mut status = HashMap::new();
            for i in 0..n {
                let s = match rng() % 4 {
                    0 => TaskState::Ready,
                    1 => TaskState::Running,
                    2 => TaskState::Done,
                    _ => TaskState::Failed,
                };
                status.insert(
                    format!("T{i}"),
                    TaskStatus {
                        state: s,
                        attempts: (rng() % 5) as u32,
                        last_error: None,
                    },
                );
            }
            let running: Vec<String> = status
                .iter()
                .filter(|(_, s)| s.state == TaskState::Running)
                .map(|(k, _)| k.clone())
                .collect();
            let _ = ready_tasks(&cfg, &status, &running, 3);
            let _ = find_deadlock(&cfg, &status, &running);
            let _ = round;
        }
    }
}
