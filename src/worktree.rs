//! Worktree: git worktree create/remove/branch-delete and serialized merges
//! via the git CLI (spec: worktree).

use crate::subprocess::{self, CmdOut};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One in-process lock guards ALL merges (merge serialization).
/// ponytail: global lock, not per-repo — correct for any repo count; keyed
/// per-repo locks if cross-repo merge throughput ever matters.
#[derive(Debug, Clone, Default)]
pub struct MergeLocks(Arc<Mutex<()>>);

impl MergeLocks {
    pub fn new() -> Self {
        MergeLocks(Arc::new(Mutex::new(())))
    }
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

fn git(repo: &Path, args: &[&str]) -> CmdOut {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    subprocess::run(
        "git",
        &args,
        Some(repo),
        &[],
        subprocess::EnvMode::Inherit, // af's own git ops are trusted
        Duration::from_secs(300),
    )
}

pub fn is_repo(repo: &Path) -> bool {
    git(repo, &["rev-parse", "--git-dir"]).passed()
}

/// Current branch of the repo's main checkout (used as merge base).
pub fn current_branch(repo: &Path) -> Result<String, String> {
    let out = git(repo, &["symbolic-ref", "--short", "HEAD"]);
    if out.passed() {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(format!("cannot determine current branch in {}", repo.display()))
    }
}

pub fn create(repo: &Path, wt_root: &Path, id: &str, prefix: &str) -> Result<Worktree, String> {
    if !is_repo(repo) {
        return Err(format!("{} is not a git repository", repo.display()));
    }
    std::fs::create_dir_all(wt_root).map_err(|e| e.to_string())?;
    let branch = format!("{prefix}/{id}");
    let path = wt_root.join(id);
    if path.exists() {
        // Stale worktree from a dead run: clear it before adding.
        let _ = std::fs::remove_dir_all(&path);
    }
    // Drop a stale branch of the same name if it exists.
    let _ = git(repo, &["branch", "-D", &branch]);
    let out = git(
        repo,
        &["worktree", "add", "-b", &branch, path.to_str().unwrap(), "HEAD"],
    );
    if !out.passed() {
        return Err(format!(
            "worktree add failed: {}",
            out.combined().trim()
        ));
    }
    Ok(Worktree { path, branch })
}

/// Best-effort removal: worktree + branch. Never fails the caller.
pub fn remove(repo: &Path, wt: &Worktree) {
    let _ = git(
        repo,
        &["worktree", "remove", "--force", wt.path.to_str().unwrap()],
    );
    let _ = git(repo, &["branch", "-D", &wt.branch]);
}

/// Serialized merge of `branch` into the repo's current checkout branch.
/// On conflict: aborts the merge and returns an error (never force-pushes).
pub fn merge(repo: &Path, branch: &str, locks: &MergeLocks, msg: &str) -> Result<(), String> {
    let _guard = locks.0.lock().unwrap_or_else(|p| p.into_inner());
    let out = git(repo, &["merge", "--no-ff", branch, "-m", msg]);
    if out.passed() {
        return Ok(());
    }
    let detail = out.combined().trim().to_string();
    let _ = git(repo, &["merge", "--abort"]);
    Err(format!("merge of {branch} failed: {detail}"))
}

/// Startup self-heal: remove worktrees whose task is not currently running
/// (dead attempts / prior crashes). Tries every repo (multi-repo, ADR-11):
/// `git worktree remove` fails harmlessly in repos that don't own the dir.
pub fn heal(
    repos: &[(String, PathBuf)],
    wt_root: &Path,
    branch_prefix: &str,
    running_ids: &[String],
) {
    let Ok(rd) = std::fs::read_dir(wt_root) else {
        return;
    };
    for e in rd.flatten() {
        let id = e.file_name().to_string_lossy().to_string();
        if running_ids.iter().any(|r| r == &id) {
            continue;
        }
        for (_, repo) in repos {
            let _ = git(
                repo,
                &["worktree", "remove", "--force", e.path().to_str().unwrap()],
            );
            let _ = git(repo, &["branch", "-D", &format!("{branch_prefix}/{id}")]);
        }
    }
}

/// Best-effort removal of every worktree dir under `wt_root` at startup.
pub fn clean_all(wt_root: &Path) {
    let _ = std::fs::remove_dir_all(wt_root);
}
