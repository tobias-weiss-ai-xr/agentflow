#!/usr/bin/env bash
# unit/test-schedule.sh — unit tests for lib/schedule.sh
# Tests: scope contention detection, critical-path priority, smart ready-listing.
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
export TF_CONTENTION_POLICY="defer"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/schedule.sh"

echo "=== [unit] schedule.sh ==="

# ---- Scope contention detection ----
tf_group_begin; tf_test "tf_scope_files_for returns task scope"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x/a.rs","x/b.rs"],"accept":"true","manual":false},
  {"id":"B","engine":"t","title":"B","section":"§1","deps":[],"scope":["y/c.rs"],"accept":"true","manual":false},
  {"id":"C","engine":"t","title":"C","section":"§1","deps":[],"scope":["x/a.rs","z/d.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init

a_files="$(tf_scope_files_for A | sort)"
b_files="$(tf_scope_files_for B | sort)"
c_files="$(tf_scope_files_for C | sort)"
tf_assert_eq "A has 2 scope files" "2" "$(echo "$a_files" | wc -l)"
tf_assert "A has x/a.rs" echo "$a_files" | grep -q "x/a.rs"
tf_assert "A has x/b.rs" echo "$a_files" | grep -q "x/b.rs"
tf_assert_eq "B has 1 scope file" "1" "$(echo "$b_files" | wc -l)"
tf_group_end

tf_group_begin; tf_test "tf_scope_conflicts detects overlapping scope between tasks"
# A and C share x/a.rs, so they should conflict
STATUS_JSON="$SBOX/state/conflict-status.json"
tf_status_init
# Mark A as running
tf_status_set A running
conflicts="$(tf_scope_conflicts C)"
tf_assert "A conflicts with C (shared x/a.rs)" echo "$conflicts" | grep -qxF "A"
# B does not share scope with C
conflicts_b="$(tf_scope_conflicts B)"
tf_assert_not "B does NOT conflict with C" echo "$conflicts_b" | grep -qxF "C"
tf_group_end

tf_group_begin; tf_test "tf_has_scope_conflict returns correct boolean"
tf_assert "C has conflict with running A" tf_has_scope_conflict C
tf_assert_not "A has no conflict (it is the running task)" tf_has_scope_conflict A
tf_assert_not "B has no conflict" tf_has_scope_conflict B
tf_group_end

tf_group_begin; tf_test "no conflict when no tasks are running"
STATUS_JSON="$SBOX/state/noconflict-status.json"
tf_status_init
# All tasks are ready, none running
tf_assert_not "no conflicts when nothing running" tf_has_scope_conflict A
tf_group_end

tf_group_begin; tf_test "contention defers conflicting tasks in smart ready list"
STATUS_JSON="$SBOX/state/smart-defer.json"
tf_status_init
# A has highest depth, mark it as running
tf_status_set A running
# B and C are ready, but C conflicts with A (shared x/a.rs)
smart="$(tf_smart_ready_task_ids)"
tf_assert "B is in smart ready list" echo "$smart" | grep -qxF "B"
tf_assert_not "C is deferred (conflicts with running A)" echo "$smart" | grep -qxF "C"
tf_group_end

tf_group_begin; tf_test "TF_CONTENTION_POLICY=allow bypasses contention check"
STATUS_JSON="$SBOX/state/allow-policy.json"
tf_status_init
tf_status_set A running
TF_CONTENTION_POLICY="allow" smart_allow="$(tf_smart_ready_task_ids)"
tf_assert "C included when policy=allow" echo "$smart_allow" | grep -qxF "C"
export TF_CONTENTION_POLICY="defer"
tf_group_end

# ---- Critical-path priority ----
tf_group_begin; tf_test "tf_schedule_init computes task depths"
STATUS_JSON="$SBOX/state/depth-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"D1","engine":"t","title":"D1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"D2","engine":"t","title":"D2","section":"§1","deps":["D1"],"scope":["b"],"accept":"true","manual":false},
  {"id":"D3","engine":"t","title":"D3","section":"§1","deps":["D2"],"scope":["c"],"accept":"true","manual":false},
  {"id":"D4","engine":"t","title":"D4","section":"§1","deps":[],"scope":["d"],"accept":"true","manual":false},
  {"id":"D5","engine":"t","title":"D5","section":"§1","deps":["D1"],"scope":["e"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_schedule_init

tf_assert_gt "D1 depth > 0" "0" "$(tf_get_task_depth D1)"
# D1 is a root on the critical path (D1→D2→D3), so it has highest depth
tf_assert_gt "D1 deeper than D2" "$(tf_get_task_depth D2)" "$(tf_get_task_depth D1)"
tf_assert_gt "D1 deeper than D3" "$(tf_get_task_depth D3)" "$(tf_get_task_depth D1)"
# D3 and D4 are leaves (nothing depends on them), so depth=1
tf_assert_eq "D3 is a leaf (depth=1)" "1" "$(tf_get_task_depth D3)"
tf_assert_eq "D4 is a leaf (depth=1)" "1" "$(tf_get_task_depth D4)"
# D2 is in the middle of the critical path
tf_assert_gt "D2 deeper than leaves" "$(tf_get_task_depth D3)" "$(tf_get_task_depth D2)"
tf_group_end

tf_group_begin; tf_test "smart ready list sorts by depth (deepest first)"
# D1 is root, D3 and D4 are leaves. D1 should come first (highest depth).
# D4 is a standalone leaf with depth 1, D1 has depth 3 (longest chain).
all_ready="$(tf_smart_ready_task_ids)"
first="$(echo "$all_ready" | head -1)"
tf_assert_eq "deepest task (D1) dispatched first" "D1" "$first"
tf_group_end

# ---- Integration: contention + priority combined ----
tf_group_begin; tf_test "combined: depth-sorted with contention filtering"
STATUS_JSON="$SBOX/state/combined-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"X1","engine":"t","title":"X1","section":"§1","deps":[],"scope":["shared.rs"],"accept":"true","manual":false},
  {"id":"X2","engine":"t","title":"X2","section":"§1","deps":[],"scope":["shared.rs"],"accept":"true","manual":false},
  {"id":"X3","engine":"t","title":"X3","section":"§1","deps":[],"scope":["unique.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_schedule_init

# Mark X1 as running — X2 shares scope, X3 doesn't
tf_status_set X1 running
smart="$(tf_smart_ready_task_ids)"
tf_assert "X3 is dispatchable" echo "$smart" | grep -qxF "X3"
tf_assert_not "X2 is deferred (scope conflict)" echo "$smart" | grep -qxF "X2"
# Only X3 should be in the list
tf_assert_eq "exactly 1 dispatchable task" "1" "$(echo "$smart" | grep -c .)"
tf_group_end

tf_group_begin; tf_test "deferred task becomes dispatchable after conflicting task finishes"
STATUS_JSON="$SBOX/state/unblock-status.json"
tf_status_init
tf_schedule_init
tf_status_set X1 running
tf_assert_not "X2 deferred while X1 running" tf_has_scope_conflict X3
# Mark X1 done
tf_done_task X1
smart="$(tf_smart_ready_task_ids)"
tf_assert "X2 now dispatchable" echo "$smart" | grep -qxF "X2"
tf_group_end

# ---- Edge cases ----
tf_group_begin; tf_test "tasks with no scope field never conflict"
STATUS_JSON="$SBOX/state/noscope-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"NS1","engine":"t","title":"NS1","section":"§1","deps":[],"scope":[],"accept":"true","manual":false},
  {"id":"NS2","engine":"t","title":"NS2","section":"§1","deps":[],"scope":[],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_status_set NS1 running
tf_assert_not "empty scope never conflicts" tf_has_scope_conflict NS2
tf_group_end

tf_group_begin; tf_test "depths file survives schedule re-init (idempotent)"
STATUS_JSON="$SBOX/state/reinit-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"R1","engine":"t","title":"R1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"R2","engine":"t","title":"R2","section":"§1","deps":["R1"],"scope":["b"],"accept":"true","manual":false}
]}
JSON
tf_status_init
tf_schedule_init
d1="$(tf_get_task_depth R1)"
d2="$(tf_get_task_depth R2)"
# Re-init
tf_schedule_init
tf_assert_eq "R1 depth stable after reinit" "$d1" "$(tf_get_task_depth R1)"
tf_assert_eq "R2 depth stable after reinit" "$d2" "$(tf_get_task_depth R2)"
tf_group_end

# ---- Property: depth cache is always valid JSON ----
tf_group_begin; tf_test "property: task-depths.json is always valid after schedule_init"
test_depths_json_valid() {
  STATUS_JSON="$SBOX/state/prop-depths-$$.json"
  cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"P1","engine":"t","title":"P1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"P2","engine":"t","title":"P2","section":"§1","deps":["P1"],"scope":["b"],"accept":"true","manual":false},
  {"id":"P3","engine":"t","title":"P3","section":"§1","deps":[],"scope":["c"],"accept":"true","manual":false}
]}
JSON
  tf_status_init
  tf_schedule_init
  jq -e . "$TF_STATE_DIR/task-depths.json" >/dev/null 2>&1
}
tf_property "depths_json_always_valid" test_depths_json_valid 5
tf_group_end

tf_test_summary
