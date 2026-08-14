#!/usr/bin/env bash
# unit/test-routing.sh — unit tests for cross-model tier routing helpers.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"
export TF_RECEIPT_DIR="$SBOX/receipts"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR"

# Workers with tier capability: cheap handles booster/fast; premium handles
# standard/deep; no-tier worker handles all (default).
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[
  {"name":"cheap","provider":"openai","model":"gpt-4o-mini","enabled":true,"tiers":["booster","fast"]},
  {"name":"premium","provider":"anthropic","model":"claude-sonnet-4-20250514","enabled":true,"tiers":["standard","deep"]},
  {"name":"any","provider":"litellm","model":"deepseek","enabled":true}
],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"

cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"BOOST","engine":"x","title":"boost","section":"s","deps":[],"scope":["a"],"accept":"true","manual":false,"model_tier":"booster"},
  {"id":"STD","engine":"x","title":"std","section":"s","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"DEEP","engine":"x","title":"deep","section":"s","deps":[],"scope":["a"],"accept":"true","manual":false,"model_tier":"deep"}
]}
JSON

tf_reset() {
  STATUS_JSON="$SBOX/state/reset-$$.json"
  rm -f "$TF_RECEIPT_DIR"/*.ndjson
  tf_status_init
}

echo "=== [unit] cross-model routing ==="

# ---- task tier defaults ----
tf_group_begin; tf_test "tf_task_tier returns explicit tier"
tf_reset
tf_assert_eq "booster task tier" "booster" "$(tf_task_tier BOOST)"
tf_assert_eq "deep task tier" "deep" "$(tf_task_tier DEEP)"
tf_group_end

tf_group_begin; tf_test "tf_task_tier defaults to standard"
tf_reset
tf_assert_eq "missing model_tier defaults to standard" "standard" "$(tf_task_tier STD)"
tf_group_end

# ---- worker tier capability ----
tf_group_begin; tf_test "tf_worker_tiers reads explicit tiers"
tf_reset
tf_assert "cheap has booster" tf_worker_tiers cheap | grep -qw booster
tf_assert "cheap has fast" tf_worker_tiers cheap | grep -qw fast
tf_assert "cheap has NO deep" bash -c '! tf_worker_tiers cheap | grep -qw deep'
tf_group_end

tf_group_begin; tf_test "worker without tiers handles all tiers (default)"
tf_reset
tf_assert "any has booster" tf_worker_tiers any | grep -qw booster
tf_assert "any has deep" tf_worker_tiers any | grep -qw deep
tf_group_end

# ---- worker can/can't handle tier ----
tf_group_begin; tf_test "tf_worker_can_tier gates capability"
tf_reset
tf_assert "cheap handles booster" tf_worker_can_tier cheap booster
tf_assert "cheap rejects deep" bash -c '! tf_worker_can_tier cheap deep'
tf_assert "premium handles deep" tf_worker_can_tier premium deep
tf_assert "premium rejects booster" bash -c '! tf_worker_can_tier premium booster'
tf_assert "any handles booster" tf_worker_can_tier any booster
tf_group_end

# ---- tier filtering of candidates (dispatch routing logic) ----
tf_group_begin; tf_test "tier routing filters workers by task tier"
tf_reset
cands() {
  local tid="$1"; shift
  local tier t
  tier="$(tf_task_tier "$tid")"
  for c in "$@"; do
    tf_worker_can_tier "$c" "$tier" && echo "$c"
  done
}
tf_assert_eq "booster task sees cheap+any" "cheap
any" "$(cands BOOST cheap premium any)"
tf_assert_eq "deep task sees premium+any" "premium
any" "$(cands DEEP cheap premium any)"
tf_assert_eq "standard task sees premium+any" "premium
any" "$(cands STD cheap premium any)"
tf_group_end

tf_test_summary
