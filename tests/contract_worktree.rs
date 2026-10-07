//! Contract tests for `agentflow::worktree` — the git safety properties the
//! orchestrator depends on.
//!
//! `src/worktree.rs` is where agentflow's git isolation lives: a task gets a
//! branch derived from its id, worktrees are mutually disjoint, merges into
//! the base branch are serialized by one in-process lock, and a conflicting
//! merge aborts instead of force-pushing or leaving the repo mid-merge. Those
//! are asserted only indirectly today (through `execute`/`run` end-to-end
//! tests), so a refactor that keeps the campaign green while changing what
//! callers may rely on would slip through.
//!
//! This file pins that PUBLIC contract from the outside, using ONLY the
//! exported surface (`is_repo`, `current_branch`, `create`, `remove`,
//! `merge`, `archive_branch`, `MergeLocks`, `heal`, `clean_all`,
//! `orphan_ids`, `clean`, `Worktree`) declared by `pub mod worktree` in
//! `src/lib.rs`. Everything asserted here was read from the implementation
//! first; where this file pins a choice the implementation could have made
//! either way, it is called out at the assertion (e.g. a repeated merge is
//! pinned as an `Ok` no-op).
//!
//! Hermeticity: every test builds its OWN scratch repo with `git init -b
//! main`, sets identity repo-locally, commits an initial file, and nests
//! both the repo and `wt_root` under a uniquely-named per-test base
//! directory. Deriving worktree paths from a SHARED parent was the cause of a
//! past flake (parallel tests deleting each other's worktrees), so no path
//! here is shared between tests. Uses only std + the crate; no new deps.

use agentflow::worktree::{
    archive_branch, archived_task_id, branch_exists, clean, clean_all, clean_rejected, create,
    current_branch, heal, is_repo, merge, orphan_ids, remove, MergeLocks, Worktree,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A scratch repo plus the sibling worktree root it owns — the entire
/// filesystem footprint of one test, all under a unique base directory.
/// `Drop` wipes it, so a failing assertion never leaks a repo into the next
/// run.
struct Scratch {
    base: PathBuf,
    repo: PathBuf,
    wt_root: PathBuf,
}

impl Scratch {
    /// Fresh repo at `<base>/repo` with a `base\n` initial commit on `main`;
    /// worktrees go under `<base>/wt`. `label` only aids failure output — the
    /// pid + atomic counter keeps paths unique across tests AND runs.
    fn new(label: &str) -> Scratch {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "af-contract-worktree-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).expect("create scratch repo dir");
        // Repo-local identity: af's own merge commits need it, and keeping it
        // out of the process env means parallel tests cannot race on it.
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.name", "af contract test"]);
        git(&repo, &["config", "user.email", "af@contract.test"]);
        std::fs::write(repo.join("f.txt"), "base\n").expect("seed initial file");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
        Scratch {
            wt_root: base.join("wt"),
            base,
            repo,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Run `git args` in `dir`, asserting success, and return stdout. Identity is
/// passed via `-c` as well as repo-locally so commits issued through a linked
/// worktree always succeed regardless of git's config-sharing quirks.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=af contract test",
            "-c",
            "user.email=af@contract.test",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git spawns");
    assert!(
        out.status.success(),
        "git {:?} in {} failed: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// True when `git args` exits zero — for the negative checks (branch/remote
/// absence, non-repo detection) where success is the thing under test.
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `create` with one retry for CI spawn transients (same rationale as the
/// module's unit tests): a deterministic failure still fails, a fork blip
/// does not flake the contract.
fn create_ok(repo: &Path, wt_root: &Path, id: &str, prefix: &str) -> Worktree {
    match create(repo, wt_root, id, prefix) {
        Ok(wt) => wt,
        Err(e) => {
            eprintln!("create {id} first try failed ({e}); retrying once");
            create(repo, wt_root, id, prefix).expect("create after retry")
        }
    }
}

/// Compile-time proof that a `MergeLocks` value may be shared and sent across
/// threads — the precondition the orchestrator's own threads rely on.
fn assert_send_sync<T: Send + Sync>() {}

/// The headline contract: `create` derives the branch from the id + prefix
/// and checks it out in a fresh directory WITHOUT disturbing the base repo;
/// disjoint worktrees coexist; `merge` integrates a branch and reports a
/// commit; a repeated merge of the same branch is pinned as an `Ok` no-op
/// (never an error); `remove` takes the worktree and branch away.
// spec: worktree/worktree-lifecycle
// spec: worktree/worktree-lifecycle#create-and-remove
// spec: worktree/merge-serialization
#[test]
fn worktree_lifecycle_and_merge_contract() {
    let s = Scratch::new("lifecycle");
    let locks = MergeLocks::new();
    let base_head = git(&s.repo, &["rev-parse", "HEAD"]);

    // create: branch = "<prefix>/<id>", checked out in wt_root/<id>, and the
    // base checkout stays on its original branch with an untouched tree.
    let wt1 = create_ok(&s.repo, &s.wt_root, "T1", "tf");
    assert_eq!(wt1.branch, "tf/T1", "branch derived from prefix + id");
    assert_eq!(wt1.path, s.wt_root.join("T1"), "path is wt_root/<id>");
    assert!(
        wt1.path.join(".git").exists(),
        "worktree is a real checkout"
    );
    assert_eq!(
        current_branch(&wt1.path).expect("worktree HEAD is a branch"),
        "tf/T1",
        "the new directory has the derived branch checked out"
    );
    assert_eq!(
        current_branch(&s.repo).unwrap(),
        "main",
        "base repo stays on its original branch"
    );
    assert_eq!(
        std::fs::read_to_string(s.repo.join("f.txt")).unwrap(),
        "base\n",
        "base working tree untouched by create"
    );
    assert_eq!(
        git(&s.repo, &["rev-parse", "HEAD"]),
        base_head,
        "create moves no ref in the base repo"
    );

    // Disjoint worktrees for two ids exist at once, on distinct branches.
    let wt2 = create_ok(&s.repo, &s.wt_root, "T2", "tf");
    assert!(wt1.path.exists() && wt2.path.exists(), "both coexist");
    assert_ne!(wt1.branch, wt2.branch, "each id gets its own branch");
    assert_eq!(current_branch(&wt2.path).unwrap(), "tf/T2");

    // merge integrates the branch into the base branch and reports a commit.
    std::fs::write(wt1.path.join("f.txt"), "from T1\n").unwrap();
    git(&wt1.path, &["commit", "-am", "T1 work"]);
    assert_eq!(
        git(&s.repo, &["rev-parse", "HEAD"]),
        base_head,
        "the commit landed on the worktree branch, not main"
    );
    merge(&s.repo, &wt1.branch, &locks, "merge T1").expect("disjoint branch merges");
    assert_eq!(
        std::fs::read_to_string(s.repo.join("f.txt")).unwrap(),
        "from T1\n",
        "merged content is on the base branch"
    );
    let merged_head = git(&s.repo, &["rev-parse", "HEAD"]);
    assert_ne!(merged_head, base_head, "merge records a new commit");
    assert_eq!(current_branch(&s.repo).unwrap(), "main");

    // Merging the SAME branch again is pinned as an Ok no-op: git reports
    // "Already up to date", so merge returns Ok and moves no ref. Callers may
    // therefore retry a merge idempotently.
    merge(&s.repo, &wt1.branch, &locks, "merge T1 again")
        .expect("second merge of an already-merged branch is Ok");
    assert_eq!(
        git(&s.repo, &["rev-parse", "HEAD"]),
        merged_head,
        "repeated merge is a no-op, not another commit"
    );

    // remove: worktree directory and its branch are gone.
    remove(&s.repo, &wt1);
    assert!(!wt1.path.exists(), "worktree removed");
    assert!(
        !git_ok(
            &s.repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/tf/T1"]
        ),
        "branch deleted"
    );
    // T2 was never merged or removed; Scratch::drop wipes it with the base.
    assert!(wt2.path.exists());
}

/// The most important safety property in the file: a CONFLICTING merge returns
/// `Err` and leaves the base repo exactly as it was — same HEAD, same branch,
/// no mid-merge state, clean tree — and pushes NOTHING. This is the abort-not-
/// force path the spec promises (`conflict fails task`).
// spec: worktree/merge-serialization#conflict-fails-task
#[test]
fn conflicting_merge_aborts_and_leaves_base_usable() {
    let s = Scratch::new("conflict");

    // A real remote makes "nothing is force-pushed" observable, not just
    // implied by the absence of a push call.
    let origin = s.base.join("origin.git");
    git(&s.base, &["init", "--bare", "-b", "main", "origin.git"]);
    git(
        &s.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&s.repo, &["push", "-u", "origin", "main"]);
    let remote_head_before = git(&origin, &["rev-parse", "main"]);

    let wt = create_ok(&s.repo, &s.wt_root, "C1", "tf");

    // A genuine conflict on the SAME line of the shared file: main moves
    // forward, then the branch changes that line differently.
    std::fs::write(s.repo.join("f.txt"), "main version\n").unwrap();
    git(&s.repo, &["commit", "-am", "main change"]);
    let head_before = git(&s.repo, &["rev-parse", "HEAD"]);
    let branch_before = current_branch(&s.repo).unwrap();

    std::fs::write(wt.path.join("f.txt"), "branch version\n").unwrap();
    git(&wt.path, &["commit", "-am", "branch change"]);

    let err = merge(&s.repo, &wt.branch, &MergeLocks::new(), "merge C1")
        .expect_err("a conflicting merge must return Err, never panic or force");
    assert!(
        err.contains("merge of tf/C1 failed"),
        "error names the failed branch: {err}"
    );

    // Base repo is still usable and untouched by the aborted merge.
    assert_eq!(
        git(&s.repo, &["rev-parse", "HEAD"]),
        head_before,
        "base HEAD unchanged after abort"
    );
    assert_eq!(
        current_branch(&s.repo).unwrap(),
        branch_before,
        "still on the same branch"
    );
    assert!(
        !s.repo.join(".git/MERGE_HEAD").exists(),
        "not left mid-merge (merge --abort ran)"
    );
    assert!(
        git(&s.repo, &["status", "--porcelain"]).trim().is_empty(),
        "working tree clean after abort (no conflicted files staged)"
    );
    assert_eq!(
        std::fs::read_to_string(s.repo.join("f.txt")).unwrap(),
        "main version\n",
        "the base side of the conflict survives"
    );

    // Nothing was pushed: the remote ref is byte-for-byte where it started,
    // and the abort path never even attempted a forced update.
    assert_eq!(
        git(&origin, &["rev-parse", "main"]),
        remote_head_before,
        "conflicting merge must not push/force-push the base branch"
    );

    // Still usable: a subsequent non-conflicting merge succeeds.
    std::fs::write(wt.path.join("f.txt"), "main version\n").unwrap();
    git(&wt.path, &["commit", "-am", "align branch"]);
    merge(&s.repo, &wt.branch, &MergeLocks::new(), "merge C1 retry")
        .expect("repo remains usable after the aborted conflict");
    assert_eq!(current_branch(&s.repo).unwrap(), "main");
}

/// `MergeLocks` is shareable and actually serializes: merges run through one
/// instance are well-ordered, so two threads merging disjoint branches into
/// the same base repo both land and the result is consistent. A lock that
/// failed to serialize would let the two `git merge` index operations race.
// spec: worktree/merge-serialization#serialized-merges
#[test]
fn merge_locks_serialize_and_are_send_sync() {
    assert_send_sync::<MergeLocks>();
    let locks = MergeLocks::new();
    // The default constructor is part of the public surface too.
    let _default = MergeLocks::default();

    let s = Scratch::new("locks");
    let wt_a = create_ok(&s.repo, &s.wt_root, "A", "tf");
    let wt_b = create_ok(&s.repo, &s.wt_root, "B", "tf");

    // Disjoint commits, both branched from the same base.
    std::fs::write(wt_a.path.join("a.txt"), "a\n").unwrap();
    git(&wt_a.path, &["add", "."]);
    git(&wt_a.path, &["commit", "-m", "A work"]);
    std::fs::write(wt_b.path.join("b.txt"), "b\n").unwrap();
    git(&wt_b.path, &["add", "."]);
    git(&wt_b.path, &["commit", "-m", "B work"]);

    // Both merges share ONE lock instance (cloned Arc), so they cannot
    // interleave. If serialization regressed, this would flake or fail.
    let mut handles = Vec::new();
    for (repo, branch, locks) in [
        (s.repo.clone(), wt_a.branch.clone(), locks.clone()),
        (s.repo.clone(), wt_b.branch.clone(), locks.clone()),
    ] {
        handles.push(std::thread::spawn(move || {
            merge(&repo, &branch, &locks, "parallel merge")
        }));
    }
    for h in handles {
        h.join()
            .expect("merge thread did not panic")
            .expect("serialized merge succeeds");
    }

    // Both branches' files are present: neither merge clobbered the other.
    assert_eq!(
        std::fs::read_to_string(s.repo.join("a.txt")).unwrap(),
        "a\n"
    );
    assert_eq!(
        std::fs::read_to_string(s.repo.join("b.txt")).unwrap(),
        "b\n"
    );
    assert_eq!(current_branch(&s.repo).unwrap(), "main");

    // Repeated merges through the same instance stay consistent (no-op Ok).
    let head = git(&s.repo, &["rev-parse", "HEAD"]);
    merge(&s.repo, &wt_a.branch, &locks, "repeat A").expect("repeat merge is Ok");
    merge(&s.repo, &wt_b.branch, &locks, "repeat B").expect("repeat merge is Ok");
    assert_eq!(
        git(&s.repo, &["rev-parse", "HEAD"]),
        head,
        "repeats move no ref"
    );
}

/// The property a past flake violated: `orphan_ids` reports on-disk ids not in
/// `keep`; `clean` and `heal` remove orphans while leaving `keep`-listed
/// worktrees (and their branches) intact; `clean_all` wipes the root.
// spec: worktree/self-heal-spans-all-repositories
// spec: worktree/self-heal-spans-all-repositories#stale-worktree-is-cleaned-regardless-of-its-repo
#[test]
fn orphan_clean_and_heal_preserve_keep_listed_worktrees() {
    let s = Scratch::new("clean");
    let repos = vec![("main".to_string(), s.repo.clone())];
    let a = create_ok(&s.repo, &s.wt_root, "A", "tf");
    let b = create_ok(&s.repo, &s.wt_root, "B", "tf");
    let c = create_ok(&s.repo, &s.wt_root, "C", "tf");

    // A missing root is not an error and reports nothing.
    assert!(orphan_ids(&s.base.join("does-not-exist"), &[]).is_empty());

    // Orphans are everything on disk except `keep`, sorted for stable output.
    assert_eq!(
        orphan_ids(&s.wt_root, &["B".to_string()]),
        vec!["A".to_string(), "C".to_string()]
    );
    assert_eq!(
        orphan_ids(&s.wt_root, &[]),
        vec!["A".to_string(), "B".to_string(), "C".to_string()]
    );

    // Dry run reports but touches nothing.
    let dry = clean(&repos, &s.wt_root, "tf", &["B".to_string()], true);
    assert_eq!(dry, vec!["A".to_string(), "C".to_string()]);
    assert!(
        a.path.exists() && b.path.exists() && c.path.exists(),
        "dry run leaves every worktree in place"
    );

    // Real clean removes orphans only; the kept worktree and its branch live.
    let removed = clean(&repos, &s.wt_root, "tf", &["B".to_string()], false);
    assert_eq!(removed, vec!["A".to_string(), "C".to_string()]);
    assert!(!a.path.exists() && !c.path.exists(), "orphans removed");
    assert!(b.path.exists(), "keep-listed worktree preserved");
    assert!(
        !git_ok(
            &s.repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/tf/A"]
        ),
        "orphan branch A deleted"
    );
    assert!(
        !git_ok(
            &s.repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/tf/C"]
        ),
        "orphan branch C deleted"
    );
    assert!(
        git_ok(
            &s.repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/tf/B"]
        ),
        "kept branch B survives"
    );

    // heal has the same keep contract: stale dirs go, running ones stay.
    let d = create_ok(&s.repo, &s.wt_root, "D", "tf");
    heal(&repos, &s.wt_root, "tf", &["B".to_string()]);
    assert!(!d.path.exists(), "heal removes the stale D");
    assert!(b.path.exists(), "heal preserves keep-listed B");
    assert!(
        !git_ok(
            &s.repo,
            &["rev-parse", "--verify", "--quiet", "refs/heads/tf/D"]
        ),
        "heal deletes the stale branch"
    );

    // clean_all removes the whole root, including a kept worktree.
    assert!(s.wt_root.exists());
    clean_all(&s.wt_root);
    assert!(!s.wt_root.exists(), "clean_all wipes wt_root");

    // And `clean` with an empty keep list routes through clean_all: it wipes
    // the root rather than leaving empty directories behind.
    let _e = create_ok(&s.repo, &s.wt_root, "E", "tf");
    let _f = create_ok(&s.repo, &s.wt_root, "F", "tf");
    let all = clean(&repos, &s.wt_root, "tf", &[], false);
    assert_eq!(all, vec!["E".to_string(), "F".to_string()]);
    assert!(!s.wt_root.exists(), "empty keep wipes the root");
}

/// `is_repo` / `current_branch` are the cheap probes callers use to decide
/// whether git work is even possible. They answer honestly for a real repo
/// and fail closed for a directory that is not one.
// spec: worktree/worktrees-target-the-task-s-repository
#[test]
fn is_repo_and_current_branch_contract() {
    let s = Scratch::new("probe");
    assert!(is_repo(&s.repo), "scratch repo is a repo");
    assert_eq!(
        current_branch(&s.repo).unwrap(),
        "main",
        "reports the checked-out branch"
    );

    // A directory that is not (inside) a repo: `is_repo` is false and
    // `current_branch` returns Err instead of a bogus branch name.
    let non_repo = s.base.join("not-a-repo");
    std::fs::create_dir_all(&non_repo).unwrap();
    assert!(!is_repo(&non_repo), "plain directory is not a repo");
    assert!(
        current_branch(&non_repo).is_err(),
        "current_branch fails closed outside a repo"
    );

    // `create` refuses a non-repo with a named error rather than half-making
    // a worktree.
    let err = create(&non_repo, &s.wt_root, "X", "tf").unwrap_err();
    assert!(
        err.contains("not a git repository"),
        "create names the non-repo: {err}"
    );
}

/// `archive_branch` keeps a COPY of a branch's tip under a fresh,
/// discoverable `<branch>-rejected-<now>` name (never a rename), leaves the
/// original branch AND its worktree untouched — so the existing cleanup and
/// the retry are unaffected — and never fails the caller: a missing branch
/// or an unresolvable name collision yields `None`.
// spec: worktree/rejected-work-is-preserved-on-an-archived-branch#archiving-never-fails-the-attempt
#[test]
fn archive_branch_copies_the_tip_without_touching_the_original() {
    let s = Scratch::new("archive");
    let wt = create_ok(&s.repo, &s.wt_root, "R1", "tf");
    std::fs::write(wt.path.join("f.txt"), "agent worked\n").unwrap();
    git(&wt.path, &["commit", "-am", "agent work"]);
    let tip = git(&s.repo, &["rev-parse", "tf/R1"]);

    // A real branch archives to a copy pointing at the SAME tip.
    let name = archive_branch(&s.repo, "tf/R1", 1234).expect("archives a real branch");
    assert_eq!(name, "tf/R1-rejected-1234");
    assert_eq!(
        git(&s.repo, &["rev-parse", name.as_str()]).trim(),
        tip.trim(),
        "the copy points at the original's tip"
    );

    // The ORIGINAL branch and its worktree are left alone.
    assert!(
        git_ok(&s.repo, &["rev-parse", "--verify", "refs/heads/tf/R1"]),
        "original branch survives archiving"
    );
    assert!(wt.path.exists(), "worktree untouched by archiving");

    // A name collision resolves via a growing `-<n>` suffix.
    let name2 = archive_branch(&s.repo, "tf/R1", 1234).expect("resolves a name collision");
    assert_eq!(name2, "tf/R1-rejected-1234-1");
    assert_eq!(
        git(&s.repo, &["rev-parse", name2.as_str()]).trim(),
        tip.trim(),
        "the suffixed copy also points at the same tip"
    );

    // A missing branch is `None` — archiving can never fail the caller.
    assert!(archive_branch(&s.repo, "tf/does-not-exist", 999).is_none());

    remove(&s.repo, &wt);
}

/// `af clean` also sweeps the archived rejected refs round 11 leaves behind:
/// `clean_rejected` lists `<prefix>/<id>-rejected-<ts>[-<n>]` branches, skips
/// the ones whose task is still Running (the running-worktree rule applied to
/// refs), deletes the rest best-effort, and touches nothing on a dry run. The
/// pure `archived_task_id` parser recovers the id through hyphens and the
/// collision suffix and ignores foreign prefixes / non-rejected names.
// spec: worktree/af-clean-sweeps-archived-rejected-branches
// spec: worktree/af-clean-sweeps-archived-rejected-branches#sweeps-archived-branches-but-keeps-a-running-task-s
#[test]
fn clean_removes_archived_rejected_branches() {
    let s = Scratch::new("rejected-clean");
    let repos = vec![("main".to_string(), s.repo.clone())];

    // Parser contract: the id contains hyphens, the tail is the archiver's
    // numeric `-<ts>[-<n>]`, and a foreign prefix or a non-rejected name is
    // not ours to sweep.
    assert_eq!(
        archived_task_id("tf/r11-measured-cost-rejected-1791259017", "tf"),
        Some("r11-measured-cost".to_string())
    );
    assert_eq!(
        archived_task_id("tf/x-rejected-1-2", "tf"),
        Some("x".to_string())
    );
    assert_eq!(archived_task_id("tf/x", "tf"), None);
    assert_eq!(archived_task_id("other/x-rejected-1", "tf"), None);
    assert_eq!(archived_task_id("tf/x-rejected-abc", "tf"), None);
    assert_eq!(archived_task_id("tf/x-rejected-1-abc", "tf"), None);

    // Two archived rejected refs of this prefix, a plain branch that must
    // stay, and a FOREIGN-prefixed rejected ref that is not ours.
    let dead = create_ok(&s.repo, &s.wt_root, "dead", "tf");
    let live = create_ok(&s.repo, &s.wt_root, "live", "tf");
    git(&dead.path, &["commit", "--allow-empty", "-m", "dead work"]);
    git(&live.path, &["commit", "--allow-empty", "-m", "live work"]);
    let dead_archived = archive_branch(&s.repo, "tf/dead", 111).expect("archive dead");
    let live_archived = archive_branch(&s.repo, "tf/live", 222).expect("archive live");
    git(&s.repo, &["branch", "other/x-rejected-1"]);

    // Dry run lists exactly the two archived refs (sorted) and removes none.
    let dry = clean_rejected(&repos, "tf", &[], true);
    assert_eq!(
        dry,
        vec![dead_archived.clone(), live_archived.clone()],
        "dry run lists both archived refs"
    );
    assert!(
        branch_exists(&s.repo, &dead_archived) && branch_exists(&s.repo, &live_archived),
        "dry run leaves every archived ref in place"
    );

    // A task still marked Running keeps its archived ref; the other is swept.
    let removed = clean_rejected(&repos, "tf", &["live".to_string()], false);
    assert_eq!(removed, vec![dead_archived.clone()]);
    assert!(
        !branch_exists(&s.repo, &dead_archived),
        "the dead task's archived ref is removed"
    );
    assert!(
        branch_exists(&s.repo, &live_archived),
        "the running task's archived ref survives"
    );

    // A foreign prefix and a plain (non-rejected) branch are never touched.
    assert!(branch_exists(&s.repo, "other/x-rejected-1"));
    assert!(branch_exists(&s.repo, "tf/dead"));

    remove(&s.repo, &dead);
    remove(&s.repo, &live);
}
