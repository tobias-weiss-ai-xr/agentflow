#!/usr/bin/env bash
# unit/test-api.sh — unit tests for taskfleet api subcommands.
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
export TF_RECEIPT_DIR="$SBOX/receipts"
export TF_PROMPT_DIR="$SBOX/prompts"
export TF_LIB_DIR="$SBOX/lib"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR" "$TF_PROMPT_DIR" "$TF_LIB_DIR"

# Create dummy prompt template
echo '{{TASK_ID}}: {{TASK_TITLE}}' > "$TF_PROMPT_DIR/worker.md"

# Basic workers config
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[
  {"name":"default","provider":"openai","model":"gpt-4o","enabled":true}
],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"

# API functions (copied from orchestrator.sh for testing)
tf_api_add() {
  local task_desc="" priority=5 scope="" accept="true"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --task) task_desc="$2"; shift 2 ;;
      --priority) priority="$2"; shift 2 ;;
      --scope) scope="$2"; shift 2 ;;
      --accept) accept="$2"; shift 2 ;;
      *) tf_error "unknown arg: $1"; return 1 ;;
    esac
  done
  [[ -z "$task_desc" ]] && { tf_error "--task is required"; return 1; }
  local task_id="T-$(date +%s%N | md5sum | head -c8)"
  local task_json
  task_json=$(jq -n \
    --arg id "$task_id" \
    --arg title "$task_desc" \
    --argjson priority "$priority" \
    --arg scope "$scope" \
    --arg accept "$accept" \
    '{id: $id, engine: "t", title: $title, section: "§api", deps: [], scope: (if $scope == "" then [] else [$scope] end), accept: $accept, manual: false, priority: $priority}')
  local tmp
  tmp=$(mktemp)
  jq --argjson task "$task_json" '.tasks += [$task]' "$TASKS_JSON" > "$tmp"
  mv "$tmp" "$TASKS_JSON"
  tf_info "Created task $task_id: $task_desc"
  echo "$task_id"
}

tf_api_status() {
  local json_output=0 task_id=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --json) json_output=1; shift ;;
      --task) task_id="$2"; shift 2 ;;
      *) tf_error "unknown arg: $1"; return 1 ;;
    esac
  done
  if [[ -n "$task_id" ]]; then
    if [[ "$json_output" -eq 1 ]]; then
      jq --arg id "$task_id" '.[$id] // {error: "task not found"}' "$STATUS_JSON"
    else
      local status title
      status=$(jq -r --arg id "$task_id" '.[$id].status // "not found"' "$STATUS_JSON")
      title=$(jq -r --arg id "$task_id" '.[$id].title // "unknown"' "$STATUS_JSON")
      echo "$task_id: $status ($title)"
    fi
  else
    if [[ "$json_output" -eq 1 ]]; then
      jq '.' "$STATUS_JSON"
    else
      tf_status_board
    fi
  fi
}

tf_api_results() {
  local task_id=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --task) task_id="$2"; shift 2 ;;
      *) tf_error "unknown arg: $1"; return 1 ;;
    esac
  done
  [[ -z "$task_id" ]] && { tf_error "--task is required"; return 1; }
  local status
  status=$(jq -r --arg id "$task_id" '.[$id].status // "not found"' "$STATUS_JSON")
  if [[ "$status" != "done" ]]; then
    tf_error "Task $task_id is not done (status: $status)"
    return 1
  fi
  echo "=== Task $task_id Results ==="
  echo "Status: $status"
  echo "Worker: $(jq -r --arg id "$task_id" '.[$id].last_worker // "unknown"' "$STATUS_JSON")"
  local gate_output
  gate_output=$(jq -r --arg id "$task_id" '.[$id].gate_output // "none"' "$STATUS_JSON")
  [[ "$gate_output" != "none" ]] && echo "Gate Output: $gate_output"
}

init_repo() {
  rm -rf "$TF_REPO_DIR"
  mkdir -p "$TF_REPO_DIR/src"
  (cd "$TF_REPO_DIR" && git init -q && git checkout -b main -q)
  echo "fn main() {}" > "$TF_REPO_DIR/src/main.rs"
  (cd "$TF_REPO_DIR" && git add -A && git commit -q -m "init")
}

setup_tasks() {
  cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[]}
JSON
  init_repo
  tf_status_init
}

echo "=== [unit] taskfleet api ==="

# ---- tf_api_add creates task with required fields ----
tf_group_begin; tf_test "tf_api_add creates task with --task"
setup_tasks
task_id=$(tf_api_add --task "Implement feature X")
tf_assert "returns task_id" echo "$task_id" | grep -qE '^T-[0-9a-f]{8}$'
tf_assert "task added to config" jq -e ".tasks[] | select(.id == \"$task_id\")" "$TASKS_JSON" >/dev/null 2>&1
tf_group_end

# ---- tf_api_add with all options ----
tf_group_begin; tf_test "tf_api_add respects --priority --scope --accept"
setup_tasks
task_id=$(tf_api_add --task "Fix bug" --priority 10 --scope "src/lib.rs" --accept "cargo test")
tf_assert_eq "priority is 10" "10" "$(jq -r --arg id "$task_id" '.tasks[] | select(.id == $id) | .priority' "$TASKS_JSON")"
tf_assert_eq "scope contains file" "src/lib.rs" "$(jq -r --arg id "$task_id" '.tasks[] | select(.id == $id) | .scope[0]' "$TASKS_JSON")"
tf_assert_eq "accept is cargo test" "cargo test" "$(jq -r --arg id "$task_id" '.tasks[] | select(.id == $id) | .accept' "$TASKS_JSON")"
tf_group_end

# ---- tf_api_add requires --task ----
tf_group_begin; tf_test "tf_api_add fails without --task"
result=$(tf_api_add 2>&1)
tf_assert_contains "error mentions --task" "--task" "$result"
tf_group_end

# ---- tf_api_status returns specific task ----
tf_group_begin; tf_test "tf_api_status --task returns task status"
setup_tasks
tf_api_add --task "Test task" >/dev/null
# Re-init status to pick up the new task
tf_status_init
task_id=$(jq -r '.tasks[0].id' "$TASKS_JSON")
status_line=$(tf_api_status --task "$task_id")
tf_assert_contains "shows task_id" "$task_id" "$status_line"
tf_assert_contains "shows status" "ready" "$status_line"
tf_group_end

# ---- tf_api_status --json returns JSON ----
tf_group_begin; tf_test "tf_api_status --json returns valid JSON"
setup_tasks
tf_api_add --task "JSON test" >/dev/null
# Re-init status to pick up the new task
tf_status_init
task_id=$(jq -r '.tasks[0].id' "$TASKS_JSON")
json_out=$(tf_api_status --task "$task_id" --json)
tf_assert "valid JSON" jq -e '.' <<< "$json_out" >/dev/null 2>&1
tf_assert "has status field" jq -e '.status' <<< "$json_out" >/dev/null 2>&1
tf_group_end

# ---- tf_api_status for non-existent task ----
tf_group_begin; tf_test "tf_api_status returns not found for missing task"
setup_tasks
status_line=$(tf_api_status --task "NONEXISTENT")
tf_assert_contains "shows not found" "not found" "$status_line"
tf_group_end

# ---- tf_api_results for done task ----
tf_group_begin; tf_test "tf_api_results shows results for done task"
setup_tasks
tf_api_add --task "Done task" >/dev/null
# Re-init status to pick up the new task
tf_status_init
task_id=$(jq -r '.tasks[0].id' "$TASKS_JSON")
# Mark as done
tf_status_set "$task_id" done '.[$id].gate_output="success"'
results=$(tf_api_results --task "$task_id")
tf_assert_contains "shows status done" "done" "$results"
tf_assert_contains "shows gate output" "success" "$results"
tf_group_end

# ---- tf_api_results for non-done task ----
tf_group_begin; tf_test "tf_api_results fails for non-done task"
setup_tasks
tf_api_add --task "Not done" >/dev/null
# Re-init status to pick up the new task
tf_status_init
task_id=$(jq -r '.tasks[0].id' "$TASKS_JSON")
result=$(tf_api_results --task "$task_id" 2>&1)
tf_assert_contains "error mentions not done" "not done" "$result"
tf_group_end

tf_test_summary
