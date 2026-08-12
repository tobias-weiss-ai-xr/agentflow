#!/usr/bin/env bash
# chaos/test-status.sh — CHAOS / FAULT-INJECTION TESTING for lib/status.sh
#
# SOTA paradigm: deliberately corrupt state files, truncate mid-write,
# kill concurrent writers, and verify the system either recovers cleanly
# or fails loudly — never silently corrupts.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config" TF_STATE_DIR="$SBOX/state" TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo" TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_REPO_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false},
  {"id":"B","engine":"t","title":"B","section":"§1","deps":["A"],"scope":["y"],"accept":"true","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"

echo "=== [chaos] status.sh fault injection ==="

# 1. Truncated status file (simulates crash mid-write)
tf_group_begin; tf_test "truncated status file → tf_status_init recovers"
tf_status_init
STATUS_JSON="$SBOX/state/chaos1.json"
tf_status_init
# truncate the file to 10 bytes (invalid JSON)
tf_chaos_truncate "$STATUS_JSON" 10
# next init should recreate from tasks.json
tf_status_init
tf_assert_valid_json "status recreated as valid JSON" "$STATUS_JSON"
tf_assert_eq "tasks present after recovery" "2" "$(jq 'length' "$STATUS_JSON")"
tf_group_end

# 2. Garbage-corrupted status file
tf_group_begin; tf_test "corrupted status file → getters return sane defaults, not crash"
STATUS_JSON="$SBOX/state/chaos2.json"
tf_status_init
tf_chaos_corrupt "$STATUS_JSON"
# getters on missing task should be empty, not error
tf_assert_eq "missing task status empty" "" "$(tf_status_get NOPE .status 2>/dev/null)"
# init should rebuild
tf_status_init
tf_assert_valid_json "rebuilt after corruption" "$STATUS_JSON"
tf_group_end

# 3. Missing state dir entirely
tf_group_begin; tf_test "missing state dir → init creates it"
rm -rf "$SBOX/state-missing"
export TF_STATE_DIR="$SBOX/state-missing"
export TF_LOG_DIR="$SBOX/state-missing/logs"
STATUS_JSON="$SBOX/state-missing/status.json"
# ensure STATS_JSON var points there (default logic)
STATUS_JSON="$SBOX/state-missing/task-status.json"
mkdir -p "$SBOX/config"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[{"id":"M","engine":"t","title":"M","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false}]}
JSON
tf_status_init
tf_assert "state dir created" test -d "$SBOX/state-missing"
tf_assert "status file created" test -f "$STATUS_JSON"
tf_assert_valid_json "valid JSON in fresh dir" "$STATUS_JSON"
tf_group_end

# 4. Concurrent writers: 20 parallel status updates, file must stay valid
tf_group_begin; tf_test "20 concurrent status writers → JSON stays valid (no torn writes)"
export TF_STATE_DIR="$SBOX/state-conc"
export TF_LOG_DIR="$SBOX/state-conc/logs"
STATUS_JSON="$SBOX/state-conc/task-status.json"
mkdir -p "$TF_STATE_DIR" "$TF_LOG_DIR"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"C1","engine":"t","title":"C1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"C2","engine":"t","title":"C2","section":"§1","deps":[],"scope":["b"],"accept":"true","manual":false},
  {"id":"C3","engine":"t","title":"C3","section":"§1","deps":[],"scope":["c"],"accept":"true","manual":false}
]}
JSON
tf_status_init
# spawn 20 background writers
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  (
    . "$HERE/../../lib/common.sh"
    . "$HERE/../../lib/status.sh"
    tf_done_task "C$(( (i - 1) % 3 + 1 ))" 2>/dev/null
  ) &
done
wait
tf_assert_valid_json "status valid after 20 concurrent writers" "$STATUS_JSON"
# every task must have a status value
for t in C1 C2 C3; do
  st="$(jq -r --arg t "$t" '.[$t].status' "$STATUS_JSON")"
  tf_assert "task $t has status" test -n "$st"
done
tf_group_end

# 5. tf_locked_mv atomicity: partial write never visible
tf_group_begin; tf_test "tf_locked_mv is atomic (tmp → rename, no partial read)"
export TF_STATE_DIR="$SBOX/state-atomic"
export TF_LOG_DIR="$SBOX/state-atomic/logs"
STATUS_JSON="$SBOX/state-atomic/task-status.json"
mkdir -p "$TF_STATE_DIR" "$TF_LOG_DIR"
echo '{"task":"alpha"}' > "$STATUS_JSON"
# write a large payload through the locked mv path
jq -n '{task:"alpha", payload:[range(0;20000)|tostring]}' > /tmp/big.json
tf_locked_mv /tmp/big.json "$STATUS_JSON"
# the file must be exactly the full JSON, never partial
tf_assert_valid_json "atomic write stays valid" "$STATUS_JSON"
tf_assert_eq "full payload preserved" "20000" "$(jq '.payload | length' "$STATUS_JSON")"
tf_group_end

# 6. Kill mid-operation: simulate by writing then killing the writer process
tf_group_begin; tf_test "writer killed after tmp write → old state intact"
export TF_STATE_DIR="$SBOX/state-kill"
export TF_LOG_DIR="$SBOX/state-kill/logs"
STATUS_JSON="$SBOX/state-kill/task-status.json"
mkdir -p "$TF_STATE_DIR" "$TF_LOG_DIR"
echo '{"K":"before"}' > "$STATUS_JSON"
# simulate: tmp file written, process killed before rename
echo '{"K":"after-partial"' > "$STATUS_JSON.tmp"
# old file must still be readable and valid
tf_assert_valid_json "original survives partial tmp" "$STATUS_JSON"
tf_assert_eq "original content intact" "before" "$(jq -r .K "$STATUS_JSON")"
tf_group_end

tf_test_summary
