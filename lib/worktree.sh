#!/usr/bin/env bash
# worktree.sh — git worktree lifecycle for taskfleet.
#
# Each task runs in its own worktree on branch "$TF_BRANCH_PREFIX/<TASK_ID>",
# branched from origin/main (or local main). This gives full isolation:
# concurrent workers never touch each other's files.

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# Ensure the worktree root is gitignored at the repo root — but ONLY when the
# worktree root lives INSIDE the repo (out-of-repo roots can't pollute status).
# Idempotent.
tf_worktree_ensure_gitignore() {
  case "$TF_WORKTREE_ROOT" in
    "$TF_REPO_DIR"/*) : ;;  # inside repo → must be ignored
    *) return 0 ;;          # outside repo → no pollution, nothing to do
  esac
  local gi="$TF_REPO_DIR/.gitignore"
  local rel="${TF_WORKTREE_ROOT#"$TF_REPO_DIR/"}"
  if (cd "$TF_REPO_DIR" && git check-ignore -q "$rel" 2>/dev/null); then
    return 0
  fi
  tf_info "adding $rel to .gitignore (worktree root was not ignored)"
  {
    printf '\n# taskfleet worktrees (do not commit)\n%s/\n' "$rel"
  } >> "$gi"
}

# tf_worktree_create <task_id> [base_ref] [--keep-branch]
#   Prints the worktree path. Creates branch $TF_BRANCH_PREFIX/<task_id>.
#   --keep-branch: reuse an existing branch (its commits are preserved) instead
#   of resetting it to the base — used when retrying a merge-conflict failure so
#   the agent resumes its own work rather than starting from scratch.
tf_worktree_create() {
  local id="$1" base="${2:-main}" keep=0
  [[ "${3:-}" == "--keep-branch" ]] && keep=1
  local branch="$TF_BRANCH_PREFIX/$id"
  local wt="$TF_WORKTREE_ROOT/$id"
  tf_worktree_ensure_gitignore

  if [[ -d "$wt" ]]; then
    tf_warn "$id: worktree exists at $wt, removing stale copy"
    tf_worktree_remove "$id" --force || true
  fi

  # Update base ref so we branch from the latest merged state.
  (cd "$TF_REPO_DIR" && git fetch --quiet github 2>/dev/null || true)
  (cd "$TF_REPO_DIR" && git rev-parse --verify --quiet "$base" >/dev/null) || base="main"

  if ! (cd "$TF_REPO_DIR" && git rev-parse --verify --quiet "$branch" >/dev/null); then
    (cd "$TF_REPO_DIR" && git worktree add -b "$branch" "$wt" "$base" >/dev/null 2>&1) || {
      tf_error "$id: git worktree add failed for $branch from $base"
      return 1
    }
  elif [[ "$keep" == "1" ]]; then
    # Resume preserved work: attach a worktree to the existing branch. The
    # agent (guided by PREVIOUS_ERROR) will rebase onto main and resolve the
    # conflict in-place.
    tf_info "$id: resuming preserved branch $branch (merge-conflict retry)"
    (cd "$TF_REPO_DIR" && git worktree add "$wt" "$branch" >/dev/null 2>&1) || {
      tf_error "$id: git worktree add (preserved branch) failed"
      return 1
    }
  else
    # branch exists (interrupted prior run) — reset to fresh base so the retry
    # starts from current main, NOT stale state. A stale branch would re-conflict
    # on merge because main has advanced since the branch was created.
    (cd "$TF_REPO_DIR" && git branch -f "$branch" "$base" >/dev/null 2>&1) || {
      tf_error "$id: git branch -f (reset stale branch) failed"
      return 1
    }
    (cd "$TF_REPO_DIR" && git worktree add "$wt" "$branch" >/dev/null 2>&1) || {
      tf_error "$id: git worktree add (existing branch) failed"
      return 1
    }
  fi
  echo "$wt"
}

# tf_worktree_remove <task_id> [--force]
tf_worktree_remove() {
  local id="$1"; shift
  local force=""
  [[ "${1:-}" == "--force" ]] && force="--force"
  local wt="$TF_WORKTREE_ROOT/$id"
  if [[ -d "$wt" ]]; then
    (cd "$TF_REPO_DIR" && git worktree remove $force "$wt" 2>/dev/null) || {
      # worktree may have untracked files; prune metadata instead
      rm -rf "$wt"
      (cd "$TF_REPO_DIR" && git worktree prune)
    }
  fi
}

# tf_worktree_merge <task_id>
#   Merge the task branch into main under the merge lock. Must be serial so
#   concurrent workers don't race on `git checkout main` / merge.
tf_worktree_merge() {
  local id="$1"
  local branch="$TF_BRANCH_PREFIX/$id"
  local merge_lock="${TF_MERGE_LOCK:-$TF_STATE_DIR/merge.lock}"
  mkdir -p "$(dirname "$merge_lock")"
  (
    flock 9
    cd "$TF_REPO_DIR" || return 1
    # Ensure main is on a clean working tree: failed merges can leave
    # untracked artifacts (e.g. op.rs from a partially-applied merge) and
    # modifications (conflict markers in shared lib.rs). Without cleaning,
    # subsequent merges fail with "Your local changes would be overwritten".
    # worktrees/ and orchestrator state/ are gitignored → safe from git clean.
    git checkout --quiet main 2>/dev/null || true
    git reset --hard --quiet main
    git clean --quiet -fd
    local before
    before="$(git rev-parse HEAD)"
    # Fast path: branch is already based on current main → plain ff-only.
    if git merge --ff-only "$branch" >/dev/null 2>&1; then
      # Verify merge actually advanced main (not a no-op): after an ff-only
      # merge HEAD always equals the branch — the no-op case is when HEAD did
      # NOT move at all (branch points at the same commit as main).
      if [[ "$before" == "$(git rev-parse HEAD)" ]]; then
        tf_warn "$id: ff-only merge was no-op (branch HEAD == main HEAD); skipping"
        return 1
      fi
      return 0
    fi
    # Diverged path: rebase the task branch onto the latest main, then
    # ff-only merge. 3-way rebase auto-resolves the common case where the
    # task and other tasks touched DIFFERENT files (main moved while the
    # agent was working) — the main source of "merge conflict (gate passed
    # but main diverged)" failures.
    #
    # NOTE: the branch is checked out in the task's worktree, so git refuses
    # to rebase it from here ("branch already used by worktree"). The rebase
    # must run from INSIDE the worktree.
    local wt
    wt="$TF_WORKTREE_ROOT/$id"
    # Rebase path (worktree present): rebase runs from INSIDE the worktree
    # because git refuses to rebase a branch checked out in another worktree.
    if [[ -d "$wt" ]] && (cd "$wt" && git rebase --autostash main >/dev/null 2>&1); then
      cd "$TF_REPO_DIR"
      if git merge --ff-only "$branch" >/dev/null 2>&1; then
        if [[ "$before" != "$(git rev-parse HEAD)" ]]; then
          tf_info "$id: rebased onto main and merged (ff-only)"
          return 0
        fi
      fi
      (cd "$wt" 2>/dev/null && git rebase --abort 2>/dev/null) || true
      cd "$TF_REPO_DIR"
    fi
    # No-ff fallback (worktree absent or rebase refused): 3-way merge handles
    # non-overlapping changes without needing the worktree.
    if git merge --no-ff -m "merge($id): agent task completed" "$branch" >/dev/null 2>&1; then
      if [[ "$before" != "$(git rev-parse HEAD)" ]]; then
        return 0
      fi
    fi
    # Merge/rebase conflicted or produced nothing. Abort cleanly but KEEP the
    # branch and worktree so the agent's work is preserved for retry/manual
    # resolution (never delete on merge failure).
    git merge --abort 2>/dev/null || true
    git checkout --quiet main 2>/dev/null || true
    git reset --hard --quiet main
    git clean --quiet -fd
    return 1
  ) 9>"$merge_lock"
}

# tf_worktree_conflicts <task_id> → names of files with unresolved conflict
# markers in the task's worktree (empty if none). Run AFTER a failed merge
# but BEFORE aborting, or in the worktree after a conflicted rebase.
tf_worktree_conflicts() {
  local id="$1"
  local wt="$TF_WORKTREE_ROOT/$id"
  (cd "$wt" 2>/dev/null && git diff --name-only --diff-filter=U 2>/dev/null) || true
}

# Delete the task branch (after successful merge). Keeps the reflog for recovery.
tf_worktree_delete_branch() {
  local id="$1"
  local branch="$TF_BRANCH_PREFIX/$id"
  (cd "$TF_REPO_DIR" && git branch --quiet -D "$branch" 2>/dev/null) || true
}

# List active worktrees (for diagnostics)
tf_worktree_list() {
  (cd "$TF_REPO_DIR" && git worktree list)
}
