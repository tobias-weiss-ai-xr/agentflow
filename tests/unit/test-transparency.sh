#!/usr/bin/env bash
# unit/test-transparency.sh — unit tests for lib/transparency.sh (dispatch + decision logs)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/state/logs"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR"

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/transparency.sh"

echo "=== [unit] transparency.sh ==="

# Dispatch log file path
DISPATCH_LOG="$TF_TRANSPARENCY_LOG/dispatch.tsv"
DECISION_LOG="$TF_TRANSPARENCY_LOG/decisions.tsv"

# ---- Dispatch log ----
tf_group_begin; tf_test "tf_log_dispatch writes TSV entry"
tf_log_dispatch "T1" "dispatch" "worker=w1 tier=standard"
tf_assert "dispatch log exists" test -f "$DISPATCH_LOG"
lines="$(wc -l < "$DISPATCH_LOG")"
tf_assert_eq "one entry" "1" "$lines"
# Verify fields
task="$(awk -F'\t' '{print $2}' "$DISPATCH_LOG")"
action="$(awk -F'\t' '{print $3}' "$DISPATCH_LOG")"
detail="$(awk -F'\t' '{print $7}' "$DISPATCH_LOG")"
tf_assert_eq "task field" "T1" "$task"
tf_assert_eq "action field" "dispatch" "$action"
tf_assert "detail contains worker" echo "$detail" | grep -q "worker=w1"
tf_group_end

# ---- Decision log ----
tf_group_begin; tf_test "tf_log_decision writes TSV entry with context"
tf_log_decision "T1" "DISPATCH" "worker=w1" "affinity=0.75"
tf_assert "decision log exists" test -f "$DECISION_LOG"
lines="$(wc -l < "$DECISION_LOG")"
tf_assert_eq "one entry" "1" "$lines"
task="$(awk -F'\t' '{print $2}' "$DECISION_LOG")"
decision="$(awk -F'\t' '{print $3}' "$DECISION_LOG")"
reason="$(awk -F'\t' '{print $4}' "$DECISION_LOG")"
context="$(awk -F'\t' '{print $5}' "$DECISION_LOG")"
tf_assert_eq "task field" "T1" "$task"
tf_assert_eq "decision field" "DISPATCH" "$decision"
tf_assert "reason contains worker" echo "$reason" | grep -q "worker=w1"
tf_assert "context contains affinity" echo "$context" | grep -q "affinity=0.75"
tf_group_end

# ---- Multiple entries ----
tf_group_begin; tf_test "multiple log entries preserve order"
: > "$DISPATCH_LOG"
tf_log_dispatch "T1" "dispatch" "worker=w1"
tf_log_dispatch "T1" "gate_pass" "verdict=PASS"
tf_log_dispatch "T2" "dispatch" "worker=w2"
lines="$(wc -l < "$DISPATCH_LOG")"
tf_assert_eq "three entries" "3" "$lines"
first_task="$(awk -F'\t' 'NR==1{print $2}' "$DISPATCH_LOG")"
last_task="$(awk -F'\t' 'NR==3{print $2}' "$DISPATCH_LOG")"
tf_assert_eq "first task" "T1" "$first_task"
tf_assert_eq "last task" "T2" "$last_task"
tf_group_end

# ---- Append-only (no overwrite) ----
tf_group_begin; tf_test "log is append-only (existing entries preserved)"
before_lines="$(wc -l < "$DISPATCH_LOG" 2>/dev/null || echo 0)"
tf_log_dispatch "T3" "dispatch" "worker=w3"
after_lines="$(wc -l < "$DISPATCH_LOG")"
tf_assert "lines increased" test "$after_lines" -gt "$before_lines"
tf_group_end

# ---- Speed (append-only TSV is fast) ----
tf_group_begin; tf_test "dispatch log is fast (append-only)"
: > "$DISPATCH_LOG"
start=$(date +%s%N)
for i in $(seq 1 100); do
  tf_log_dispatch "T$i" "dispatch" "worker=w1" >/dev/null
done
end=$(date +%s%N)
elapsed_ms=$(( (end - start) / 1000000 ))
tf_info "100 log writes: ${elapsed_ms}ms"
tf_assert "100 writes under 5000ms" test "$elapsed_ms" -lt 5000
tf_group_end

tf_test_summary
