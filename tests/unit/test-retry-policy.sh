#!/usr/bin/env bash
# unit/test-retry-policy.sh — unit tests for error-category-aware retry policies.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5,
             "no_op_max_attempts":2,"timeout_retry_multiplier":1.5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"

echo "=== [unit] retry policies ==="

# ---- no_op early termination ----
tf_group_begin; tf_test "no_op permanently fails after no_op_max_attempts (default 2)"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"N1","engine":"t","title":"N1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/noop-status.json"
tf_status_init

# First no_op
echo '{"category":"no_op","summary":"no changes","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/N1.error.json"
tf_fail_task N1 "no changes detected"
tf_assert_eq "attempt 1" "1" "$(tf_status_get N1 .attempts)"
tf_assert_eq "still retryable" "failed" "$(tf_status_get N1 .status)"
tf_assert "has next_retry_at" test -n "$(tf_status_get N1 .next_retry_at)"

# Wait for cooldown, then second no_op
sleep 2
tf_is_ready N1 >/dev/null 2>&1  # consume cooldown→ready
echo '{"category":"no_op","summary":"no changes again","classified_at":"2026-01-01T00:00:01Z"}' > "$TF_LOG_DIR/N1.error.json"
tf_fail_task N1 "no changes detected again"
tf_assert_eq "attempt 2" "2" "$(tf_status_get N1 .attempts)"
tf_assert_eq "permanently failed" "failed" "$(tf_status_get N1 .status)"
tf_assert "no next_retry_at (permanent)" test -z "$(tf_status_get N1 .next_retry_at)"
tf_assert "has finished_at" test -n "$(tf_status_get N1 .finished_at)"
tf_group_end

tf_group_begin; tf_test "non-no_op failures still retry normally after 2 no_ops"
# Verify that a compile_error on attempt 1 is handled normally (not cut short)
STATUS_JSON="$SBOX/state/nonnoop-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"C1","engine":"t","title":"C1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
echo '{"category":"compile_error","summary":"2 errors","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/C1.error.json"
tf_fail_task C1 "compile failed"
tf_assert_eq "attempt 1" "1" "$(tf_status_get C1 .attempts)"
tf_assert "has next_retry_at" test -n "$(tf_status_get C1 .next_retry_at)"
# Compile errors should NOT get permanent-failed at attempt 1
tf_assert_contains "last_error mentions compile" "[compile_error]" "$(tf_status_get C1 .last_error)"
tf_group_end

# ---- timeout multiplier ----
tf_group_begin; tf_test "timeout failure stores timeout_multiplier in status"
STATUS_JSON="$SBOX/state/timeout-mult-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"T1","engine":"t","title":"T1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
echo '{"category":"timeout","summary":"exceeded time limit","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/T1.error.json"
tf_fail_task T1 "dispatch timed out"
tf_assert "has timeout_multiplier" test -n "$(tf_status_get T1 .timeout_multiplier)"
local_mult="$(tf_status_get T1 .timeout_multiplier)"
# Should be 1.5 (default multiplier from config)
tf_assert "multiplier > 1.0 (got $local_mult)" test "$(echo "$local_mult > 1.0" | bc -l)" = "1"
tf_group_end

tf_group_begin; tf_test "timeout multiplier compounds on consecutive timeouts"
STATUS_JSON="$SBOX/state/timeout-compound-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"T2","engine":"t","title":"T2","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
echo '{"category":"timeout","summary":"exceeded time limit","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/T2.error.json"
tf_fail_task T2 "dispatch timed out"
mult1="$(tf_status_get T2 .timeout_multiplier)"
sleep 2
tf_is_ready T2 >/dev/null 2>&1
echo '{"category":"timeout","summary":"exceeded time limit again","classified_at":"2026-01-01T00:00:01Z"}' > "$TF_LOG_DIR/T2.error.json"
tf_fail_task T2 "dispatch timed out again"
mult2="$(tf_status_get T2 .timeout_multiplier)"
# mult2 should be 1.5 * 1.5 = 2.25
compounded="$(echo "$mult2 > $mult1" | bc -l 2>/dev/null || echo 0)"
tf_assert_gt "multiplier compounds" "0" "$compounded"
tf_group_end

# ---- non-timeout failures don't set multiplier ----
tf_group_begin; tf_test "non-timeout failures leave timeout_multiplier unchanged"
STATUS_JSON="$SBOX/state/nontimeout-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"NT1","engine":"t","title":"NT1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
echo '{"category":"test_failure","summary":"3 tests failed","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/NT1.error.json"
tf_fail_task NT1 "tests failed"
# Should not have a timeout_multiplier (or it should be null)
no_mult="$(tf_status_get NT1 .timeout_multiplier)"
tf_assert_eq "no timeout_multiplier set" "" "$no_mult"
tf_group_end

# ---- Edge: no_op_max_attempts configurable ----
tf_group_begin; tf_test "no_op_max_attempts=1 permanently fails on first no_op"
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5,
             "no_op_max_attempts":1,"timeout_retry_multiplier":1.5}}
JSON
STATUS_JSON="$SBOX/state/noop1-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"N2","engine":"t","title":"N2","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
echo '{"category":"no_op","summary":"no changes","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/N2.error.json"
tf_fail_task N2 "no changes"
tf_assert_eq "permanently failed at attempt 1" "failed" "$(tf_status_get N2 .status)"
tf_assert "no retry scheduled" test -z "$(tf_status_get N2 .next_retry_at)"
tf_group_end

# ---- Property: status JSON stays valid after category-aware retry ----
tf_group_begin; tf_test "property: status JSON valid after mixed failure sequence"
test_mixed_failures_valid() {
  STATUS_JSON="$SBOX/state/prop-mixed-$$.json"
  cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"M1","engine":"t","title":"M1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
  {"id":"M2","engine":"t","title":"M2","section":"§1","deps":[],"scope":["b.rs"],"accept":"true","manual":false}
]}
JSON
  # Restore default config
  cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5,
             "no_op_max_attempts":2,"timeout_retry_multiplier":1.5}}
JSON
  tf_status_init
  # no_op
  echo '{"category":"no_op","summary":"nope","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/M1.error.json"
  tf_fail_task M1 "noop"
  # timeout
  echo '{"category":"timeout","summary":"slow","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/M2.error.json"
  tf_fail_task M2 "timeout"
  # compile
  echo '{"category":"compile_error","summary":"err","classified_at":"2026-01-01T00:00:00Z"}' > "$TF_LOG_DIR/M2.error.json"
  sleep 2
  tf_is_ready M2 >/dev/null 2>&1
  tf_fail_task M2 "compile"
  jq -e . "$STATUS_JSON" >/dev/null 2>&1
}
tf_property "status_valid_after_mixed_failures" test_mixed_failures_valid 5
tf_group_end

tf_test_summary
