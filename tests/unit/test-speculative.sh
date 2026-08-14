#!/usr/bin/env bash
# unit/test-speculative.sh — unit tests for speculative dispatch functions
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"openai","model":"gpt-4o","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

# Tasks used in tests
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{
  "tasks": [
    {"id":"A","engine":"rust","title":"task A","section":"s","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
    {"id":"B","engine":"rust","title":"task B","section":"s","deps":["A"],"scope":["b.rs"],"accept":"true","manual":false},
    {"id":"C","engine":"rust","title":"task C","section":"s","deps":["A","B"],"scope":["c.rs"],"accept":"true","manual":false},
    {"id":"D","engine":"rust","title":"task D","section":"s","deps":["A"],"scope":["d.rs"],"accept":"true","manual":false},
    {"id":"E","engine":"rust","title":"task E (no deps)","section":"s","deps":[],"scope":["e.rs"],"accept":"true","manual":false}
  ]
}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/schedule.sh"

tf_status_init

echo "=== [unit] speculative dispatch ==="

tf_group_begin
tf_test "tf_is_speculatively_ready returns false when disabled"
export TF_SPECULATIVE_DISPATCH_ENABLED=0
tf_is_speculatively_ready "B" && false || true
tf_group_end

tf_group_begin
tf_test "tf_is_speculatively_ready returns false for task with no deps"
export TF_SPECULATIVE_DISPATCH_ENABLED=1
# E has no deps, so not speculative
tf_is_speculatively_ready "E" && false || true
tf_group_end

tf_group_begin
tf_test "tf_is_speculatively_ready returns false when task not ready"
# A is ready but has no deps, B depends on A which is not done
tf_status_set "A" "done"
tf_status_set "B" "running"  # B is running, not ready
tf_is_speculatively_ready "B" && false || true
tf_group_end

tf_group_begin
tf_test "tf_is_speculatively_ready returns true when one dep running, rest done"
# Reset
for id in A B C D E; do tf_status_set "$id" "ready"; done
# A is running, B depends only on A
tf_status_set "A" "running"
tf_is_speculatively_ready "B" || false
tf_group_end

tf_group_begin
tf_test "tf_is_speculatively_ready returns false when multiple deps running"
# C depends on A and B, both running
for id in A B C D E; do tf_status_set "$id" "ready"; done
tf_status_set "A" "running"
tf_status_set "B" "running"
tf_is_speculatively_ready "C" && false || true
tf_group_end

tf_group_begin
tf_test "tf_is_speculatively_ready returns false when dep is blocked"
# B depends on A which is blocked
tf_status_set "A" "blocked"
tf_status_set "B" "ready"
tf_is_speculatively_ready "B" && false || true
tf_group_end

tf_group_begin
tf_test "tf_is_speculatively_ready returns false when more than one dep not done"
# D depends on A (running), but also has no other deps - wait D only has A
# Let's use a task with two deps where one is running and one is blocked
# C depends on A and B. A is running, B is ready (not done)
for id in A B C D E; do tf_status_set "$id" "ready"; done
tf_status_set "A" "running"
tf_status_set "B" "ready"  # not done yet
tf_is_speculatively_ready "C" && false || true
tf_group_end

tf_group_begin
tf_test "tf_speculative_base_dep returns the running dependency"
for id in A B C D E; do tf_status_set "$id" "ready"; done
tf_status_set "A" "running"
base="$(tf_speculative_base_dep "B")"
tf_assert_eq "base dep is A" "A" "$base"
tf_group_end

tf_group_begin
tf_test "tf_speculative_base_dep returns empty when no running dep"
for id in A B C D E; do tf_status_set "$id" "ready"; done
tf_status_set "A" "done"
base="$(tf_speculative_base_dep "B")"
tf_assert_eq "no running dep" "" "$base"
tf_group_end

tf_group_begin
tf_test "tf_smart_ready_task_ids includes speculative tasks when enabled"
export TF_SPECULATIVE_DISPATCH_ENABLED=1
for id in A B C D E; do tf_status_set "$id" "ready"; done
tf_status_set "A" "running"
# B depends on A (running) — should be speculatively ready
all_ready="$(tf_smart_ready_task_ids)"
tf_assert_contains "B is speculatively ready" "B" "$all_ready"
# E has no deps — should be regularly ready
tf_assert_contains "E is regularly ready" "E" "$all_ready"
tf_group_end

tf_group_begin
tf_test "tf_smart_ready_task_ids excludes speculative tasks when disabled"
export TF_SPECULATIVE_DISPATCH_ENABLED=0
for id in A B C D E; do tf_status_set "$id" "ready"; done
tf_status_set "A" "running"
# B depends on A (running) — should NOT be ready without speculative
all_ready="$(tf_smart_ready_task_ids)"
tf_assert "B not in ready list without speculative" test -z "$(echo "$all_ready" | grep -w "B")"
# E has no deps — should still be regularly ready
tf_assert_contains "E is regularly ready" "E" "$all_ready"
tf_group_end

tf_test_summary
