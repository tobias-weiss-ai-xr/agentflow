#!/usr/bin/env bash
# golden/test-status-board.sh — golden-file tests for status board output
# Snapshot-based: compares actual output against stored golden files.
# Run TF_UPDATE_GOLDEN=1 to regenerate golden files.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR"

export TF_GOLDEN_DIR="$HERE/golden"
mkdir -p "$TF_GOLDEN_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":2,"retry_cooldown_s":1}}
JSON

cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"G1","engine":"core","title":"Build pipeline","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
  {"id":"G2","engine":"core","title":"Parser layer","section":"§2","deps":["G1"],"scope":["b.rs"],"accept":"true","manual":false},
  {"id":"G3","engine":"core","title":"Renderer","section":"§3","deps":["G1"],"scope":["c.rs"],"accept":"true","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/worktree.sh"
. "$HERE/../../lib/verify.sh"

echo "=== [golden] status board output ==="

tf_group_begin; tf_test "golden: empty status board (all ready)"
STATUS_JSON="$SBOX/state/golden-init.json"
tf_status_init
board="$(tf_status_board 2>&1)"
tf_assert_golden "status-board-init" "$board"
tf_group_end

tf_group_begin; tf_test "golden: mixed status board"
STATUS_JSON="$SBOX/state/golden-mixed.json"
tf_status_init
tf_done_task G1
tf_status_set G2 running '.worker="w1"|.branch="agent/G2"'
tf_fail_task G3 "synthetic"
board="$(tf_status_board 2>&1)"
tf_assert_golden "status-board-mixed" "$board"
tf_group_end

tf_test_summary
