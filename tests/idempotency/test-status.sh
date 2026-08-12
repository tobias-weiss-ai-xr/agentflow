#!/usr/bin/env bash
# idempotency/test-status.sh — IDEMPOTENCY TESTING.
#
# SOTA paradigm: run every operation twice (and N times) and verify the
# result is identical — the system must be safe to re-run after crashes,
# retries, and interrupted orchestrator cycles.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config" TF_STATE_DIR="$SBOX/state" TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo" TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_REPO_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false},
  {"id":"B","engine":"t","title":"B","section":"§1","deps":["A"],"scope":["y"],"accept":"true","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/worktree.sh"

echo "=== [idempotency] status + worktree ==="
tf_seed_init

# 1. tf_done_task twice → same final state
tf_group_begin; tf_test "tf_done_task is idempotent"
STATUS_JSON="$SBOX/state/idem1.json"
tf_status_init
tf_status_set A running
tf_done_task A
a1="$(cat "$STATUS_JSON")"
tf_done_task A
tf_assert_eq "done twice = done once" "$a1" "$(cat "$STATUS_JSON")"
tf_group_end

# 2. tf_fail_task records each failure event (attempts++) — NOT idempotent by
#    design; it is the retry counter. The idempotency contract is: repeated
#    failure stays a consistent 'failed' state with valid JSON and monotonic
#    attempts, and a permanently-failed task stays failed.
tf_group_begin; tf_test "tf_fail_task is a retry counter: consistent + monotonic"
STATUS_JSON="$SBOX/state/idem2.json"
tf_status_init
tf_fail_task B "first"
tf_fail_task B "second"
tf_assert_eq "attempts monotonically 2" "2" "$(tf_status_get B .attempts)"
tf_assert_eq "status stays failed" "failed" "$(tf_status_get B .status)"
tf_assert "last_error updated" test -n "$(tf_status_get B .last_error)"
tf_assert_valid_json "ledger valid after double fail" "$STATUS_JSON"
tf_group_end

# 3. tf_status_set same value repeatedly → stable
tf_group_begin; tf_test "tf_status_set is idempotent"
STATUS_JSON="$SBOX/state/idem3.json"
tf_status_init
tf_status_set A running
r1="$(cat "$STATUS_JSON")"
tf_status_set A running
tf_status_set A running
tf_assert_eq "set ×3 = set ×1" "$r1" "$(cat "$STATUS_JSON")"
tf_group_end

# 4. status_init after done → done stays done (no regression to ready)
tf_group_begin; tf_test "tf_status_init never downgrades done→ready"
STATUS_JSON="$SBOX/state/idem4.json"
tf_status_init
tf_done_task A
tf_status_init
tf_assert_eq "A stays done" "done" "$(tf_status_get A .status)"
tf_group_end

# 5. worktree create/remove/create is safe (re-create after removal)
tf_group_begin; tf_test "worktree create→remove→create cycle is idempotent"
cd "$TF_REPO_DIR" || exit 1
git init -q -b main 2>/dev/null
git config user.email t@t.t; git config user.name test
echo base > README.md; git add -A; git commit -qm init
export TF_WORKTREE_ROOT="$TF_REPO_DIR/.tf-worktrees"
export TF_MERGE_LOCK="$TF_STATE_DIR/merge.lock"
w1="$(tf_worktree_create A)"
tf_worktree_remove A
w2="$(tf_worktree_create A)"
tf_assert "re-created worktree exists" test -d "$w2"
tf_assert_eq "same worktree path" "$w1" "$w2"
tf_group_end

# 6. worktree_remove on missing worktree → still exit 0 (safe)
tf_group_begin; tf_test "tf_worktree_remove is idempotent on missing"
tf_worktree_remove A
tf_assert "second remove no error" tf_worktree_remove A
tf_group_end

# 7. 10× re-init → status file byte-identical each time
tf_group_begin; tf_test "repeated init produces byte-identical ledger"
STATUS_JSON="$SBOX/state/idem7.json"
tf_status_init
cp "$STATUS_JSON" "$SBOX/golden-ledger.json"
same=1
for i in 1 2 3 4 5 6 7 8 9 10; do
  tf_status_init >/dev/null 2>&1
  cmp -s "$SBOX/golden-ledger.json" "$STATUS_JSON" || same=0
done
tf_assert "10 inits identical" test "$same" -eq 1
tf_group_end

tf_test_summary
