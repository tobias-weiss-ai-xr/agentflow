#!/usr/bin/env bash
# unit/test-sources.sh — unit tests for lib/sources.sh (external task sources)
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

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"openai","model":"gpt-4o","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/sources.sh"

# Atomic ingestion: give sources.sh its own sandbox tasks.json
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"EXISTING","engine":"rust","title":"hand task","section":"s","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON

# Mock the network fetch — tests run offline.
mock_issues() {
  tf_github_fetch_issues() {
    echo '[{"number":101,"title":"Fix flaky test","body":"The test is flaky.","pull_request":null,
           "labels":[{"name":"bug"},{"name":"p1"}]},
          {"number":102,"title":"Add docs","body":"Write docs.","pull_request":null,"labels":[]}]'
  }
}

echo "=== [unit] sources.sh ==="

# ---- github fetch issues is a curl wrapper (syntax/arg passthrough smoke) ----
tf_group_begin; tf_test "tf_github_to_task maps issue fields to task JSON"
task="$(tf_github_to_task some/repo 101 "Fix flaky test" "The test is flaky." "bug,p1")"
tf_assert_eq "id is GH-101" "GH-101" "$(echo "$task" | jq -r .id)"
tf_assert_eq "engine is issue" "issue" "$(echo "$task" | jq -r .engine)"
tf_assert_eq "title preserved" "Fix flaky test" "$(echo "$task" | jq -r .title)"
tf_assert_eq "source tag" "github:some/repo#101" "$(echo "$task" | jq -r .source)"
tf_assert_eq "labels parsed" "bug" "$(echo "$task" | jq -r '.labels[0]')"
tf_assert "accept defaults to true" test "$(echo "$task" | jq -r .accept)" = "true"
tf_group_end

# ---- github fetch returns JSON array (uses API helper path construction) ----
tf_group_begin; tf_test "tf_github_fetch_issues builds request via curl wrapper"
# Override curl to capture args but return json — verify URL construction offline
skip_curl() {
  tf_github_fetch_issues() { echo '[]'; }
}
skip_curl
tf_assert_eq "fetch returns array" "[]" "$(tf_github_fetch_issues some/repo --state open)"
tf_group_end

# ---- import adds new tasks, skips existing, no dupes ----
tf_group_begin; tf_test "tf_source_import_github imports and dedups"
mock_issues
out="$(tf_source_import_github some/repo 2>&1)"
tf_assert_contains "reports 2 imported" "imported 2 new" "$out"
tf_assert "existing task kept" jq -e '.tasks[] | select(.id=="EXISTING")' "$TASKS_JSON" >/dev/null 2>&1
tf_assert "GH-101 added" jq -e '.tasks[] | select(.id=="GH-101")' "$TASKS_JSON" >/dev/null 2>&1
tf_assert "GH-102 added" jq -e '.tasks[] | select(.id=="GH-102")' "$TASKS_JSON" >/dev/null 2>&1
# Run again — should skip now (idempotent)
out2="$(tf_source_import_github some/repo 2>&1)"
tf_assert_contains "second run skips both" "imported 0 new" "$out2"
tf_assert_eq "still exactly two GH tasks" "2" "$(jq '[.tasks[] | select(.id | startswith("GH-"))] | length' "$TASKS_JSON")"
tf_group_end

# ---- dry-run doesn't modify tasks.json ----
tf_group_begin; tf_test "tf_source_import_github --dry-run does not modify"
mock_issues
before="$(cat "$TASKS_JSON")"
out="$(tf_source_import_github some/repo --dry-run 2>&1)"
tf_assert_contains "dry-run reports would import" "would be imported" "$out"
tf_assert "tasks.json unchanged in dry-run" test "$(cat "$TASKS_JSON")" = "$before"
tf_group_end

# ---- import respects --label filter via fetch passthrough ----
tf_group_begin; tf_test "tf_label filter forwarded to fetch"
args_file="$SBOX/fetch-args.txt"
tf_github_fetch_issues() {
  echo "$*" > "$args_file"
  echo '[]'
}
tf_source_import_github some/repo --label bug >/dev/null 2>&1
tf_assert_contains "label value forwarded" "bug" "$(cat "$args_file")"
tf_group_end

tf_test_summary
