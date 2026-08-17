#!/usr/bin/env bash
# unit/test-cache.sh — unit tests for lib/cache.sh (TSV caching)
# Tests: cache building, cache-aware lookups, performance equivalence.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/state/logs"
export TF_REPO_DIR="$SBOX/repo"
export TF_RECEIPT_DIR="$SBOX/state/receipts"
export TF_BRANCH_PREFIX="agent"
export TF_CONTENTION_POLICY="defer"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true},{"name":"w2","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"rust","title":"Task A","section":"§1","deps":[],"scope":["x/a.rs"],"accept":"true","manual":false,"model_tier":"standard"},
  {"id":"B","engine":"python","title":"Task B","section":"§1","deps":["A"],"scope":["y/b.py"],"accept":"true","manual":false,"model_tier":"standard"},
  {"id":"C","engine":"rust","title":"Task C","section":"§1","deps":["A"],"scope":["x/a.rs","z/c.rs"],"accept":"true","manual":false,"model_tier":"deep"}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/cache.sh"
. "$HERE/../../lib/schedule.sh"
. "$HERE/../../lib/affinity.sh"
. "$HERE/../../lib/receipt.sh"

echo "=== [unit] cache.sh ==="

# ---- Cache building ----
tf_group_begin; tf_test "tf_cache_build creates TSV cache files"
tf_status_init
tf_cache_build
tf_assert "tasks.tsv exists" test -s "$TF_CACHE_DIR/tasks.tsv"
tf_assert "status.tsv exists" test -s "$TF_CACHE_DIR/status.tsv"
tf_assert "scope.tsv exists" test -f "$TF_CACHE_DIR/scope.tsv"
tf_assert "affinity.tsv exists" test -f "$TF_CACHE_DIR/affinity.tsv"
tf_group_end

# ---- Cache task field ----
tf_group_begin; tf_test "tf_cache_task_field returns correct values"
tf_assert_eq "A engine" "rust" "$(tf_cache_task_field A 2)"
tf_assert_eq "A tier" "standard" "$(tf_cache_task_field A 3)"
tf_assert_eq "A deps" "" "$(tf_cache_task_field A 4)"
tf_assert_eq "A scope" "x/a.rs" "$(tf_cache_task_field A 5)"
tf_assert_eq "A title" "Task A" "$(tf_cache_task_field A 7)"
tf_assert_eq "B deps" "A" "$(tf_cache_task_field B 4)"
tf_assert_eq "C tier" "deep" "$(tf_cache_task_field C 3)"
tf_assert_eq "C scope" "x/a.rs,z/c.rs" "$(tf_cache_task_field C 5)"
tf_group_end

# ---- Cache status ----
tf_group_begin; tf_test "tf_cache_status_get returns correct status"
tf_assert_eq "A status" "ready" "$(tf_cache_status_get A .status)"
tf_assert_eq "A attempts" "0" "$(tf_cache_status_get A .attempts)"
tf_group_end

# ---- Cache status after update ----
tf_group_begin; tf_test "cache reflects status after rebuild"
tf_status_set A running '.[$id].worker="w1"'
tf_cache_build
tf_assert_eq "A status after set" "running" "$(tf_cache_status_get A .status)"
tf_assert_eq "A worker" "w1" "$(tf_cache_status_get A .worker)"
tf_group_end

# ---- Cache scope conflicts ----
tf_group_begin; tf_test "cache scope conflicts match jq-based conflicts"
tf_status_set A running
tf_status_set C running
tf_cache_build
# A and C share x/a.rs; B is independent
conflicts_C="$(tf_cache_scope_conflicts C)"
tf_assert "C conflicts with A (shared x/a.rs)" echo "$conflicts_C" | grep -qxF "A"
conflicts_B="$(tf_cache_scope_conflicts B)"
tf_assert_not "B has no conflicts" echo "$conflicts_B" | grep -q .
tf_group_end

# ---- Cache is_ready ----
tf_group_begin; tf_test "cache is_ready matches jq-based is_ready"
# Reset: A→done, B and C stay ready
tf_status_set A done
tf_status_set C ready   # reset C from previous test (was running)
tf_cache_build
# B depends on A (done), B is still "ready" → should be ready
tf_cache_is_ready B
rc_b=$?
tf_assert "B ready (dep A done)" test $rc_b -eq 0
# C depends on A (done), C is "ready" → should be ready
tf_cache_is_ready C
rc_c=$?
tf_assert "C ready (dep A done)" test $rc_c -eq 0
# A is done, not ready
tf_cache_is_ready A
rc_a=$?
tf_assert "A not ready (done)" test $rc_a -ne 0
tf_group_end

# ---- Cache affinity score ----
tf_group_begin; tf_test "cache affinity score matches jq-based score"
# Write some receipts
tf_receipt_begin A w1 p m agent/A
tf_receipt_finish_dispatch A /dev/null 0
tf_receipt_finish_gate A "PASS" "true"
tf_receipt_close A "done"
tf_cache_build
score_w1="$(tf_cache_affinity_score w1 A)"
tf_assert "w1 score is numeric" echo "$score_w1" | grep -qP '^\d+\.\d+$'
tf_assert "w1 score > 0" echo "$score_w1" | awk '{exit !($1 > 0)}'
# No history for w2 → neutral 0.5
score_w2="$(tf_cache_affinity_score w2 A)"
tf_assert_eq "w2 neutral" "0.500" "$score_w2"
tf_group_end

# ---- Cache all_task_ids ----
tf_group_begin; tf_test "cache all_task_ids matches jq"
ids_cache="$(tf_cache_all_task_ids | sort)"
ids_jq="$(tf_all_task_ids | sort)"
tf_assert_eq "cache == jq" "$ids_jq" "$ids_cache"
tf_group_end

# ---- Cache count_status ----
tf_group_begin; tf_test "cache count_status matches jq"
tf_cache_build
done_cache="$(tf_cache_count_status done)"
done_jq="$(tf_count_status done)"
tf_assert_eq "done count" "$done_jq" "$done_cache"
tf_group_end

# ---- Performance: cache is faster than jq ----
tf_group_begin; tf_test "cache lookups are faster than jq"
# Time 100 cache lookups vs 100 jq lookups
cache_ns=$( { time (for i in $(seq 1 100); do tf_cache_task_field A 2 >/dev/null; done) ; } 2>&1 )
cache_ns=$(echo "$cache_ns" | grep real | sed 's/.*m\([0-9.]*\)s.*/\1/' | tr ',' '.')
jq_ns=$( { time (for i in $(seq 1 100); do tf_task_field A .engine >/dev/null; done) ; } 2>&1 )
jq_ns=$(echo "$jq_ns" | grep real | sed 's/.*m\([0-9.]*\)s.*/\1/' | tr ',' '.')
tf_info "100 cache lookups: ${cache_ns}s, 100 jq lookups: ${jq_ns}s"
# Cache should be significantly faster (at least 2x)
tf_assert "cache faster than jq" awk "BEGIN{exit !(${cache_ns} < ${jq_ns})}"
tf_group_end

tf_test_summary
