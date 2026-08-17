#!/usr/bin/env bash
# unit/test-trust.sh — unit tests for lib/trust.sh (trust score)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/state/logs"
export TF_RECEIPT_DIR="$SBOX/state/receipts"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR"

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/trust.sh"

echo "=== [unit] trust.sh ==="

# ---- Trust score: perfect task ----
tf_group_begin; tf_test "trust score for perfect task (gate pass, no scope violations, 0 retries)"
score="$(tf_trust_score "T1" 1 1 0 "w1" "rust")"
tf_assert "score >= 80" test "$score" -ge 80
tf_assert "score <= 100" test "$score" -le 100
tf_group_end

# ---- Trust score: failed gate ----
tf_group_begin; tf_test "trust score for failed gate"
score="$(tf_trust_score "T2" 0 1 0 "w1" "rust")"
tf_assert "score < 50" test "$score" -lt 50
tf_group_end

# ---- Trust score: scope violation ----
tf_group_begin; tf_test "trust score for scope violation"
score_ok="$(tf_trust_score "T3" 1 1 0 "w1" "rust")"
score_violation="$(tf_trust_score "T3" 1 0 0 "w1" "rust")"
tf_assert "scope violation lowers score" test "$score_violation" -lt "$score_ok"
tf_group_end

# ---- Trust score: retries lower score ----
tf_group_begin; tf_test "trust score decreases with retries"
score_0="$(tf_trust_score "T4" 1 1 0 "w1" "rust")"
score_2="$(tf_trust_score "T4" 1 1 2 "w1" "rust")"
score_5="$(tf_trust_score "T4" 1 1 5 "w1" "rust")"
tf_assert "0 retries >= 2 retries" test "$score_0" -ge "$score_2"
tf_assert "2 retries >= 5 retries" test "$score_2" -ge "$score_5"
tf_group_end

# ---- Trust bucket ----
tf_group_begin; tf_test "tf_trust_bucket classifies correctly"
tf_assert_eq "90 → trusted" "trusted" "$(tf_trust_bucket 90)"
tf_assert_eq "80 → trusted" "trusted" "$(tf_trust_bucket 80)"
tf_assert_eq "79 → review" "review" "$(tf_trust_bucket 79)"
tf_assert_eq "50 → review" "review" "$(tf_trust_bucket 50)"
tf_assert_eq "49 → blocked" "blocked" "$(tf_trust_bucket 49)"
tf_assert_eq "0 → blocked" "blocked" "$(tf_trust_bucket 0)"
tf_group_end

# ---- Trust score is pure arithmetic (no jq) ----
tf_group_begin; tf_test "trust score is fast (pure bash arithmetic)"
start=$(date +%s%N)
for i in $(seq 1 100); do
  tf_trust_score "T5" 1 1 0 "w1" "rust" >/dev/null
done
end=$(date +%s%N)
elapsed_ms=$(( (end - start) / 1000000 ))
tf_info "100 trust scores: ${elapsed_ms}ms"
tf_assert "100 scores under 1000ms" test "$elapsed_ms" -lt 1000
tf_group_end

tf_test_summary
