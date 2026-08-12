#!/usr/bin/env bash
# test-status.sh — task status machine: init, ready detection, dep blocking,
# fail→retry→ready cooldown, done propagation.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/test-harness.sh"

# Isolated sandbox: temp config + state
SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR"

# Minimal dependency graph: A (none) → B (A) → C (A,B); plus isolated D.
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{
  "_meta": {"task_count": 4},
  "tasks": [
    {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x.rs"],"accept":"true","manual":false},
    {"id":"B","engine":"t","title":"B","section":"§1","deps":["A"],"scope":["y.rs"],"accept":"true","manual":false},
    {"id":"C","engine":"t","title":"C","section":"§1","deps":["A","B"],"scope":["z.rs"],"accept":"true","manual":false},
    {"id":"D","engine":"t","title":"D","section":"§1","deps":[],"scope":["w.rs"],"accept":"true","manual":false}
  ]
}
JSON
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"dispatch_timeout_s":30,"accept_timeout_s":10,"max_attempts":2,"retry_cooldown_s":1}}
JSON

. "$HERE/../lib/common.sh"
. "$HERE/../lib/status.sh"

echo "=== Status machine tests ==="

tf_group_begin; tf_test "init creates ready entries for all tasks"
tf_status_init
tf_assert_eq "status file has 4 tasks" "4" "$(jq 'length' "$STATUS_JSON")"
tf_assert_eq "A status" "ready" "$(tf_status_get A .status)"
tf_assert_eq "B status" "ready" "$(tf_status_get B .status)"
tf_assert_eq "D attempts" "0" "$(tf_status_get D .attempts)"
tf_group_end

tf_group_begin; tf_test "ready detection respects deps"
# A and D have no deps → ready. B needs A (not done) → not ready. C needs A,B → not ready.
tf_assert "A is ready"        tf_is_ready A
tf_assert "D is ready"        tf_is_ready D
tf_assert_not "B is NOT ready"    tf_is_ready B
tf_assert_not "C is NOT ready"    tf_is_ready C
local_a="$(tf_ready_task_ids)"
tf_assert_eq "ready set = A,D" "$(echo -e 'A\nD' | sort)" "$(echo "$local_a" | sort)"
tf_group_end

tf_group_begin; tf_test "done propagates: after A done, B becomes ready"
tf_done_task A
tf_assert_eq "A done" "done" "$(tf_status_get A .status)"
tf_assert "B is now ready"  tf_is_ready B
tf_assert_not "C is NOT ready (still needs B)"  tf_is_ready C
tf_group_end

tf_group_begin; tf_test "chain completion: B done → C ready → C done"
tf_done_task B
tf_assert "C is now ready"  tf_is_ready C
tf_done_task C
tf_assert_eq "C done" "done" "$(tf_status_get C .status)"
tf_group_end

tf_group_begin; tf_test "fail → retry cooldown → ready again (under max_attempts=2)"
# D fails once: attempts 1/2, scheduled retry, NOT ready until cooldown passes
tf_fail_task D "synthetic failure 1"
tf_assert_eq "D attempts after 1 fail" "1" "$(tf_status_get D .attempts)"
tf_assert_eq "D status" "failed" "$(tf_status_get D .status)"
# immediately after, still within cooldown → not ready
tf_assert_not "D NOT ready immediately after fail"  tf_is_ready D
# wait out the 1s cooldown
sleep 2
tf_assert "D ready again after cooldown"  tf_is_ready D
tf_group_end

tf_group_begin; tf_test "fail twice → permanently failed (max_attempts=2)"
tf_is_ready D >/dev/null 2>&1 || true   # consume the flip-to-ready
# now D is ready again; fail it a second time
tf_fail_task D "synthetic failure 2"
tf_assert_eq "D attempts after 2 fails" "2" "$(tf_status_get D .attempts)"
tf_assert_eq "D permanently failed" "failed" "$(tf_status_get D .status)"
tf_assert "D has no next_retry"  test -z "$(tf_status_get D .next_retry_at)"
# failed-permanent tasks are never ready
tf_assert_not "D NOT ready when permanently failed"  tf_is_ready D
tf_group_end

tf_group_begin; tf_test "count_status tallies correctly"
tf_assert_eq "done count"   "3" "$(tf_count_status done)"     # A,B,C
tf_assert_eq "failed count" "1" "$(tf_count_status failed)"   # D
tf_assert_eq "ready count"  "0" "$(tf_count_status ready)"    # nothing ready
tf_group_end

tf_test_summary
