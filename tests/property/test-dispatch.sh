#!/usr/bin/env bash
# property/test-dispatch.sh — property-based tests for prompt rendering
# Uses generators and invariants to find edge cases in template substitution.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"
export TF_STATE_DIR="$SBOX/state"; export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"; mkdir -p "$TF_REPO_DIR"
export TF_PROMPT_DIR="$HERE/../../prompts"
git -C "$TF_REPO_DIR" init -q -b main >/dev/null 2>&1
git -C "$TF_REPO_DIR" config user.email t@t.t
echo base > "$TF_REPO_DIR/README.md"
git -C "$TF_REPO_DIR" add -A; git -C "$TF_REPO_DIR" commit -qm init

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/worktree.sh"
. "$HERE/../../lib/verify.sh"
. "$HERE/../../lib/dispatch.sh"

echo "=== [property] dispatch.sh — prompt rendering ==="

# ---- Generate tasks with random values and verify invariants ----

generate_random_tasks() {
  local n="$1"
  jq -n --argjson n "$n" '
    {tasks: [range(0; $n) | {
      id: ("TASK-\(. + 100)"),
      engine: ("engine-\(. % 5)"),
      title: ("Task Title #\(. + 100) with some description"),
      section: ("§\(. % 20)"),
      deps: (if . > 0 and . % 3 == 0 then ["TASK-\(. - 1 + 100)"] else [] end),
      scope: ["core/crates/wo-foo/src/lib.rs", "core/crates/wo-bar/src/op.rs"],
      accept: "cargo test -p wo-foo test_\(. + 100)",
      manual: false
    }]}'
}

# Property 1: tf_render_prompt always contains the task ID
tf_group_begin; tf_test "property: prompt always contains task ID"
test_prompt_has_id() {
  generate_random_tasks 1 > "$TF_CONFIG_DIR/tasks.json"
  local id
  id="$(jq -r '.tasks[0].id' "$TF_CONFIG_DIR/tasks.json")"
  STATUS_JSON="$SBOX/state/prop-id-$$.json"
  tf_status_init
  local prompt
  prompt="$(tf_render_prompt "$id")"
  [[ "$prompt" == *"$id"* ]]
}
tf_property "prompt_always_has_task_id" test_prompt_has_id 30
tf_group_end

# Property 2: tf_render_prompt always contains the title
tf_group_begin; tf_test "property: prompt always contains task title"
test_prompt_has_title() {
  generate_random_tasks 1 > "$TF_CONFIG_DIR/tasks.json"
  local id title
  id="$(jq -r '.tasks[0].id' "$TF_CONFIG_DIR/tasks.json")"
  title="$(jq -r '.tasks[0].title' "$TF_CONFIG_DIR/tasks.json")"
  STATUS_JSON="$SBOX/state/prop-title-$$.json"
  tf_status_init
  local prompt
  prompt="$(tf_render_prompt "$id")"
  [[ "$prompt" == *"$title"* ]]
}
tf_property "prompt_always_has_title" test_prompt_has_title 30
tf_group_end

# Property 3: no unfilled {{ }} placeholders remain
tf_group_begin; tf_test "property: no unfilled mustache placeholders after render"
test_no_unfilled_placeholders() {
  generate_random_tasks 1 > "$TF_CONFIG_DIR/tasks.json"
  local id
  id="$(jq -r '.tasks[0].id' "$TF_CONFIG_DIR/tasks.json")"
  STATUS_JSON="$SBOX/state/prop-placeholder-$$.json"
  tf_status_init
  local prompt
  prompt="$(tf_render_prompt "$id")"
  # Check for {{SOMETHING}} pattern
  ! grep -qP '\{\{[A-Z_]+\}\}' <<< "$prompt"
}
tf_property "no_unfilled_placeholders" test_no_unfilled_placeholders 30
tf_group_end

# Property 4: prompt always contains accept command
tf_group_begin; tf_test "property: prompt always contains accept command"
test_prompt_has_accept() {
  generate_random_tasks 1 > "$TF_CONFIG_DIR/tasks.json"
  local id accept
  id="$(jq -r '.tasks[0].id' "$TF_CONFIG_DIR/tasks.json")"
  accept="$(jq -r '.tasks[0].accept' "$TF_CONFIG_DIR/tasks.json")"
  STATUS_JSON="$SBOX/state/prop-accept-$$.json"
  tf_status_init
  local prompt
  prompt="$(tf_render_prompt "$id")"
  [[ "$prompt" == *"$accept"* ]]
}
tf_property "prompt_always_has_accept" test_prompt_has_accept 30
tf_group_end

# Property 5: prompt always contains scope files
tf_group_begin; tf_test "property: prompt always contains scope files"
test_prompt_has_scope() {
  generate_random_tasks 1 > "$TF_CONFIG_DIR/tasks.json"
  local id scope
  id="$(jq -r '.tasks[0].id' "$TF_CONFIG_DIR/tasks.json")"
  scope="$(jq -r '.tasks[0].scope[0]' "$TF_CONFIG_DIR/tasks.json")"
  STATUS_JSON="$SBOX/state/prop-scope-$$.json"
  tf_status_init
  local prompt
  prompt="$(tf_render_prompt "$id")"
  [[ "$prompt" == *"$scope"* ]]
}
tf_property "prompt_always_has_scope" test_prompt_has_scope 30
tf_group_end

# Property 6: tf_render_prompt is deterministic (same input → same output)
tf_group_begin; tf_test "property: prompt rendering is deterministic"
STATUS_JSON="$SBOX/state/prop-det.json"
generate_random_tasks 1 > "$TF_CONFIG_DIR/tasks.json"
local_id="$(jq -r '.tasks[0].id' "$TF_CONFIG_DIR/tasks.json")"
tf_status_init
p1="$(tf_render_prompt "$local_id")"
p2="$(tf_render_prompt "$local_id")"
tf_assert_eq "deterministic" "$p1" "$p2"
tf_group_end

# Property 7: retry prompt contains PREVIOUS_ERROR section
tf_group_begin; tf_test "property: retry prompt (attempt > 0) contains error feedback"
STATUS_JSON="$SBOX/state/prop-retry.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[{"id":"RETRY1","engine":"t","title":"retry task","section":"§1","deps":[],"scope":["a.rs"],"accept":"false","manual":false}]}
JSON
tf_status_init
# Simulate a failure with error classification
jq -n '{category:"compile_error",summary:"mismatched types",classified_at:"now"}' > "$TF_LOG_DIR/RETRY1.error.json"
echo 'error[E0308]: mismatched types' > "$TF_LOG_DIR/RETRY1.verify.log"
# Set attempts > 0 to trigger retry logic
tmp="$(mktemp)"
jq '.RETRY1.attempts=1' "$STATUS_JSON" > "$tmp"; mv "$tmp" "$STATUS_JSON"

prompt="$(tf_render_prompt RETRY1)"
tf_assert_contains "has RETRY header" "RETRY" "$prompt"
tf_assert_contains "has error category" "compile_error" "$prompt"
tf_assert_contains "has error summary" "mismatched types" "$prompt"
tf_group_end

# Property 8: prompt does NOT contain PREVIOUS_ERROR on first attempt
tf_group_begin; tf_test "first attempt has no PREVIOUS_ERROR section"
STATUS_JSON="$SBOX/state/prop-first.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[{"id":"FIRST1","engine":"t","title":"first task","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}]}
JSON
tf_status_init
prompt="$(tf_render_prompt FIRST1)"
tf_assert_not_contains "no retry section" "RETRY" "$prompt"
tf_assert_not_contains "no PREVIOUS_ERROR" "⚠️" "$prompt"
tf_group_end

tf_test_summary
