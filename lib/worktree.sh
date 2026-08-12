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

# tf_worktree_create <task_id> [base_ref]
#   Prints the worktree path. Creates branch $TF_BRANCH_PREFIX/<task_id>.
tf_worktree_create() {
  local id="$1" base="${2:-main}"
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
    if git merge --ff-only "$branch" >/dev/null 2>&1; then
      return 0
    fi
    if git merge --no-ff -m "merge($id): agent task completed" "$branch" >/dev/null 2>&1; then
      return 0
    fi
    # Clean up the failed merge's working tree artifacts so the next
    # merge doesn't inherit them.
    git merge --abort 2>/dev/null || true
    git reset --hard --quiet main
    git clean --quiet -fd
    return 1
  ) 9>"$merge_lock"
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
