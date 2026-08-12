#!/usr/bin/env bash
# unit/test-status.sh — unit tests for lib/status.sh
# Comprehensive: state machine transitions, edge cases, concurrency, idempotency.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"
export TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"

echo "=== [unit] status.sh ==="

# ---- Init & idempotency ----
tf_group_begin; tf_test "tf_status_init creates entries for all tasks from tasks.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false},
  {"id":"B","engine":"t","title":"B","section":"§1","deps":[],"scope":["y"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_assert_eq "count" "2" "$(jq 'length' "$STATUS_JSON")"
tf_assert_eq "A status" "ready" "$(tf_status_get A .status)"
tf_assert_eq "A attempts" "0" "$(tf_status_get A .attempts)"
tf_assert_eq "B status" "ready" "$(tf_status_get B .status)"
tf_group_end

tf_group_begin; tf_test "tf_status_init is idempotent (preserves existing state)"
tf_status_set A running
tf_status_init
tf_assert_eq "A preserved as running" "running" "$(tf_status_get A .status)"
tf_group_end

tf_group_begin; tf_test "tf_status_init adds new tasks to existing state"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false},
  {"id":"C","engine":"t","title":"C","section":"§1","deps":[],"scope":["z"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_assert_eq "C added" "ready" "$(tf_status_get C .status)"
tf_assert_eq "A still running" "running" "$(tf_status_get A .status)"
tf_group_end

# ---- State transitions ----
tf_group_begin; tf_test "ready → running → done lifecycle"
tf_status_set A running
tf_assert_eq "A running" "running" "$(tf_status_get A .status)"
tf_assert "A has started_at" test -n "$(tf_status_get A .started_at)"
tf_done_task A
tf_assert_eq "A done" "done" "$(tf_status_get A .status)"
tf_assert "A has finished_at" test -n "$(tf_status_get A .finished_at)"
tf_assert "A worker cleared" test -z "$(tf_status_get A .worker)"
tf_group_end

tf_group_begin; tf_test "ready → running → failed lifecycle"
tf_status_set C running
tf_fail_task C "test failure"
tf_assert_eq "C failed" "failed" "$(tf_status_get C .status)"
tf_assert_eq "C attempts" "1" "$(tf_status_get C .attempts)"
tf_assert "C has last_error" test -n "$(tf_status_get C .last_error)"
tf_assert "C has next_retry_at" test -n "$(tf_status_get C .next_retry_at)"
tf_group_end

tf_group_begin; tf_test "failed → ready after cooldown"
# next_retry_at is 1s in the future; wait for it
sleep 2
tf_is_ready C
tf_assert_eq "C ready after cooldown" "ready" "$(tf_status_get C .status)"
tf_group_end

# ---- Dependency resolution ----
tf_group_begin; tf_test "tf_is_ready respects deps (chain: A→B→C)"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"X1","engine":"t","title":"X1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"X2","engine":"t","title":"X2","section":"§1","deps":["X1"],"scope":["b"],"accept":"true","manual":false},
  {"id":"X3","engine":"t","title":"X3","section":"§1","deps":["X1","X2"],"scope":["c"],"accept":"true","manual":false},
  {"id":"X4","engine":"t","title":"X4","section":"§1","deps":[],"scope":["d"],"accept":"true","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/chain-status.json"
tf_status_init

tf_assert "X1 ready (no deps)" tf_is_ready X1
tf_assert_not "X2 NOT ready (needs X1)" tf_is_ready X2
tf_assert_not "X3 NOT ready (needs X1+X2)" tf_is_ready X3
tf_assert "X4 ready (no deps)" tf_is_ready X4

tf_done_task X1
tf_assert "X2 ready after X1 done" tf_is_ready X2
tf_assert_not "X3 NOT ready (still needs X2)" tf_is_ready X3

tf_done_task X2
tf_assert "X3 ready after X1+X2 done" tf_is_ready X3
tf_done_task X3

tf_done_task X4; tf_assert_eq "all done count" "4" "$(tf_count_status done)"
tf_group_end

# ---- Retry behavior ----
tf_group_begin; tf_test "fail increments attempts, schedules retry"
STATUS_JSON="$SBOX/state/retry-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"R1","engine":"t","title":"R1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_fail_task R1 "first fail"
tf_assert_eq "attempts=1" "1" "$(tf_status_get R1 .attempts)"
tf_assert "has next_retry_at" test -n "$(tf_status_get R1 .next_retry_at)"
tf_assert_eq "status=failed" "failed" "$(tf_status_get R1 .status)"
tf_group_end

tf_group_begin; tf_test "fail 3 times → permanently failed (max_attempts=3)"
tf_fail_task R1 "second fail"
tf_assert_eq "attempts=2" "2" "$(tf_status_get R1 .attempts)"
sleep 2
tf_is_ready R1 >/dev/null 2>&1 # consume cooldown→ready
tf_fail_task R1 "third fail"
tf_assert_eq "attempts=3" "3" "$(tf_status_get R1 .attempts)"
tf_assert_eq "permanently failed" "failed" "$(tf_status_get R1 .status)"
tf_assert "no next_retry" test -z "$(tf_status_get R1 .next_retry_at)"
tf_assert_not "never ready again" tf_is_ready R1
tf_group_end

# ---- Counting ----
tf_group_begin; tf_test "tf_count_status tallies by status"
STATUS_JSON="$SBOX/state/count-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"C1","engine":"t","title":"C1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"C2","engine":"t","title":"C2","section":"§1","deps":[],"scope":["b"],"accept":"true","manual":false},
  {"id":"C3","engine":"t","title":"C3","section":"§1","deps":[],"scope":["c"],"accept":"true","manual":false},
  {"id":"C4","engine":"t","title":"C4","section":"§1","deps":[],"scope":["d"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_done_task C1
tf_done_task C2
tf_fail_task C3 "fail"
# C4 stays ready
tf_assert_eq "done=2" "2" "$(tf_count_status done)"
tf_assert_eq "failed=1" "1" "$(tf_count_status failed)"
tf_assert_eq "ready=1" "1" "$(tf_count_status ready)"
tf_group_end

# ---- Ready task listing ----
tf_group_begin; tf_test "tf_ready_task_ids lists only dispatchable tasks"
STATUS_JSON="$SBOX/state/ready-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"Y1","engine":"t","title":"Y1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"Y2","engine":"t","title":"Y2","section":"§1","deps":["Y1"],"scope":["b"],"accept":"true","manual":false},
  {"id":"Y3","engine":"t","title":"Y3","section":"§1","deps":[],"scope":["c"],"accept":"true","manual":false}
]}
JSON
tf_status_init
ready="$(tf_ready_task_ids)"
tf_assert "Y1 in ready set" echo "$ready" | grep -qxF Y1
tf_assert_not "Y2 NOT in ready set" echo "$ready" | grep -qxF Y2
tf_assert "Y3 in ready set" echo "$ready" | grep -qxF Y3
tf_done_task Y1
ready="$(tf_ready_task_ids)"
tf_assert "Y2 now in ready set" echo "$ready" | grep -qxF Y2
tf_group_end

# ---- Status board ----
tf_group_begin; tf_test "tf_status_board prints without error"
STATUS_JSON="$SBOX/state/board-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"Z1","engine":"t","title":"Board task","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false}
]}
JSON
tf_status_init
out="$(tf_status_board 2>&1)"
tf_assert "board has header" echo "$out" | grep -q "TASK"
tf_assert "board has task row" echo "$out" | grep -q "Z1"
tf_group_end

# ---- Error classification in fail_task ----
tf_group_begin; tf_test "tf_fail_task enriches error with classification when error.json exists"
STATUS_JSON="$SBOX/state/err-enrich.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"E1","engine":"t","title":"E1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false}
]}
JSON
tf_status_init
# Write a fake error classification
mkdir -p "$TF_LOG_DIR"
jq -n '{category:"compile_error",summary:"2 error(s)",classified_at:"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/E1.error.json"
tf_fail_task E1 "acceptance gate: FAIL"
tf_assert_contains "last_error has category" "[compile_error]" "$(tf_status_get E1 .last_error)"
tf_assert_eq "error_category field" "compile_error" "$(tf_status_get E1 .error_category)"
tf_group_end

# ---- Edge case: empty tasks.json ----
tf_group_begin; tf_test "tf_status_init with empty task list"
STATUS_JSON="$SBOX/state/empty-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[]}
JSON
tf_status_init
tf_assert_eq "count=0" "0" "$(jq 'length' "$STATUS_JSON")"
tf_group_end

# ---- Edge case: single task with self-dependency (invalid but shouldn't crash) ----
tf_group_begin; tf_test "task with self-dependency is never ready"
STATUS_JSON="$SBOX/state/self-dep-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"SELF","engine":"t","title":"Self","section":"§1","deps":["SELF"],"scope":["a"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_assert_not "self-dep task never ready" tf_is_ready SELF
tf_group_end

# ---- Property: status JSON is always valid JSON after any sequence of operations ----
tf_group_begin; tf_test "property: status JSON remains valid after random operations"
test_status_json_valid() {
  STATUS_JSON="$SBOX/state/prop-status-$$.json"
  cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"P1","engine":"t","title":"P1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"P2","engine":"t","title":"P2","section":"§1","deps":[],"scope":["b"],"accept":"true","manual":false}
]}
JSON
  tf_status_init
  # Random sequence of operations
  for i in 1 2 3 4 5; do
    local task="P$((RANDOM % 2 + 1))"
    case $((RANDOM % 3)) in
      0) tf_done_task "$task" 2>/dev/null ;;
      1) tf_fail_task "$task" "synthetic" 2>/dev/null ;;
      2) tf_status_set "$task" running 2>/dev/null ;;
    esac
  done
  jq -e . "$STATUS_JSON" >/dev/null 2>&1
}
tf_property "status_json_always_valid" test_status_json_valid 5
tf_group_end

tf_test_summary
