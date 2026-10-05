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

/// Remove ONLY the worktree directory; the branch and its committed work
/// survive. The gate-failure cleanup: the agent's committed change is the
/// durable `AgentDone` evidence, so only the checkout is dropped — the
/// retry re-attaches to the branch via [`attach`] and never re-pays the
/// agent. Best-effort, never fails the caller.
pub fn remove_worktree_only(repo: &Path, wt: &Worktree) {
    let _ = git(
        repo,
        &["worktree", "remove", "--force", wt.path.to_str().unwrap()],
    );
}

/// Does `branch` exist as a local branch in `repo`?
pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(
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

/// Commits on `branch` not reachable from `base` (`git rev-list --count
/// base..branch`). Err when git cannot answer (e.g. a missing branch).
pub fn commits_ahead(repo: &Path, base: &str, branch: &str) -> Result<u64, String> {
    let out = git(repo, &["rev-list", "--count", &format!("{base}..{branch}")]);
    if !out.passed() {
        return Err(format!(
            "cannot count commits {base}..{branch}: {}",
            out.combined().trim()
        ));
    }
    out.stdout
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("cannot parse rev-list count for {base}..{branch}: {e}"))
}

/// Attach a worktree to an EXISTING branch — the gate-only-retry
/// counterpart of [`create`]: the branch is never deleted or recreated,
/// because the agent's committed work on it is exactly what the retry must
/// preserve (a gate failure must not discard paid agent work). Any leftover
/// dir/registration at `wt_root/<id>` is cleared first so the branch can be
/// checked out fresh from its own tip.
pub fn attach(repo: &Path, wt_root: &Path, id: &str, prefix: &str) -> Result<Worktree, String> {
    if !is_repo(repo) {
        return Err(format!("{} is not a git repository", repo.display()));
    }
    let branch = format!("{prefix}/{id}");
    let path = wt_root.join(id);
    if path.exists() {
        let _ = git(
            repo,
            &["worktree", "remove", "--force", path.to_str().unwrap()],
        );
        let _ = std::fs::remove_dir_all(&path);
    }
    std::fs::create_dir_all(wt_root).map_err(|e| e.to_string())?;
    // Prune stale registrations (same reason as `create`) so the branch is
    // free to check out in the new worktree.
    let _ = git(repo, &["worktree", "prune"]);
    let out = git(repo, &["worktree", "add", path.to_str().unwrap(), &branch]);
    if !out.passed() {
        return Err(format!(
            "worktree attach failed ({:?}): {}",
            out.kind,
            out.combined().trim()
        ));
    }
    Ok(Worktree { path, branch })
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

/// Task ids with a worktree dir under `wt_root`, excluding `keep` (tasks still
/// running). Sorted for stable dry-run output. Missing root → empty.
pub fn orphan_ids(wt_root: &Path, keep: &[String]) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(wt_root) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|id| !keep.iter().any(|k| k == id))
        .collect();
    ids.sort();
    ids
}

/// `af clean [--dry-run]`: remove orphaned worktrees + their branches left by
/// crashed runs, preserving `keep` (tasks still marked Running). With
/// `dry_run`, report the ids that *would* be removed and touch nothing.
/// Returns the affected ids (sorted). Never fails the caller.
pub fn clean(
    repos: &[(String, PathBuf)],
    wt_root: &Path,
    branch_prefix: &str,
    keep: &[String],
    dry_run: bool,
) -> Vec<String> {
    let ids = orphan_ids(wt_root, keep);
    if dry_run || ids.is_empty() {
        return ids;
    }
    // Same path as startup self-heal: unregister worktrees and delete their
    // branches in every repo that owns them.
    heal(repos, wt_root, branch_prefix, keep);
    if keep.is_empty() {
        // Nothing must survive, so wipe the root too — drops dirs no repo
        // owns (e.g. no repos configured, or non-worktree junk).
        clean_all(wt_root);
    } else {
        // Preserve running worktrees; drop only orphans heal couldn't.
        for id in &ids {
            let path = wt_root.join(id);
            if path.exists() {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn scratch_repo() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Repo lives in a UNIQUE per-test directory so `repo.parent()` is
        // private to this test. Several worktree tests derive their
        // `wt_root` from `repo.parent()`; when that was the shared global
        // temp dir they deleted each other's worktrees concurrently (flaky
        // CI: `heal`/`clean` removing another test's `wt` entries).
        let base = std::env::temp_dir().join(format!("af-wt-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("repo");
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

    /// The gate-flake reuse contract: after a gate failure the worktree dir
    /// is dropped but the BRANCH (the agent's committed work) survives, and
    /// `attach` re-materializes a worktree on that exact branch tip.
    #[test]
    fn remove_worktree_only_keeps_the_branch_and_attach_reuses_its_tip() {
        let repo = scratch_repo();
        let wt_root = repo.parent().unwrap().join("wt");
        let wt = create_ok(&repo, &wt_root, "R1");
        std::fs::write(wt.path.join("f.txt"), "agent work\n").unwrap();
        git_cmd(&wt.path, &["add", "."]);
        git_cmd(&wt.path, &["commit", "-m", "agent work"]);
        let tip = git(&repo, &["rev-parse", &wt.branch]);
        let tip = tip.stdout.trim().to_string();

        // Gate-failure cleanup: dir gone, branch (and its commit) kept.
        remove_worktree_only(&repo, &wt);
        assert!(!wt.path.exists(), "worktree dir removed");
        assert!(
            branch_exists(&repo, "tf/R1"),
            "branch survives the gate failure"
        );
        assert_eq!(
            commits_ahead(&repo, "main", "tf/R1").unwrap(),
            1,
            "the committed agent work is beyond the base branch"
        );

        // The retry attaches to the SAME branch, at the same tip.
        let wt2 = attach(&repo, &wt_root, "R1", "tf").expect("attach to existing branch");
        assert_eq!(wt2.branch, "tf/R1");
        assert_eq!(wt2.path, wt.path);
        let head = git(&wt2.path, &["rev-parse", "HEAD"]);
        assert_eq!(
            head.stdout.trim(),
            tip,
            "attached worktree sits on the kept branch tip — no re-created branch"
        );
        assert_eq!(
            std::fs::read_to_string(wt2.path.join("f.txt")).unwrap(),
            "agent work\n",
            "the committed agent work is intact"
        );
        remove(&repo, &wt2);
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    /// `attach` never invents a branch: without one it fails (and leaves no
    /// dir behind) — the caller must fall back to a full agent run.
    #[test]
    fn attach_fails_and_leaves_no_dir_without_the_branch() {
        let repo = scratch_repo();
        let wt_root = repo.parent().unwrap().join("wt");
        assert!(!branch_exists(&repo, "tf/R2"));
        assert!(commits_ahead(&repo, "main", "tf/R2").is_err());
        let err = attach(&repo, &wt_root, "R2", "tf").unwrap_err();
        assert!(err.contains("worktree attach failed"), "{err}");
        assert!(!wt_root.join("R2").exists(), "failed attach leaves no dir");
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }

    /// Durability evidence: a branch carries nothing beyond the base until
    /// it is committed to (and again nothing once the base contains it).
    #[test]
    fn commits_ahead_counts_only_work_beyond_the_base() {
        let repo = scratch_repo();
        let wt_root = repo.parent().unwrap().join("wt");
        let wt = create_ok(&repo, &wt_root, "R3");
        assert_eq!(
            commits_ahead(&repo, "main", &wt.branch).unwrap(),
            0,
            "fresh branch sits at the base tip — not durable"
        );
        std::fs::write(wt.path.join("f.txt"), "work\n").unwrap();
        git_cmd(&wt.path, &["add", "."]);
        git_cmd(&wt.path, &["commit", "-m", "work"]);
        assert_eq!(commits_ahead(&repo, "main", &wt.branch).unwrap(), 1);
        // Once merged, the branch's work is reachable from the base → 0
        // again (a consumed attempt is not durable evidence).
        merge(&repo, &wt.branch, &MergeLocks::new(), "merge R3").unwrap();
        assert_eq!(
            commits_ahead(&repo, "main", &wt.branch).unwrap(),
            0,
            "merged branch is fully reachable from the base"
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

    #[test]
    fn clean_dry_run_reports_without_touching_and_removes_dirs_branches() {
        let repo = scratch_repo();
        // Unique root: scratch repos share a parent, so a fixed "wt" would
        // leak orphans from tests running in parallel.
        let wt_root = repo.parent().unwrap().join(format!(
            "wt-clean-dry-{}",
            repo.file_name().unwrap().to_string_lossy()
        ));
        let repos = vec![("main".to_string(), repo.clone())];
        let w1 = create_ok(&repo, &wt_root, "C1");
        let w2 = create_ok(&repo, &wt_root, "C2");

        // Dry run: lists orphans, deletes nothing.
        let dry = clean(&repos, &wt_root, "tf", &[], true);
        assert_eq!(dry, vec!["C1".to_string(), "C2".to_string()]);
        assert!(
            w1.path.exists() && w2.path.exists(),
            "dry run leaves worktrees in place"
        );

        // Real clean: worktree dirs and branches are gone.
        let gone = clean(&repos, &wt_root, "tf", &[], false);
        assert_eq!(gone, vec!["C1".to_string(), "C2".to_string()]);
        assert!(
            !w1.path.exists() && !w2.path.exists(),
            "orphan dirs removed"
        );
        for b in ["tf/C1", "tf/C2"] {
            let out = git(
                &repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{b}"),
                ],
            );
            assert!(!out.passed(), "branch {b} deleted");
        }
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&wt_root);
    }

    #[test]
    fn clean_keeps_running_worktrees() {
        let repo = scratch_repo();
        let wt_root = repo.parent().unwrap().join(format!(
            "wt-clean-keep-{}",
            repo.file_name().unwrap().to_string_lossy()
        ));
        let repos = vec![("main".to_string(), repo.clone())];
        let w1 = create_ok(&repo, &wt_root, "K1");
        let w2 = create_ok(&repo, &wt_root, "K2");
        let removed = clean(&repos, &wt_root, "tf", &["K2".to_string()], false);
        assert_eq!(removed, vec!["K1".to_string()]);
        assert!(!w1.path.exists(), "orphan K1 removed");
        assert!(w2.path.exists(), "running K2 kept");
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&wt_root);
    }
}
