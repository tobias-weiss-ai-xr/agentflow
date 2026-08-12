#!/usr/bin/env bash
# table/test-status.sh — TABLE-DRIVEN TESTING of the status state machine.
#
# SOTA paradigm: enumerate every state transition as a data table
# (start-state, operation, expected-end-state, expected-effects) and verify
# each row. A table makes the state machine's behavior auditable at a glance.
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
  {"id":"T","engine":"t","title":"T","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"

echo "=== [table] status state machine ==="
tf_seed_init

# Each row: "label,start,op,end"
#   start/end: ready|running|done|failed
#   op: set_running|done|fail|reinit|cooldown
run_row() {
  local id="$1" start="$2" op="$3" end="$4"
  STATUS_JSON="$SBOX/state/table-$id.json"
  mkdir -p "$SBOX/state"
  tf_status_init >/dev/null 2>&1
  # force start state
  tf_status_set T "$start" >/dev/null 2>&1
  case "$op" in
    set_running) tf_status_set T running >/dev/null 2>&1 ;;
    done) tf_done_task T >/dev/null 2>&1 ;;
    fail) tf_fail_task T "table-fail" >/dev/null 2>&1 ;;
    reinit) tf_status_init >/dev/null 2>&1 ;;
    cooldown) : ;;
  esac
  local got
  got="$(tf_status_get T .status 2>/dev/null)"
  [[ "$got" == "$end" ]]
}

tf_group_begin; tf_test "state transition table"
tf_table_test "transition" run_row \
  'ready-to-running,ready,set_running,running' \
  'ready-to-done,ready,done,done' \
  'ready-to-failed,ready,fail,failed' \
  'running-to-done,running,done,done' \
  'running-to-failed,running,fail,failed' \
  'done-stays-done-on-done,done,done,done' \
  'failed-stays-failed-on-fail,failed,fail,failed' \
  'reinit-keeps-state,running,reinit,running' \
  'reinit-keeps-state2,failed,reinit,failed'
tf_group_end

# Effect table: each op must produce the documented side effects
effect_row() {
  local id="$1" op="$2" field="$3" expect="$4"
  STATUS_JSON="$SBOX/state/effect-$id.json"
  mkdir -p "$SBOX/state"
  tf_status_init >/dev/null 2>&1
  tf_status_set T running >/dev/null 2>&1
  case "$op" in
    done) tf_done_task T >/dev/null 2>&1 ;;
    fail) tf_fail_task T "eff" >/dev/null 2>&1 ;;
  esac
  local got
  got="$(tf_status_get T "$field" 2>/dev/null)"
  case "$expect" in
    NONEMPTY) [[ -n "$got" ]] ;;
    EMPTY) [[ -z "$got" ]] ;;
    *) [[ "$got" == "$expect" ]] ;;
  esac
}

tf_group_begin; tf_test "side-effect table"
tf_table_test "effect" effect_row \
  'done-clears-worker,done,.worker,EMPTY' \
  'done-clears-error,done,.last_error,EMPTY' \
  'done-clears-retry,done,.next_retry_at,EMPTY' \
  'done-sets-finished,done,.finished_at,NONEMPTY' \
  'fail-sets-error,fail,.last_error,NONEMPTY' \
  'fail-sets-retry,fail,.next_retry_at,NONEMPTY' \
  'fail-increments-attempts,fail,.attempts,1'
tf_group_end

# Counting table: after a scripted sequence, counts must match exactly
count_row() {
  local id="$1" op="$2" expect_done="$3" expect_failed="$4" expect_ready="$5"
  STATUS_JSON="$SBOX/state/count-$id.json"
  mkdir -p "$SBOX/state"
  tf_status_init >/dev/null 2>&1
  case "$op" in
    done) tf_done_task T >/dev/null 2>&1 ;;
    fail) tf_fail_task T "count" >/dev/null 2>&1 ;;
  esac
  tf_status_init >/dev/null 2>&1  # re-init must not change counts
  local d f r
  d="$(tf_count_status done 2>/dev/null)"
  f="$(tf_count_status failed 2>/dev/null)"
  r="$(tf_count_status ready 2>/dev/null)"
  [[ "$d" == "$expect_done" && "$f" == "$expect_failed" && "$r" == "$expect_ready" ]]
}

tf_group_begin; tf_test "counting table after sequences"
tf_table_test "count" count_row \
  'single-done,done,1,0,0' \
  'single-fail,fail,0,1,0'
tf_group_end

tf_test_summary
