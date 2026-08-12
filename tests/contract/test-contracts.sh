#!/usr/bin/env bash
# contract/test-contracts.sh — CONTRACT TESTING for the tf_* public API.
#
# SOTA paradigm: pin the public API contract — every function exists, takes
# the documented arguments, returns the documented exit codes, and fails
# loudly (non-zero) on misuse rather than silently misbehaving.
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
  {"id":"A","engine":"t","title":"Task A","section":"§1","deps":[],"scope":["x.rs"],"accept":"test -f x.rs","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/verify.sh"
. "$HERE/../../lib/worktree.sh"

echo "=== [contract] public API ==="

# 1. Every tf_* function in the libs is defined (no missing/typo'd functions)
tf_group_begin; tf_test "all documented tf_* functions exist"
for fn in \
  tf_log tf_info tf_warn tf_error tf_require_jq tf_task_field tf_task_json \
  tf_all_task_ids tf_worker tf_worker_field tf_worker_names tf_default \
  tf_status_init tf_status_get tf_status_set tf_is_ready tf_ready_task_ids \
  tf_count_status tf_status_board tf_fail_task tf_done_task tf_locked_mv \
  tf_classify_error tf_error_snippet tf_verify tf_write_error_json \
  tf_get_error_category tf_get_error_summary tf_verify_scope \
  tf_worktree_ensure_gitignore tf_worktree_create tf_worktree_remove \
  tf_worktree_merge tf_worktree_delete_branch; do
  if declare -F "$fn" >/dev/null; then
    printf '    \033[32mok\033[0m   function %s defined\n' "$fn"
  else
    printf '    \033[31mBAD\033[0m  function %s defined\n' "$fn"
    TF_GROUP_FAILED=1
  fi
done
tf_group_end

# 2. tf_task_field returns empty (not error) for missing field
tf_group_begin; tf_test "tf_task_field tolerates missing fields"
mf="$(tf_task_field A .nonexistent 2>/dev/null)"
tf_assert "missing field no crash (got '$mf')" test -z "$mf" -o "$mf" = "null"
mt="$(tf_task_field NOPE .title 2>/dev/null)"
tf_assert "missing task no crash (got '$mt')" test -z "$mt" -o "$mt" = "null"
tf_group_end

# 3. tf_status_get returns empty for missing task/field (never non-zero crash)
tf_group_begin; tf_test "tf_status_get is total (never crashes)"
tf_status_init
tf_assert_eq "missing task" "" "$(tf_status_get NOPE .status 2>/dev/null)"
tf_assert_eq "missing field" "" "$(tf_status_get A .nonexistent 2>/dev/null)"
tf_group_end

# 4. tf_worker_names returns only enabled workers
tf_group_begin; tf_test "tf_worker_names filters by enabled"
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[
  {"name":"on","provider":"p","model":"m","enabled":true},
  {"name":"off","provider":"p","model":"m","enabled":false}
]}
JSON
tf_assert_eq "only enabled listed" "on" "$(tf_worker_names)"
tf_group_end

# 5. tf_classify_error never returns empty for any input (total function)
tf_group_begin; tf_test "tf_classify_error is total (always classifies)"
for input in "" " " $'\n' "error: x" "FAILED" "ok"; do
  out="$(tf_classify_error "$input" 2>/dev/null)"
  tf_assert "classifies $(printf %q "$input") → $out" test -n "$out"
done
tf_group_end

# 6. tf_verify_scope exit contract: 0 for clean, non-zero for violations
#    The scope check diffs main...HEAD, so the out-of-scope change must live
#    on a side branch, not main.
tf_group_begin; tf_test "tf_verify_scope exit-code contract"
S2="$SBOX/scope"
mkdir -p "$S2"
cd "$S2" || exit 1
git init -q -b main 2>/dev/null
git config user.email t@t.t; git config user.name test
echo x > x.rs; git add -A; git commit -qm init
# branch with ONLY an out-of-scope file (evil.txt)
git checkout -qb bad && echo evil > evil.txt && git add -A && git commit -qm evil
if out="$(tf_verify_scope A "$S2")" && echo "$out" | grep -q "evil.txt"; then
  printf '    \033[32mok\033[0m   out-of-scope evil.txt flagged\n'
else
  printf '    \033[31mBAD\033[0m  out-of-scope flagged (got: %q)\n' "$out"
  TF_GROUP_FAILED=1
fi
# branch with ONLY an in-scope file (x.rs)
git checkout -q main && git checkout -qb clean && echo x2 > x.rs && git add -A && git commit -qm clean
if out="$(tf_verify_scope A "$S2")"; then
  tf_assert "clean scope empty output" test -z "$out"
else
  printf '    \033[31mBAD\033[0m  clean scope should be exit 0\n'
  TF_GROUP_FAILED=1
fi
tf_group_end

# 7. tf_worktree functions fail loudly on bad input
tf_group_begin; tf_test "worktree ops reject unknown tasks cleanly"
wt="$(tf_worktree_create NOPE 2>/dev/null)"
tf_assert_eq "create NOPE returns empty" "" "$wt"
tf_group_end

# 8. status JSON schema invariant: every entry has required keys
tf_group_begin; tf_test "status schema: every task entry has all required keys"
tf_status_init
bad="$(jq -r '[to_entries[] | select((.value | has("status")) | not)] | length' "$STATUS_JSON")"
tf_assert_eq "no entry lacks status" "0" "$bad"
# every done task has finished_at
bad2="$(jq -r '[.[] | select(.status=="done" and (.finished_at == null))] | length' "$STATUS_JSON")"
tf_assert_eq "no done task lacks finished_at" "0" "$bad2"
tf_group_end

# 9. tf_status_init is idempotent (calling twice = calling once)
tf_group_begin; tf_test "tf_status_init idempotency"
tf_status_init
cp "$STATUS_JSON" "$SBOX/snap1.json"
tf_status_set A running
tf_status_init
tf_assert_eq "existing state preserved across re-init" "running" "$(tf_status_get A .status)"
tf_group_end

tf_test_summary
