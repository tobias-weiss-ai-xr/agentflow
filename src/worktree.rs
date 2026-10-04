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
        Err(format!(
            "cannot determine current branch in {}",
            repo.display()
        ))
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
    // Prune stale worktree registrations first: a raw-deleted worktree
    // directory still holds its branch "checked out" in git's metadata,
    // which would make the branch -D below (and thus the add) fail.
    let _ = git(repo, &["worktree", "prune"]);
    let _ = git(repo, &["branch", "-D", &branch]);
    let out = git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            path.to_str().unwrap(),
            "HEAD",
        ],
    );
    if !out.passed() {
        return Err(format!(
            "worktree add failed ({:?}): {}",
            out.kind,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn scratch_repo() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("af-wt-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_cmd(&dir, &["init", "-b", "main"]);
        // af's own merge commits need an identity; set it repo-locally so
        // parallel tests don't race on the process env.
        git_cmd(&dir, &["config", "user.name", "af test"]);
        git_cmd(&dir, &["config", "user.email", "af@test"]);
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        git_cmd(&dir, &["add", "."]);
        git_cmd(&dir, &["commit", "-m", "init"]);
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
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// create() with one retry: CI runners occasionally fail a spawn
    /// transiently (fork pressure with parallel test binaries). Production
    /// semantics retry failed attempts too; a deterministic bug fails twice
    /// and still panics here.
    fn create_ok(repo: &Path, wt_root: &Path, id: &str) -> Worktree {
        match create(repo, wt_root, id, "tf") {
            Ok(w) => w,
            Err(e) => {
                eprintln!("create {id} first try failed ({e}); retrying once");
                create(repo, wt_root, id, "tf").expect("create after retry")
            }
        }
    }

    #[test]
    fn create_rejects_non_repo() {
        let dir = std::env::temp_dir().join(format!("af-wt-norepo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = create(&dir, &dir.join("wt"), "T1", "tf").unwrap_err();
        assert!(err.contains("not a git repository"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_merge_roundtrip_writes_into_repo() {
        let repo = scratch_repo();
        let locks = MergeLocks::new();
        let wt = create_ok(&repo, &repo.parent().unwrap().join("wt"), "T1");
        std::fs::write(wt.path.join("f.txt"), "changed\n").unwrap();
        git_cmd(&wt.path, &["add", "."]);
        git_cmd(&wt.path, &["commit", "-m", "task work"]);
        merge(&repo, &wt.branch, &locks, "merge T1").unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(current_branch(&repo).unwrap(), "main");
        remove(&repo, &wt);
        assert!(!wt.path.exists());
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn merge_conflict_aborts_cleanly() {
        let repo = scratch_repo();
        let locks = MergeLocks::new();
        let wt = create_ok(&repo, &repo.parent().unwrap().join("wt"), "T2");
        // Both sides change the same line differently.
        std::fs::write(
            repo.join("f.txt"),
            "main version
",
        )
        .unwrap();
        git_cmd(&repo, &["commit", "-am", "main change"]);
        std::fs::write(
            wt.path.join("f.txt"),
            "branch version
",
        )
        .unwrap();
        git_cmd(&wt.path, &["commit", "-am", "branch change"]);
        let err = merge(&repo, &wt.branch, &locks, "merge T2").unwrap_err();
        assert!(err.contains("merge of tf/T2 failed"));
        // Abort left the working tree clean (no conflicted files staged).
        let out = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).trim().is_empty());
        remove(&repo, &wt);
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn create_replaces_stale_worktree_and_branch() {
        let repo = scratch_repo();
        let wt_root = repo.parent().unwrap().join("wt");
        let stale = wt_root.join("T3");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("junk.txt"), "leftover\n").unwrap();
        let wt = create_ok(&repo, &wt_root, "T3");
        assert!(
            wt.path.join(".git").exists(),
            "stale dir cleared, fresh worktree in place"
        );
        remove(&repo, &wt);
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn heal_removes_stale_worktrees_but_keeps_running_ones() {
        let repo = scratch_repo();
        let wt_root = repo.parent().unwrap().join("wt");
        let repos = vec![("main".to_string(), repo.clone())];
        let w1 = create_ok(&repo, &wt_root, "T4");
        let w2 = create_ok(&repo, &wt_root, "T5");
        heal(&repos, &wt_root, "tf", &["T5".to_string()]);
        assert!(!w1.path.exists(), "stale worktree removed");
        assert!(w2.path.exists(), "running worktree kept");
        let gone = git(
            &repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/tf/T4"],
        );
        assert!(!gone.passed(), "stale branch deleted");
        heal(&repos, &wt_root, "tf", &[] as &[String]);
        assert!(!w2.path.exists());
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }
}
