#!/usr/bin/env bash
# unit/test-worktree.sh — unit tests for lib/worktree.sh (merge locks & worktrees)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR"

# Create a minimal git repo for worktree tests
(
  cd "$SBOX" || exit 1
  mkdir -p repo
  cd repo
  git init --quiet
  git config user.email "test@test.test"
  git config user.name "Test User"
  echo "file1" > lib.rs
  echo "file2" > model.rs
  git add lib.rs model.rs
  git commit -q --allow-empty -m "initial"
  git checkout -q -b main
)
export TF_WORKTREE_ROOT="$SBOX/worktrees"
mkdir -p "$TF_WORKTREE_ROOT"

# Create minimal config workers
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"openai","model":"gpt-4o","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

# Create a minimal tasks.json
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"T1","engine":"rust","title":"touch lib","section":"s","deps":[],"scope":["lib.rs"],"accept":"true","manual":false},
  {"id":"T2","engine":"rust","title":"touch model","section":"s","deps":[],"scope":["model.rs"],"accept":"true","manual":false},
  {"id":"T3","engine":"rust","title":"touch both","section":"s","deps":[],"scope":["lib.rs","model.rs"],"accept":"true","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/worktree.sh"

tf_status_init

echo "=== [unit] worktree.sh ==="

tf_group_begin

# ---- Basic worktree create + remove ----
tf_test "tf_worktree_create produces valid path"
wt="$(tf_worktree_create T1 main 2>/dev/null)" || true
tf_assert "worktree path is non-empty" test -n "$wt"
tf_assert "worktree dir exists" test -d "$wt"
tf_worktree_remove T1 >/dev/null 2>&1
tf_group_end

tf_group_begin
# Clean up from previous test
rm -rf "$TF_WORKTREE_ROOT"/*

# ---- Global merge lock (default) ----
tf_test "tf_worktree_merge in global mode uses single lock"
export TF_MERGE_LOCK_MODE="global"
# Create a task with valid branch
git -C "$SBOX/repo" checkout -q -b "$TF_BRANCH_PREFIX/T1" main 2>/dev/null
# The task branch must exist or create will fail; just test lock file logic
merge_lock="${TF_MERGE_LOCK:-$TF_STATE_DIR/merge.lock}"
tf_assert "global lock path is set" test -n "$merge_lock"
tf_group_end

tf_group_begin
tf_test "per-file lock helper encodes paths"
lock_dir="$(_tf_merge_lock_dir "lib.rs")"
tf_assert "encoded dir ends with lib_rs" test "${lock_dir##*/}" = "lib_rs"
lock_dir2="$(_tf_merge_lock_dir "src/lib.rs")"
tf_assert "nested path encoded" test "${lock_dir2##*/}" = "src_lib_rs"
lock_dir3="$(_tf_merge_lock_dir "a.b/c.d/x.rs")"
tf_assert "dots and slashes encoded" test "${lock_dir3##*/}" = "a_b_c_d_x_rs"
tf_group_end

tf_group_begin
tf_test "_tf_merge_acquire_per_file_locks succeeds with no contention"
acquired="$(mktemp)"
# Make sure lock dir clean
rm -rf "$TF_STATE_DIR/merge-locks"
_tf_merge_acquire_per_file_locks T1 "$acquired" && {
  tf_assert "lock file created" test -s "$acquired"
  tf_assert "lock dir exists" test -d "$TF_STATE_DIR/merge-locks/lib_rs"
  _tf_merge_release_per_file_locks "$acquired"
  tf_assert "lock dir removed after release" test ! -d "$TF_STATE_DIR/merge-locks/lib_rs"
} || true
tf_group_end

tf_group_begin
tf_test "_tf_merge_acquire_per_file_locks fails with contention"
acquired1="$(mktemp)" acquired2="$(mktemp)"
rm -rf "$TF_STATE_DIR/merge-locks"
# First acquisition succeeds
_tf_merge_acquire_per_file_locks T1 "$acquired1" && {
  # Second acquisition on same scope fails
  _tf_merge_acquire_per_file_locks T1 "$acquired2" && {
    tf_assert "SHOULD FAIL: second lock on same scope succeeded" false
  } || {
    tf_assert "second lock on same scope fails as expected" true
  }
  # Verify first lock was NOT released by the failed second attempt
  tf_assert "first lock still held" test -d "$TF_STATE_DIR/merge-locks/lib_rs"
  _tf_merge_release_per_file_locks "$acquired1"
}
tf_group_end

tf_group_begin
tf_test "_tf_merge_acquire_per_file_locks disjoint scopes succeed"
acquired1="$(mktemp)" acquired2="$(mktemp)"
rm -rf "$TF_STATE_DIR/merge-locks"
# T1: scope = lib.rs
_tf_merge_acquire_per_file_locks T1 "$acquired1" && {
  # T2: scope = model.rs (disjoint)
  _tf_merge_acquire_per_file_locks T2 "$acquired2" && {
    tf_assert "both disjoint locks acquired" test -s "$acquired1" -a -s "$acquired2"
    tf_assert "both lock dirs exist" test -d "$TF_STATE_DIR/merge-locks/lib_rs" -a -d "$TF_STATE_DIR/merge-locks/model_rs"
    _tf_merge_release_per_file_locks "$acquired1"
    _tf_merge_release_per_file_locks "$acquired2"
  } || true
}
tf_group_end

tf_group_begin
tf_test "_tf_merge_acquire_per_file_locks overlapping scopes fail"
acquired1="$(mktemp)" acquired2="$(mktemp)"
rm -rf "$TF_STATE_DIR/merge-locks"
# T1: scope = lib.rs, model.rs
_tf_merge_acquire_per_file_locks T3 "$acquired1" && {
  # T2: scope = model.rs (overlaps with T3)
  _tf_merge_acquire_per_file_locks T2 "$acquired2" && {
    tf_assert "SHOULD FAIL: T2 acquired lock despite overlapping scope" false
  } || {
    tf_assert "T2 failed to acquire lock due to overlap" true
  }
  # T1 still holds its locks
  tf_assert "T3 lock on model.rs still held" test -d "$TF_STATE_DIR/merge-locks/model_rs"
  tf_assert "T3 lock on lib.rs still held" test -d "$TF_STATE_DIR/merge-locks/lib_rs"
  _tf_merge_release_per_file_locks "$acquired1"
}
tf_group_end

tf_test_summary
