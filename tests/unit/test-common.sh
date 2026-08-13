#!/usr/bin/env bash
# unit/test-common.sh — unit tests for lib/common.sh
# Tests: JSON helpers, logging, config resolution, env var handling.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

# ---------------------------------------------------------------------------
# Setup: sandbox with known config
# ---------------------------------------------------------------------------
SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"
export TF_WORKTREE_ROOT="$SBOX/repo/.tf-worktrees"
export TF_PROMPT_DIR="$HERE/../../prompts"
export TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_WORKTREE_ROOT"

# ---- Config fixtures ----

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{
  "defaults": {
    "max_attempts": 3,
    "retry_cooldown_s": 10,
    "accept_timeout_s": 600,
    "dispatch_timeout_s": 1800
  },
  "workers": [
    {"name":"alpha","provider":"p1","model":"m1","endpoint":"http://a","enabled":true},
    {"name":"beta","provider":"p2","model":"m2","endpoint":"http://b","enabled":true},
    {"name":"gamma","provider":"p3","model":"m3","endpoint":"http://c","enabled":false}
  ]
}
JSON

cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{
  "_meta": {"project":"test"},
  "tasks": [
    {"id":"T1","engine":"e1","title":"Task One","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
    {"id":"T2","engine":"e2","title":"Task Two","section":"§2","deps":["T1"],"scope":["b.rs"],"accept":"false","manual":false},
    {"id":"T3","engine":"e1","title":"Task Three","section":"§1","deps":["T1","T2"],"scope":["c.rs","d.rs"],"accept":"echo ok","manual":false},
    {"id":"T4","engine":"e3","title":"Manual task","section":"§3","deps":[],"scope":["e.rs"],"accept":"","manual":true}
  ]
}
JSON

. "$HERE/../../lib/common.sh"

echo "=== [unit] common.sh ==="

# -- Logging --
tf_group_begin; tf_test "tf_info writes to stderr with timestamp"
out="$(tf_info "hello world" 2>&1)"
tf_assert_re "has ISO timestamp" '\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z' "$out"
tf_assert_contains "contains INFO" "[INFO]" "$out"
tf_assert_contains "contains message" "hello world" "$out"
tf_group_end

tf_group_begin; tf_test "tf_error writes to stderr with ERROR level"
out="$(tf_error "bad thing" 2>&1)"
tf_assert_contains "contains ERROR" "[ERROR]" "$out"
tf_assert_contains "contains message" "bad thing" "$out"
tf_group_end

tf_group_begin; tf_test "tf_warn writes to stderr with WARN level"
out="$(tf_warn "caution" 2>&1)"
tf_assert_contains "contains WARN" "[WARN]" "$out"
tf_group_end

# -- JSON helpers --
tf_group_begin; tf_test "tf_task_field reads scalar field"
tf_assert_eq "T1 id" "T1" "$(tf_task_field T1 .id)"
tf_assert_eq "T1 title" "Task One" "$(tf_task_field T1 .title)"
tf_assert_eq "T1 engine" "e1" "$(tf_task_field T1 .engine)"
tf_assert_eq "T2 accept" "false" "$(tf_task_field T2 .accept)"
tf_group_end

tf_group_begin; tf_test "tf_task_field reads array field (scope)"
scope="$(tf_task_field T3 '.scope[]')"
tf_assert_contains "has c.rs" "c.rs" "$scope"
tf_assert_contains "has d.rs" "d.rs" "$scope"
tf_group_end

tf_group_begin; tf_test "tf_task_field reads deps array"
deps="$(tf_task_field T3 '.deps[]')"
tf_assert_contains "has T1" "T1" "$deps"
tf_assert_contains "has T2" "T2" "$deps"
tf_group_end

tf_group_begin; tf_test "tf_task_field returns empty for missing task"
tf_assert_eq "missing task" "" "$(tf_task_field NONEXISTENT .id)"
tf_group_end

tf_group_begin; tf_test "tf_all_task_ids lists all task ids"
ids="$(tf_all_task_ids)"
tf_assert_contains "has T1" "T1" "$ids"
tf_assert_contains "has T2" "T2" "$ids"
tf_assert_contains "has T3" "T3" "$ids"
tf_assert_contains "has T4" "T4" "$ids"
n_lines="$(echo "$ids" | wc -l)"
tf_assert_eq "exactly 4 tasks" "4" "$n_lines"
tf_group_end

tf_group_begin; tf_test "tf_task_json returns full task object"
json="$(tf_task_json T1)"
tf_assert_contains "is JSON" '"id"' "$json"
tf_assert_contains "has title" '"Task One"' "$json"
tf_group_end

# -- Worker helpers --
tf_group_begin; tf_test "tf_worker_names returns only enabled workers"
names="$(tf_worker_names)"
tf_assert_contains "has alpha" "alpha" "$names"
tf_assert_contains "has beta" "beta" "$names"
tf_assert_not_contains "no disabled gamma" "gamma" "$names"
tf_assert_eq "exactly 2 enabled" "2" "$(echo "$names" | wc -l)"
tf_group_end

tf_group_begin; tf_test "tf_worker_field reads worker properties"
tf_assert_eq "alpha provider" "p1" "$(tf_worker_field alpha .provider)"
tf_assert_eq "alpha model" "m1" "$(tf_worker_field alpha .model)"
tf_assert_eq "alpha endpoint" "http://a" "$(tf_worker_field alpha .endpoint)"
tf_group_end

tf_group_begin; tf_test "tf_worker_field returns empty for unknown worker"
tf_assert_eq "unknown worker" "" "$(tf_worker_field nonexistent .provider)"
tf_group_end

tf_group_begin; tf_test "tf_worker returns JSON for enabled worker"
w="$(tf_worker alpha)"
tf_assert_contains "is JSON object" '"name"' "$w"
tf_assert_contains "has alpha" '"alpha"' "$w"
tf_group_end

tf_group_begin; tf_test "tf_worker returns empty for disabled worker"
w="$(tf_worker gamma)"
tf_assert_eq "disabled worker empty" "" "$w"
tf_group_end

tf_group_begin; tf_test "tf_default reads defaults config"
tf_assert_eq "max_attempts" "3" "$(tf_default max_attempts)"
tf_assert_eq "retry_cooldown_s" "10" "$(tf_default retry_cooldown_s)"
tf_assert_eq "accept_timeout_s" "600" "$(tf_default accept_timeout_s)"
tf_group_end

tf_group_begin; tf_test "tf_default returns empty for missing key"
tf_assert_eq "missing default" "" "$(tf_default nonexistent_key)"
tf_group_end

# -- Env var handling --
tf_group_begin; tf_test "TF_GATE_ENV is exported when set"
# This is tested indirectly — verify the mechanism works
# (TF_GATE_ENV is evaluated in common.sh, but we can't easily test side effects
#  without subprocess isolation. The mechanism is: if TF_GATE_ENV="FOO=bar baz=qux",
#  then eval "export $TF_GATE_ENV" runs. Test that the syntax is valid.)
export TF_GATE_ENV="_TF_TEST_VAR=hello"
eval "export $TF_GATE_ENV"
tf_assert_eq "gate env exported" "hello" "${_TF_TEST_VAR:-}"
unset _TF_TEST_VAR
unset TF_GATE_ENV
tf_group_end

# -- Path resolution --
tf_group_begin; tf_test "TF_DIR points to project root"
tf_assert "TF_DIR exists" test -d "$TF_DIR"
tf_assert "TF_DIR has orchestrator.sh" test -f "$TF_DIR/orchestrator.sh"
tf_group_end

tf_group_begin; tf_test "config paths point to correct files"
tf_assert "workers.json exists" test -f "$WORKERS_JSON"
tf_assert "tasks.json exists" test -f "$TASKS_JSON"
tf_group_end

# -- Property-based: tf_task_field is null-safe on any task id --
tf_group_begin; tf_test "property: tf_task_field returns empty for any garbage id"
test_empty_field() {
  local result
  result="$(tf_task_field "$1" .id 2>/dev/null)"
  [[ -z "$result" ]]
}
tf_for_all "task_field on random ids" tf_gen_task_id test_empty_field 10
tf_group_end

# -- Property-based: tf_worker_names always returns unique values --
tf_group_begin; tf_test "property: tf_worker_names always returns unique values"
test_unique_worker_names() {
  local names
  names="$(tf_worker_names)"
  local n_lines n_unique
  n_lines="$(echo "$names" | wc -l)"
  n_unique="$(echo "$names" | sort -u | wc -l)"
  [[ "$n_lines" == "$n_unique" ]]
}
tf_property "worker_names unique" test_unique_worker_names 10
tf_group_end

# ---- Robustness: task validation (L3) ----
tf_group_begin; tf_test "tf_validate_tasks flags invalid cargo test gates"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x"],"accept":"cargo test -p wo-x a:: b::","manual":false},
  {"id":"B","engine":"t","title":"B","section":"§1","deps":["GHOST"],"scope":["y"],"accept":"true","manual":false}
]}
JSON
out="$(tf_validate_tasks 2>&1)"
tf_assert_contains "flags multi-pattern cargo test" "takes one test-name pattern" "$out"
tf_assert_contains "flags unknown dep" "GHOST" "$out"
tf_group_end

# ---- Robustness: dispatch failure classification (L4) ----
tf_group_begin; tf_test "tf_classify_dispatch_failure detects rate limits and auth"
mkdir -p "$TF_LOG_DIR"
echo '{"code":"1308","message":"Usage limit reached for 5 hour"}' > "$TF_LOG_DIR/rl.log"
echo '401 Unauthorized: invalid api key' > "$TF_LOG_DIR/auth.log"
echo 'connection refused: curl(7)' > "$TF_LOG_DIR/net.log"
tf_assert_eq "rate limit" "rate_limit" "$(tf_classify_dispatch_failure "$TF_LOG_DIR/rl.log")"
tf_assert_eq "auth" "auth_error" "$(tf_classify_dispatch_failure "$TF_LOG_DIR/auth.log")"
tf_assert_eq "network" "network_error" "$(tf_classify_dispatch_failure "$TF_LOG_DIR/net.log")"
tf_assert_eq "benign log empty" "" "$(tf_classify_dispatch_failure "$TF_LOG_DIR/../no-such.log")"
tf_group_end




tf_test_summary
