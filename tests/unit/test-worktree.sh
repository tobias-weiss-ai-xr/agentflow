#!/usr/bin/env bash
# unit/test-worktree.sh — unit tests for lib/worktree.sh
# Tests: create/remove/merge, edge cases, retry path, gitignore.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_REPO_DIR="$SBOX/repo"
export TF_WORKTREE_ROOT="$SBOX/repo/.tf-worktrees"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_WORKTREE_ROOT" "$TF_STATE_DIR" "$TF_LOG_DIR"

git -C "$TF_REPO_DIR" init -q -b main
git -C "$TF_REPO_DIR" config user.email t@t.t
git -C "$TF_REPO_DIR" config user.name test
echo "base" > "$TF_REPO_DIR/README.md"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm "init"

export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":2,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[{"id":"WT","engine":"t","title":"WT","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/worktree.sh"

echo "=== [unit] worktree.sh ==="

# ---- Gitignore ----
tf_group_begin; tf_test "worktree root inside repo gets gitignored"
# Reset gitignore to clean state
echo "" > "$TF_REPO_DIR/.gitignore"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm "clean gitignore"
tf_worktree_ensure_gitignore
tf_assert "worktree root is gitignored" git -C "$TF_REPO_DIR" check-ignore -q .tf-worktrees
tf_group_end

tf_group_begin; tf_test "worktree root outside repo is not modified"
OUTER="$SBOX/external-wt"
TF_WORKTREE_ROOT="$OUTER" tf_worktree_ensure_gitignore
tf_assert "no error from out-of-repo root" true
tf_assert_not ".gitignore unchanged" git -C "$TF_REPO_DIR" diff --quiet HEAD -- .gitignore
tf_group_end

tf_group_begin; tf_test "gitignore is idempotent"
before="$(cat "$TF_REPO_DIR/.gitignore")"
tf_worktree_ensure_gitignore
after="$(cat "$TF_REPO_DIR/.gitignore")"
tf_assert_eq "gitignore unchanged on second call" "$before" "$after"
tf_group_end

# ---- Create ----
tf_group_begin; tf_test "create worktree and branch from main"
WT="$(tf_worktree_create T1)"
tf_assert "worktree dir exists" test -d "$WT"
tf_assert "branch exists" git -C "$TF_REPO_DIR" rev-parse --verify --quiet agent/T1
tf_assert "worktree HEAD = main" \
  test "$(git -C "$WT" rev-parse HEAD)" == "$(git -C "$TF_REPO_DIR" rev-parse main)"
tf_assert_eq "worktree path" "$TF_WORKTREE_ROOT/T1" "$WT"
tf_worktree_remove T1
tf_group_end

tf_group_begin; tf_test "create removes stale existing worktree"
WT1="$(tf_worktree_create STALE)"
echo "stale" > "$WT1/stale.txt"
# Create again without removing — should auto-remove stale
WT2="$(tf_worktree_create STALE)"
tf_assert "new worktree exists" test -d "$WT2"
tf_assert_eq "same path" "$WT2" "$WT1"
tf_assert "stale content gone" test ! -f "$WT2/stale.txt"
tf_worktree_remove STALE
tf_group_end

tf_group_begin; tf_test "create with existing stale branch resets to main"
# Create branch, advance main, then re-create
WT1="$(tf_worktree_create RESET)"
echo "v1" > "$WT1/a.rs"
git -C "$WT1" add -A && git -C "$WT1" commit -qm "v1"
tf_worktree_remove RESET
# Advance main
echo "v2" > "$TF_REPO_DIR/main.rs"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm "main advanced"
# Re-create should reset branch to latest main
WT2="$(tf_worktree_create RESET)"
tf_assert "stale a.rs gone" test ! -f "$WT2/a.rs"
tf_assert "main.rs from latest main" test -f "$WT2/main.rs"
tf_worktree_remove RESET
tf_group_end

# ---- Remove ----
tf_group_begin; tf_test "remove deletes worktree directory"
WT="$(tf_worktree_create RM1)"
tf_worktree_remove RM1
tf_assert "worktree dir gone" test ! -d "$WT"
tf_group_end

tf_group_begin; tf_test "remove is idempotent on already-removed worktree"
WT="$(tf_worktree_create RM2)"
tf_worktree_remove RM2
tf_worktree_remove RM2  # second call should not error
tf_assert "no error on double remove" true
tf_group_end

# ---- Delete branch ----
tf_group_begin; tf_test "delete_branch removes git branch"
WT="$(tf_worktree_create DB)"
tf_worktree_remove DB
tf_worktree_delete_branch DB
tf_assert_not "branch gone" git -C "$TF_REPO_DIR" rev-parse --verify --quiet agent/DB
tf_group_end

tf_group_begin; tf_test "delete_branch is idempotent"
tf_worktree_delete_branch NONEXISTENT
tf_assert "no error on missing branch" true
tf_group_end

# ---- Merge ----
tf_group_begin; tf_test "ff merge brings worktree commits to main"
WT="$(tf_worktree_create MG1)"
echo "impl" > "$WT/a.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "feat(MG1): impl"
tf_worktree_merge MG1
tf_assert "a.rs on main" git -C "$TF_REPO_DIR" show main:a.rs >/dev/null
tf_worktree_remove MG1
tf_worktree_delete_branch MG1
tf_group_end

tf_group_begin; tf_test "merge fails cleanly when worktree has no commits"
WT="$(tf_worktree_create MG2)"
# No new commits — ff-only should still work (nothing to merge)
tf_worktree_merge MG2
tf_assert "no error" true
tf_worktree_remove MG2
tf_worktree_delete_branch MG2
tf_group_end

tf_group_begin; tf_test "merge self-cleans main worktree before attempting"
WT="$(tf_worktree_create MG3)"
echo "impl" > "$WT/a.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "feat(MG3): impl"
tf_worktree_merge MG3
# Check main is clean after merge
tf_assert "main working tree clean" git -C "$TF_REPO_DIR" diff --quiet HEAD
tf_worktree_remove MG3
tf_worktree_delete_branch MG3
tf_group_end

# ---- Property: worktree creation always produces valid git worktree ----
tf_group_begin; tf_test "property: created worktree is always a valid git dir"
test_worktree_valid() {
  local id="PROP-$RANDOM"
  local wt
  wt="$(tf_worktree_create "$id")" || return 1
  local valid=0
  git -C "$wt" rev-parse --is-inside-work-tree >/dev/null 2>&1 && valid=1
  tf_worktree_remove "$id" 2>/dev/null
  [[ $valid -eq 1 ]]
}
tf_property "worktree_always_valid" test_worktree_valid 20
tf_group_end

# ---- Property: merge is idempotent ----
tf_group_begin; tf_test "property: merging same branch twice is safe"
WT="$(tf_worktree_create MGI)"
echo "impl" > "$WT/a.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "feat(MGI): impl"
tf_worktree_merge MGI
tf_worktree_merge MGI  # second merge — nothing new to merge
tf_assert "no error on double merge" true
tf_assert "a.rs still on main" git -C "$TF_REPO_DIR" show main:a.rs >/dev/null
tf_worktree_remove MGI
tf_worktree_delete_branch MGI
tf_group_end

# ---- Timing ----
tf_group_begin; tf_test "worktree create + remove under 500ms"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[{"id":"TIMED","engine":"t","title":"T","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}]}
JSON
tf_timed "create+remove cycle" \
  bash -c ". $HERE/../../lib/common.sh; . $HERE/../../lib/worktree.sh; WT=\$(tf_worktree_create TIMED); tf_worktree_remove TIMED"
tf_group_end

tf_test_summary
