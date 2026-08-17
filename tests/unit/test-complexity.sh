#!/usr/bin/env bash
# unit/test-complexity.sh — unit tests for lib/complexity.sh (complexity auto-tier)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/state/logs"
export TF_RECEIPT_DIR="$SBOX/state/receipts"
export TF_AUTO_TIER=1
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR"

cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"simple","engine":"rust","title":"Simple task","section":"§1","deps":[],"scope":["src/lib.rs"],"accept":"true","manual":false,"model_tier":"standard"},
  {"id":"complex","engine":"rust","title":"Complex multi-file refactoring with database migrations and API changes","section":"§1","deps":["simple"],"scope":["src/lib.rs","src/api.rs","src/db.rs","src/models.rs","src/migrations/001.sql","src/migrations/002.sql","tests/integration_test.rs","tests/api_test.rs"],"accept":"cargo test","manual":false,"model_tier":"standard"},
  {"id":"trivial","engine":"rust","title":"Fix typo","section":"§1","deps":[],"scope":["README.md"],"accept":"true","manual":false,"model_tier":"standard"}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/cache.sh"
. "$HERE/../../lib/complexity.sh"

echo "=== [unit] complexity.sh ==="

# Build cache so tf_auto_tier can read from it
tf_status_init
tf_cache_build

# ---- Trivial task (small scope, short title) ----
tf_group_begin; tf_test "trivial task → booster tier"
tier="$(tf_auto_tier "trivial")"
tf_assert_eq "trivial → booster" "booster" "$tier"
tf_group_end

# ---- Simple task (moderate scope) ----
tf_group_begin; tf_test "simple task → standard or booster"
tier="$(tf_auto_tier "simple")"
tf_assert "simple is booster or standard" echo "$tier" | grep -qE '^(booster|standard)$'
tf_group_end

# ---- Complex task (large scope, long title, many files) ----
tf_group_begin; tf_test "complex task → deep tier"
tier="$(tf_auto_tier "complex")"
tf_assert_eq "complex → deep" "deep" "$tier"
tf_group_end

# ---- Complexity score is numeric ----
tf_group_begin; tf_test "tf_complexity_score returns number"
score="$(tf_complexity_score "complex")"
tf_assert "score is numeric" echo "$score" | grep -qE '^[0-9]+$'
tf_assert "complex score > 10" test "$score" -gt 10
score_simple="$(tf_complexity_score "trivial")"
tf_assert "trivial score < 10" test "$score_simple" -lt 10
tf_group_end

# ---- Compression ratio ----
tf_group_begin; tf_test "tf_complexity_compress_ratio measures compressibility"
ratio="$(tf_complexity_compress_ratio "src/lib.rs")"
tf_assert "ratio is numeric" echo "$ratio" | grep -qE '^[0-9.]+$'
tf_group_end

# ---- Speed (no jq on hot path) ----
tf_group_begin; tf_test "auto-tier is fast (precomputed)"
start=$(date +%s%N)
for i in $(seq 1 100); do
  tf_auto_tier "complex" >/dev/null
done
end=$(date +%s%N)
elapsed_ms=$(( (end - start) / 1000000 ))
tf_info "100 auto-tier calls: ${elapsed_ms}ms"
tf_assert "100 calls under 15000ms" test "$elapsed_ms" -lt 15000
tf_group_end

tf_test_summary
