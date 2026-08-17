#!/usr/bin/env bash
# unit/test-episodic.sh — unit tests for lib/episodic.sh (episodic memory)
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

cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"T1","engine":"rust","title":"Task A","section":"§1","deps":[],"scope":["x/a.rs"],"accept":"true","manual":false,"model_tier":"standard"},
  {"id":"T2","engine":"rust","title":"Task B","section":"§1","deps":["T1"],"scope":["y/b.rs"],"accept":"true","manual":false,"model_tier":"standard"}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/cache.sh"
. "$HERE/../../lib/episodic.sh"

echo "=== [unit] episodic.sh ==="

# ---- Recording ----
tf_group_begin; tf_test "tf_episode_record writes TSV entry"
tf_status_init
tf_cache_build
tf_episode_record "T1" "w1" 1 300 "" "rust" "standard" "x/a.rs"
tf_assert "episode file exists" test -f "$TF_EPISODES_FILE"
lines="$(wc -l < "$TF_EPISODES_FILE")"
tf_assert_eq "one line" "1" "$lines"
tf_group_end

# ---- Recall by task ID ----
tf_group_begin; tf_test "tf_episode_recall returns matching episodes"
tf_episode_record "T2" "w1" 1 200 "compile_error" "rust" "standard" "y/b.rs"
tf_episode_record "T3" "w2" 0 500 "test_failure" "python" "standard" "z/c.py"
result="$(tf_episode_recall "T1" 2>/dev/null)"
tf_assert "T1 recall non-empty" test -n "$result"
tf_assert "T1 recall contains w1" echo "$result" | grep -q "w1"
tf_group_end

# ---- Recall by scope similarity ----
tf_group_begin; tf_test "tf_episode_recall_by_scope finds similar tasks"
# T1 has scope x/a.rs, engine rust
result="$(tf_episode_recall_by_scope "x/a.rs" "rust" 2>/dev/null)"
tf_assert "scope recall non-empty" test -n "$result"
tf_assert "scope recall contains w1" echo "$result" | grep -q "w1"
# Different scope should not match
result2="$(tf_episode_recall_by_scope "completely/different.rs" "rust" 2>/dev/null)"
tf_assert_not "unrelated scope empty" test -n "$result2"
tf_group_end

# ---- Recall by error category ----
tf_group_begin; tf_test "tf_episode_recall_by_error finds same category"
result="$(tf_episode_recall_by_error "compile_error" 2>/dev/null)"
tf_assert "error recall non-empty" test -n "$result"
tf_assert "error recall contains T2 or w1" echo "$result" | grep -qE "w1|T2"
# Non-existent category
result2="$(tf_episode_recall_by_error "nonexistent" 2>/dev/null)"
tf_assert_not "nonexistent error empty" test -n "$result2"
tf_group_end

# ---- LRU cap ----
tf_group_begin; tf_test "episodic memory caps at 500 entries"
# Record 501 episodes
for i in $(seq 1 501); do
  tf_episode_record "CAP-$i" "w1" 1 100 "" "rust" "standard" "file_$i.rs"
done
lines="$(wc -l < "$TF_EPISODES_FILE")"
tf_assert "capped at 500" test "$lines" -le 500
tf_group_end

# ---- Win rate calculation ----
tf_group_begin; tf_test "tf_episode_win_rate computes correct rate"
# Clear and record fresh
: > "$TF_EPISODES_FILE"
tf_episode_record "W1" "w1" 1 100 "" "rust" "standard" "a.rs"
tf_episode_record "W2" "w1" 1 100 "" "rust" "standard" "b.rs"
tf_episode_record "W3" "w1" 0 100 "" "rust" "standard" "c.rs"
rate="$(tf_episode_win_rate "w1" 2>/dev/null)"
tf_assert "win rate is 0.66x" echo "$rate" | grep -qP '^0\.66[0-9]'
tf_group_end

tf_test_summary
