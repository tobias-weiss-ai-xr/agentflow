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

/// Keep a COPY of `branch`'s tip under a fresh, discoverable name so the work
/// an attempt paid for survives the cleanup that follows a failure, without
/// disturbing the original branch (the retry MUST still start clean from the
/// current base). With `Some(attempt)` the name is
/// `<branch>-rejected-<attempt>-<now>`; with `None` it stays the legacy
/// `<branch>-rejected-<now>` (startup heal has no attempt to name). A `-<n>`
/// suffix is appended when the first choice is already taken. Returns the new
/// branch name. Never fails the caller: a git error or a name collision that
/// cannot be resolved yields `None`, because the attempt's own failure is the
/// story and archiving is a courtesy.
pub fn archive_branch(repo: &Path, branch: &str, attempt: Option<u32>, now: u64) -> Option<String> {
    // Nothing to preserve when the branch is already gone.
    if !branch_exists(repo, branch) {
        return None;
    }
    let base = match attempt {
        Some(a) => format!("{branch}-rejected-{a}-{now}"),
        None => format!("{branch}-rejected-{now}"),
    };
    let mut name = base.clone();
    let mut n: u64 = 1;
    loop {
        // A COPY (`git branch <new> <branch>`), never a rename — the
        // original branch is left untouched so the existing cleanup and the
        // retry are unaffected. Pick the next free `-<n>` name when the
        // requested one already exists; a git error or an unresolvable
        // collision falls through to `None`.
        if !branch_exists(repo, &name) && git(repo, &["branch", &name, branch]).passed() {
            return Some(name);
        }
        name = format!("{base}-{n}");
        n += 1;
    }
}

/// The parsed fields of an archived rejected branch name built by
/// [`archive_branch`]: `<prefix>/<id>-rejected-<attempt>-<unix-ts>` (plus an
/// optional `-<n>` collision suffix), or the legacy
/// `<prefix>/<id>-rejected-<unix-ts>` where `attempt` is `0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedBranch {
    pub task_id: String,
    /// 1-based attempt number the archive was named for; `0` for a legacy
    /// archive whose name carries no attempt.
    pub attempt: u32,
    /// Unix seconds the archiver wrote.
    pub ts: u64,
    /// The `-<n>` collision suffix (0 when the first choice of name was free).
    pub collision: u64,
}

/// [`archive_branch`] writes real wall-clock seconds (well past 2001), so a
/// numeric tail field `>= 1_000_000_000` is the ts and the field just before
/// it is the attempt. This is what lets the parser tell the current
/// `<attempt>-<ts>` form apart from the legacy `<ts>` / `<ts>-<n>` forms
/// without a format flag.
const TS_FLOOR: u64 = 1_000_000_000;

/// Recover the task id from an archived rejected branch name built by
/// [`archive_branch`]. Returns `None` for anything that is not an archived
/// rejected branch of `prefix` — a plain branch (`tf/x`), a foreign prefix
/// (`other/x-rejected-1`), or a malformed tail. See
/// [`parse_archived_branch`] for the accepted forms.
pub fn archived_task_id(branch: &str, prefix: &str) -> Option<String> {
    parse_archived_branch(branch, prefix).map(|a| a.task_id)
}

/// Parse an archived rejected branch name built by [`archive_branch`] into
/// its fields. Accepts the current
/// `<prefix>/<id>-rejected-<attempt>-<unix-ts>` form (plus a `-<n>` collision
/// suffix) and the legacy `<prefix>/<id>-rejected-<unix-ts>` form (plus
/// `-<n>`, where `attempt` is `0`). Returns `None` for anything that is not
/// an archived rejected branch of `prefix` — a plain branch (`tf/x`), a
/// foreign prefix (`other/x-rejected-1`), or a malformed tail.
///
/// Task ids contain hyphens (`r11-measured-cost`), so the parser cannot count
/// hyphens: it locates the LAST `-rejected-` marker (everything after it is a
/// numeric tail the archiver generated). A one-field tail is a legacy `<ts>`;
/// a two-field tail is either the current `<attempt>-<ts>` (when the second
/// field is a real ts `>= 1_000_000_000`) or a legacy `<ts>-<n>`; a
/// three-field tail is the current `<attempt>-<ts>-<n>` and requires a real
/// ts. This is why `tf/x-rejected-1-2` yields the legacy ts `1`, not the
/// attempt `1`, and `tf/x-rejected-1-2-3` is rejected.
pub fn parse_archived_branch(branch: &str, prefix: &str) -> Option<ArchivedBranch> {
    let rest = branch.strip_prefix(prefix)?.strip_prefix('/')?;
    const MARKER: &str = "-rejected-";
    let at = rest.rfind(MARKER)?;
    let id = &rest[..at];
    if id.is_empty() {
        return None;
    }
    let tail = &rest[at + MARKER.len()..];
    let parts: Vec<&str> = tail.split('-').collect();
    if !parts.iter().all(|p| is_digits(p)) {
        return None;
    }
    let num = |s: &str| s.parse::<u64>().ok();
    match parts.as_slice() {
        [ts] => Some(ArchivedBranch {
            task_id: id.to_string(),
            attempt: 0,
            ts: num(ts)?,
            collision: 0,
        }),
        [a, b] => {
            let (a, b) = (num(a)?, num(b)?);
            if b >= TS_FLOOR && a < TS_FLOOR {
                // Current form: <attempt>-<ts>.
                Some(ArchivedBranch {
                    task_id: id.to_string(),
                    attempt: u32::try_from(a).ok()?,
                    ts: b,
                    collision: 0,
                })
            } else {
                // Legacy collision: <ts>-<n>.
                Some(ArchivedBranch {
                    task_id: id.to_string(),
                    attempt: 0,
                    ts: a,
                    collision: b,
                })
            }
        }
        [attempt, ts, n] => {
            let (attempt, ts, n) = (num(attempt)?, num(ts)?, num(n)?);
            if ts < TS_FLOOR {
                return None;
            }
            Some(ArchivedBranch {
                task_id: id.to_string(),
                attempt: u32::try_from(attempt).ok()?,
                ts,
                collision: n,
            })
        }
        _ => None,
    }
}

/// A non-empty run of ASCII digits (the numeric tail fields the archiver
/// writes).
fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Every local branch under `<prefix>/` in `repo`, as full short names
/// (deterministic input for [`newest_archived_branch`]). A git error yields
/// an empty list — selection is best-effort, never fatal.
pub fn archived_branches(repo: &Path, prefix: &str) -> Vec<String> {
    let out = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            &format!("refs/heads/{prefix}/"),
        ],
    );
    if !out.passed() {
        return Vec::new();
    }
    out.stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// The NEWEST archived rejected branch for task `id` among `branches`,
/// decided PURESTLY by parsing the numeric `<ts>` (then the attempt number,
/// then the `-<n>` collision suffix) — never by git's output order and never
/// by committer dates, so the same branch set always selects the same branch.
/// Returns the full branch name. Foreign prefixes and non-rejected names are
/// ignored by [`parse_archived_branch`].
pub fn newest_archived_branch(branches: &[String], prefix: &str, id: &str) -> Option<String> {
    branches
        .iter()
        .filter_map(|b| {
            parse_archived_branch(b, prefix)
                .filter(|a| a.task_id == id)
                .map(|a| (a.ts, a.attempt, a.collision, b.clone()))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)))
        .map(|(_, _, _, b)| b)
}

/// The NEWEST archived rejected branch for task `id` whose parsed attempt is
/// exactly `attempt`, decided PURELY by parsing the numeric `<ts>` (then the
/// collision suffix) — never by git's output order and never by committer
/// dates. `attempt` may be `0` to select a legacy archive with no attempt in
/// its name. Foreign prefixes, other tasks and non-rejected names are ignored
/// by [`parse_archived_branch`]. Returns the full branch name.
pub fn archived_branch_for_attempt(
    branches: &[String],
    prefix: &str,
    id: &str,
    attempt: u32,
) -> Option<String> {
    branches
        .iter()
        .filter_map(|b| {
            parse_archived_branch(b, prefix)
                .filter(|a| a.task_id == id && a.attempt == attempt)
                .map(|a| (a.ts, a.collision, b.clone()))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, _, b)| b)
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

/// Any uncommitted change (staged, unstaged, or untracked) in the worktree
/// at `path`? `git status --porcelain` is non-empty exactly when the tree
/// differs from its HEAD. An unreadable status is an error, never "clean".
pub fn is_dirty(path: &Path) -> Result<bool, String> {
    let out = git(path, &["status", "--porcelain"]);
    if !out.passed() {
        return Err(format!(
            "cannot read worktree status in {}: {}",
            path.display(),
            out.combined().trim()
        ));
    }
    Ok(!out.stdout.trim().is_empty())
}

/// Make the agent's work DURABLE on the attempt branch: commit every
/// uncommitted change in the worktree at `path` so the scope check, the
/// acceptance gate and the merge all judge exactly the same committed tree.
/// A clean worktree is a no-op. Never passes `--no-verify`: the operator's
/// git hooks are theirs to run (a hook that rejects the work fails the
/// attempt, which is the honest outcome). An error carries git's output so
/// the caller can name the reason.
pub fn commit_all(path: &Path, msg: &str) -> Result<(), String> {
    if !is_dirty(path)? {
        return Ok(());
    }
    let add = git(path, &["add", "-A"]);
    if !add.passed() {
        return Err(add.combined().trim().to_string());
    }
    let commit = git(path, &["commit", "-m", msg]);
    if !commit.passed() {
        return Err(commit.combined().trim().to_string());
    }
    Ok(())
}

/// Attach a worktree to an EXISTING branch — the gate-only-retry
/// counterpart of [`create`]: the branch is never deleted or recreated,
/// because the agent's committed work on it is exactly what the retry must
/// preserve (a gate failure must not discard paid agent work). Any leftover
/// dir/registration at `wt_root/<id>` is cleared first so the branch can be
/// checked out fresh from its own tip.
pub fn attach(repo: &Path, wt_root: &Path, id: &str, prefix: &str) -> Result<Worktree, String> {
    attach_existing(repo, wt_root, id, &format!("{prefix}/{id}"))
}

/// Attach a worktree to an arbitrary EXISTING branch at `wt_root/<id>` —
/// the [`attach`] generalisation used by `af recover` to check out an
/// archived rejected branch for re-validation. The branch is never created
/// or moved: the committed work it points at is exactly what re-validation
/// must preserve. Any leftover dir/registration at `wt_root/<id>` is cleared
/// first so the branch can be checked out fresh from its own tip.
pub fn attach_existing(
    repo: &Path,
    wt_root: &Path,
    id: &str,
    branch: &str,
) -> Result<Worktree, String> {
    if !is_repo(repo) {
        return Err(format!("{} is not a git repository", repo.display()));
    }
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
    let out = git(repo, &["worktree", "add", path.to_str().unwrap(), branch]);
    if !out.passed() {
        return Err(format!(
            "worktree attach failed ({:?}): {}",
            out.kind,
            out.combined().trim()
        ));
    }
    Ok(Worktree {
        path,
        branch: branch.to_string(),
    })
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
///
/// Before a stale branch is deleted, its committed work is archived under
/// `<branch>-rejected-<now>` ([`archive_branch`]) so an interrupted attempt —
/// the orchestrator was killed mid-flight, or the task is no longer marked
/// running — does not silently lose the agent's commits. Preserving is
/// best-effort and never fatal, and the running-task rule is unchanged: a
/// task still marked running is never touched.
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
            let branch = format!("{branch_prefix}/{id}");
            // Preserve the branch's committed work BEFORE it is removed:
            // the stale worktree is exactly the "af was killed mid-attempt"
            // case, where the agent may have committed real work.
            archive_stale_branch(repo, &branch);
            let _ = git(
                repo,
                &["worktree", "remove", "--force", e.path().to_str().unwrap()],
            );
            let _ = git(repo, &["branch", "-D", &branch]);
        }
    }
}

/// Best-effort preservation of a stale branch's committed work before
/// [`heal`] deletes it. Archives a COPY under `<branch>-rejected-<now>` only
/// when `branch` exists AND carries commits beyond the repo's current branch
/// (a fresh worktree with no work must not leave a meaningless ref). Never
/// fails the caller: an unanswerable base (`detached HEAD`, missing branch,
/// git error) preserves nothing — deletion still proceeds.
fn archive_stale_branch(repo: &Path, branch: &str) {
    let Ok(base) = current_branch(repo) else {
        return;
    };
    if !matches!(commits_ahead(repo, &base, branch), Ok(n) if n > 0) {
        return;
    }
    let _ = archive_branch(repo, branch, None, crate::state::now_ts());
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

/// Archived rejected branches under `<prefix>/` whose task id is not in
/// `keep` (tasks still marked Running), sorted and de-duplicated across
/// repos. With `dry_run`, only LISTS. Otherwise each branch is deleted
/// best-effort (`git branch -D`) in every repo that owns it — a git error
/// never fails the caller, exactly like [`archive_branch`]. Returns the full
/// branch names that were listed (and, when not `dry_run`, targeted for
/// removal).
pub fn clean_rejected(
    repos: &[(String, PathBuf)],
    branch_prefix: &str,
    keep: &[String],
    dry_run: bool,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, repo) in repos {
        let out = git(
            repo,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                &format!("refs/heads/{branch_prefix}/"),
            ],
        );
        if !out.passed() {
            continue;
        }
        for line in out.stdout.lines() {
            let name = line.trim();
            if name.is_empty() || names.iter().any(|n| n == name) {
                continue;
            }
            match archived_task_id(name, branch_prefix) {
                Some(id) if !keep.iter().any(|k| k == &id) => names.push(name.to_string()),
                // Not an archived rejected branch, or its task is running:
                // the running-worktree rule applies to archived refs too.
                _ => {}
            }
        }
    }
    names.sort();
    if dry_run {
        return names;
    }
    for name in &names {
        // Only one repo owns the ref; the others fail harmlessly.
        for (_, repo) in repos {
            let _ = git(repo, &["branch", "-D", name]);
        }
    }
    names
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

    /// `commit_all` makes a dirty worktree durable under the given message
    /// and is a no-op on a clean one; `is_dirty` reports both states.
    #[test]
    fn commit_all_makes_a_dirty_worktree_durable_and_is_a_noop_when_clean() {
        let repo = scratch_repo();
        let wt_root = repo.parent().unwrap().join("wt");
        let wt = create_ok(&repo, &wt_root, "D1");
        // Fresh worktree sits at the base tip: clean, and commit_all is a
        // no-op that does not invent a commit.
        assert!(!is_dirty(&wt.path).unwrap(), "fresh worktree is clean");
        commit_all(&wt.path, "af: D1 attempt 1 — agent left uncommitted work").unwrap();
        assert_eq!(commits_ahead(&repo, "main", &wt.branch).unwrap(), 0);
        // A dirty edit is committed under the harness message and becomes
        // real work beyond the base.
        std::fs::write(wt.path.join("dirty.txt"), "agent work\n").unwrap();
        assert!(is_dirty(&wt.path).unwrap(), "uncommitted edit is dirty");
        commit_all(&wt.path, "af: D1 attempt 1 — agent left uncommitted work").unwrap();
        assert!(!is_dirty(&wt.path).unwrap(), "commit cleaned the tree");
        assert_eq!(commits_ahead(&repo, "main", &wt.branch).unwrap(), 1);
        let msg = git(&wt.path, &["log", "-1", "--format=%s"]);
        assert_eq!(
            msg.stdout.trim(),
            "af: D1 attempt 1 — agent left uncommitted work"
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
    fn archived_task_id_recovers_ids_and_rejects_foreign_names() {
        // Task ids contain hyphens; the numeric tail after the LAST marker
        // is what separates the id from the archiver's `-<ts>[-<n>]`.
        assert_eq!(
            archived_task_id("tf/r11-measured-cost-rejected-1791259017", "tf"),
            Some("r11-measured-cost".to_string())
        );
        // Collision form `-<ts>-<n>`: the suffix is not part of the id.
        assert_eq!(
            archived_task_id("tf/x-rejected-1-2", "tf"),
            Some("x".to_string())
        );
        // An id may itself contain the marker; the LAST one wins.
        assert_eq!(
            archived_task_id("tf/a-rejected-b-rejected-7", "tf"),
            Some("a-rejected-b".to_string())
        );
        // Not archived rejected branches of this prefix.
        assert_eq!(archived_task_id("tf/x", "tf"), None);
        assert_eq!(archived_task_id("other/x-rejected-1", "tf"), None);
        assert_eq!(archived_task_id("tf/x-rejected", "tf"), None);
        assert_eq!(archived_task_id("tf/x-rejected-abc", "tf"), None);
        assert_eq!(archived_task_id("tf/x-rejected-1-abc", "tf"), None);
        assert_eq!(archived_task_id("tf/x-rejected-1-2-3", "tf"), None);
        assert_eq!(archived_task_id("tf/-rejected-1", "tf"), None);
    }

    /// `af recover` selection is a pure decision on the PARSED `<ts>` (then
    /// the `-<n>` collision suffix) — never git output order, never committer
    /// dates. The same branch set always selects the same branch.
    // spec: worktree/recovered-branches-are-re-validated-then-merged
    #[test]
    fn newest_archived_branch_picks_by_parsed_ts_then_collision() {
        // Newest ts wins regardless of list order.
        let branches = vec![
            "tf/A-rejected-111".to_string(),
            "tf/A-rejected-222".to_string(),
            "tf/A-rejected-333".to_string(),
        ];
        assert_eq!(
            newest_archived_branch(&branches, "tf", "A").as_deref(),
            Some("tf/A-rejected-333")
        );
        // The `-<n>` collision suffix breaks a ts tie: `-2` is newer than `-1`.
        let branches = vec![
            "tf/A-rejected-222-1".to_string(),
            "tf/A-rejected-222-2".to_string(),
        ];
        assert_eq!(
            newest_archived_branch(&branches, "tf", "A").as_deref(),
            Some("tf/A-rejected-222-2")
        );
        // A bare (collision-less) name sorts before the `-n` forms under the
        // same ts (n=0 < n=1).
        let branches = vec![
            "tf/A-rejected-222-1".to_string(),
            "tf/A-rejected-222".to_string(),
        ];
        assert_eq!(
            newest_archived_branch(&branches, "tf", "A").as_deref(),
            Some("tf/A-rejected-222-1")
        );
        // Only this task's archived branches are considered: a foreign
        // prefix and a plain branch of this prefix are ignored.
        let branches = vec![
            "other/A-rejected-999".to_string(),
            "tf/A".to_string(),
            "tf/B-rejected-999".to_string(),
            "tf/A-rejected-444".to_string(),
        ];
        assert_eq!(
            newest_archived_branch(&branches, "tf", "A").as_deref(),
            Some("tf/A-rejected-444")
        );
        // No matching archived branch at all.
        assert_eq!(newest_archived_branch(&branches, "tf", "Z"), None);
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
