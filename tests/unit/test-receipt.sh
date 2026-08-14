#!/usr/bin/env bash
# unit/test-receipt.sh — unit tests for lib/receipt.sh
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
export TF_RECEIPT_DIR="$SBOX/receipts"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"openai","model":"gpt-4o","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/receipt.sh"

# Helper: reset all state for isolation
tf_reset() {
  STATUS_JSON="$SBOX/state/reset-$$.json"
  rm -f "$TF_RECEIPT_DIR"/*.ndjson
  cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"R1","engine":"t","title":"Receipt test","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
  tf_status_init
}

echo "=== [unit] receipt.sh ==="

# ---- Receipt file path ----
tf_group_begin; tf_test "tf_receipt_file returns correct path"
rfile="$(tf_receipt_file 2025-01-15)"
tf_assert "receipt file in correct dir" echo "$rfile" | grep -q "receipts/2025-01-15.ndjson"
tf_group_end

# ---- Receipt writing ----
tf_group_begin; tf_test "tf_receipt_begin writes valid NDJSON"
tf_reset
receipt_file="$(tf_receipt_file)"
tf_receipt_begin R1 w1 openai gpt-4o tf/R1
tf_assert "receipt file exists and is non-empty" test -s "$receipt_file"
tf_assert_valid_json "first receipt is valid JSON" "$receipt_file"
tf_group_end

tf_group_begin; tf_test "receipt begin has correct fields"
tf_reset
receipt_file="$(tf_receipt_file)"
tf_receipt_begin R1 w1 openai gpt-4o tf/R1
first="$(head -1 "$receipt_file")"
tf_assert_eq "task_id" "R1" "$(echo "$first" | jq -r .task_id)"
tf_assert_eq "worker" "w1" "$(echo "$first" | jq -r .worker)"
tf_assert_eq "provider" "openai" "$(echo "$first" | jq -r .provider)"
tf_assert_eq "model" "gpt-4o" "$(echo "$first" | jq -r .model)"
tf_assert_eq "status" "running" "$(echo "$first" | jq -r .status)"
tf_assert "has dispatch_started_at" test -n "$(echo "$first" | jq -r .dispatch_started_at)"
tf_group_end

# ---- Token extraction ----
tf_group_begin; tf_test "tf_extract_tokens parses JSON usage block"
log="$SBOX/sample-tokens.log"
printf 'Some output\nusage: { "input_tokens": 1234, "output_tokens": 567 }\nmore output\n' > "$log"
tokens="$(tf_extract_tokens "$log")"
tf_assert_eq "parsed tokens" "1234 567" "$tokens"
tf_group_end

tf_group_begin; tf_test "tf_extract_tokens returns 0 0 for empty/missing log"
tokens="$(tf_extract_tokens "/nonexistent/path")"
tf_assert_eq "missing log" "0 0" "$tokens"
: > "$SBOX/empty.log"
tokens2="$(tf_extract_tokens "$SBOX/empty.log")"
tf_assert_eq "empty log" "0 0" "$tokens2"
tf_group_end

# ---- Cost calculation ----
tf_group_begin; tf_test "tf_cost_per_1k returns positive cost for gpt-4o"
cost="$(tf_cost_per_1k openai gpt-4o 1000 500)"
is_positive="$(echo "$cost > 0" | bc -l)"
tf_assert "cost is positive" test "$is_positive" = "1"
tf_group_end

tf_group_begin; tf_test "tf_cost_per_1k returns 0 for zero tokens"
cost="$(tf_cost_per_1k openai gpt-4o 0 0)"
is_zero="$(echo "$cost == 0" | bc -l)"
tf_assert "zero cost for zero tokens" test "$is_zero" = "1"
tf_group_end

# ---- Full lifecycle ----
tf_group_begin; tf_test "receipt lifecycle: begin → dispatch → gate → close"
tf_reset
receipt_file="$(tf_receipt_file)"

dispatch_log="$SBOX/r1.dispatch.log"
printf '=== R1 dispatch ===\nusage: { "input_tokens": 2000, "output_tokens": 800 }\n' > "$dispatch_log"

tf_receipt_begin R1 w1 openai gpt-4o tf/R1
tf_receipt_finish_dispatch R1 "$dispatch_log" 0
tf_receipt_finish_gate R1 "PASS" "cargo test -p wo-common"
tf_receipt_close R1 "done"

lines="$(wc -l < "$receipt_file")"
tf_assert_eq "4 receipt records written" "4" "$lines"

closing="$(jq -s '[.[] | select(.type == "closed")][0]' "$receipt_file")"
tf_assert_eq "final status is done" "done" "$(echo "$closing" | jq -r .final_status)"
tf_assert_gt "total_tokens_in > 0" "0" "$(echo "$closing" | jq -r .total_tokens_in)"
tf_group_end

# ---- Cost summary ----
tf_group_begin; tf_test "tf_cost_summary --task shows single task detail"
tf_reset
receipt_file="$(tf_receipt_file)"
dispatch_log="$SBOX/r1b.dispatch.log"
printf 'usage: { "input_tokens": 500, "output_tokens": 200 }\n' > "$dispatch_log"
tf_receipt_begin R1 w1 openai gpt-4o tf/R1
tf_receipt_finish_dispatch R1 "$dispatch_log" 0
tf_receipt_close R1 "done"

output="$(tf_cost_summary --task R1)"
tf_assert "output contains task id" echo "$output" | grep -q "R1"
tf_assert "output contains tokens" echo "$output" | grep -qi "token"
tf_group_end

# ---- Multiple tasks ----
tf_group_begin; tf_test "multiple task receipts are independent"
tf_reset
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"R1","engine":"t","title":"Task 1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
  {"id":"R2","engine":"t","title":"Task 2","section":"§1","deps":[],"scope":["b.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
receipt_file="$(tf_receipt_file)"

dispatch_log="$SBOX/r2.dispatch.log"
printf 'usage: { "input_tokens": 3000, "output_tokens": 1000 }\n' > "$dispatch_log"

tf_receipt_begin R1 w1 openai gpt-4o tf/R1
tf_receipt_finish_dispatch R1 "$dispatch_log" 0
tf_receipt_close R1 "done"

tf_receipt_begin R2 w1 openai gpt-4o tf/R2
tf_receipt_finish_dispatch R2 "$dispatch_log" 0
tf_receipt_close R2 "failed" "gate_failed"

r1_cost="$(tf_receipt_total R1 .cost_usd)"
r2_cost="$(tf_receipt_total R2 .cost_usd)"
tf_assert "R1 has cost" test "$(echo "$r1_cost > 0" | bc -l)" = "1"
tf_assert "R2 has cost" test "$(echo "$r2_cost > 0" | bc -l)" = "1"
tf_group_end

# ---- Property: receipt file is always valid NDJSON ----
tf_group_begin; tf_test "property: receipt file always valid NDJSON after operations"
test_receipt_ndjson_valid() {
  local receipt_file="$SBOX/receipts/prop-$$.ndjson"
  > "$receipt_file"
  STATUS_JSON="$SBOX/state/prop-status-$$.json"
  cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"R1","engine":"t","title":"T","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
  tf_status_init

  local log="$SBOX/prop-dispatch-$$.log"
  printf 'usage: { "input_tokens": 100, "output_tokens": 50 }\n' > "$log"

  for i in 1 2 3 4 5; do
    tf_receipt_begin R1 w1 openai gpt-4o "tf/R1"
    tf_receipt_finish_dispatch R1 "$log" 0
    if [[ $((RANDOM % 2)) -eq 0 ]]; then
      tf_receipt_finish_gate R1 "PASS" "true"
      tf_receipt_close R1 "done"
    else
      tf_receipt_close R1 "failed" "test"
    fi
  done
  while IFS= read -r line; do
    jq -e . <<< "$line" >/dev/null 2>&1 || return 1
  done < "$receipt_file"
  rm -f "$receipt_file"
}
tf_property "receipt_ndjson_always_valid" test_receipt_ndjson_valid 5
tf_group_end

tf_test_summary
