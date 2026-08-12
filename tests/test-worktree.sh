#!/usr/bin/env bash
# test-worktree.sh — git worktree create/remove/merge lifecycle, isolated.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT

# Isolated fake repo (NOT the real server checkout)
export TF_REPO_DIR="$SBOX/repo"
export TF_WORKTREE_ROOT="$TF_REPO_DIR/.tf-worktrees"   # inside repo (real default)
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_REPO_DIR" "$TF_WORKTREE_ROOT" "$TF_STATE_DIR" "$TF_LOG_DIR"

git -C "$TF_REPO_DIR" init -q -b main
git -C "$TF_REPO_DIR" config user.email t@t.t
git -C "$TF_REPO_DIR" config user.name test
echo "base" > "$TF_REPO_DIR/README.md"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm "init"

# Minimal config so common.sh resolves
export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"
echo '{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
       "defaults":{"max_attempts":2,"retry_cooldown_s":1,"accept_timeout_s":5,"dispatch_timeout_s":30}}' \
  > "$TF_CONFIG_DIR/workers.json"
echo '{"tasks":[{"id":"T1","engine":"t","title":"T1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}]}' \
  > "$TF_CONFIG_DIR/tasks.json"

. "$HERE/../lib/common.sh"
. "$HERE/../lib/worktree.sh"

echo "=== Worktree lifecycle tests ==="

tf_group_begin; tf_test "worktree gitignore entry is added when missing (in-repo root)"
tf_worktree_ensure_gitignore
tf_assert "worktree root is gitignored" git -C "$TF_REPO_DIR" check-ignore -q .tf-worktrees
# out-of-repo root should be a no-op (no error)
TF_WORKTREE_ROOT="$SBOX/external" tf_worktree_ensure_gitignore
tf_assert "out-of-repo root skipped cleanly" true
tf_group_end

tf_group_begin; tf_test "create worktree + branch from main"
WT="$(tf_worktree_create T1)"
tf_assert "worktree dir exists"        test -d "$WT"
tf_assert "branch exists"              git -C "$TF_REPO_DIR" rev-parse --verify --quiet agent/T1
tf_assert "worktree shares HEAD as main" git -C "$WT" rev-parse --quiet --verify HEAD
tf_assert_eq "worktree path suffix" "/T1" "${WT#$TF_WORKTREE_ROOT}"
tf_group_end

tf_group_begin; tf_test "edit + commit in worktree, then merge to main"
echo "impl" > "$WT/a.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "feat(T1): T1"
# main should not yet have a.rs
tf_assert_not "a.rs absent on main pre-merge" git -C "$TF_REPO_DIR" show main:a.rs
tf_assert "merge succeeds (ff)"        tf_worktree_merge T1
tf_assert "a.rs present on main post-merge" git -C "$TF_REPO_DIR" show main:a.rs >/dev/null
tf_group_end

tf_group_begin; tf_test "remove worktree + delete branch"
tf_worktree_remove T1
tf_assert_not "worktree dir gone"      test -d "$WT"
tf_worktree_delete_branch T1
tf_assert_not "branch deleted"         git -C "$TF_REPO_DIR" rev-parse --verify --quiet agent/T1
tf_group_end

tf_group_begin; tf_test "re-create worktree resets stale branch to base (retry path)"
# Simulate a prior failed attempt leaving a branch based on old main
git -C "$TF_REPO_DIR" worktree add -q -b agent/T2 "$TF_WORKTREE_ROOT/T2" main
echo "v1" > "$TF_WORKTREE_ROOT/T2/stale.rs"
git -C "$TF_WORKTREE_ROOT/T2" add -A && git -C "$TF_WORKTREE_ROOT/T2" commit -qm "attempt1"
tf_worktree_remove T2
# now branch exists but worktree doesn't — create should RESET it to base so the
# retry starts fresh from current main (stale state would re-conflict on merge)
WT2="$(tf_worktree_create T2)"
tf_assert "worktree re-created on existing branch" test -d "$WT2"
tf_assert "stale commit discarded (fresh retry from base)" test ! -f "$WT2/stale.rs"
tf_worktree_remove T2
tf_worktree_delete_branch T2
tf_group_end

tf_test_summary
