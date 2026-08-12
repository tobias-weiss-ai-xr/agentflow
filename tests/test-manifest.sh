#!/usr/bin/env bash
# test-manifest.sh — validate tasks.json + workers.json.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/test-harness.sh"
# point at real config (overridable)
export TF_CONFIG_DIR="${TF_CONFIG_DIR:-$HERE/../config}"
. "$HERE/../lib/common.sh"

echo "=== Manifest tests ==="

tf_group_begin; tf_test "enabled workers resolve via tf_worker_names"
enabled_n="$(jq '[.workers[]|select(.enabled)]|length' "$WORKERS_JSON")"
tf_assert_eq "enabled worker count" "$enabled_n" "$(tf_worker_names | wc -l)"
tf_assert "zai worker present" grep -qxF zai < <(tf_worker_names)
tf_assert "tud worker present" grep -qxF tud < <(tf_worker_names)
tf_assert "local-flash worker present" grep -qxF local-flash < <(tf_worker_names)
tf_group_end

tf_group_begin; tf_test "worker model ids match pi config"
tf_assert_eq "zai model"        "glm-5-turbo"            "$(tf_worker_field zai .model)"
tf_assert_eq "tud model"        "Mistral-Medium-3.5-128B" "$(tf_worker_field tud .model)"
tf_assert_eq "local-flash model" "deepseek-v4-flash/local" "$(tf_worker_field local-flash .model)"
tf_group_end

tf_group_begin; tf_test "worker providers match pi config"
tf_assert_eq "zai provider"        "zai"          "$(tf_worker_field zai .provider)"
tf_assert_eq "tud provider"        "tud"          "$(tf_worker_field tud .provider)"
tf_assert_eq "local-flash provider" "litellm"     "$(tf_worker_field local-flash .provider)"
tf_group_end

tf_group_begin; tf_test "tasks.json: 69 tasks, 11 engines, zero dangling deps"
tf_assert_eq "task count" "69" "$(jq '.tasks|length' "$TASKS_JSON")"
tf_assert_eq "engine count" "11" "$(jq '.tasks|map(.engine)|unique|length' "$TASKS_JSON")"
# dangling deps = deps referencing non-existent ids
dang="$(jq -r '.tasks as $ts | ($ts|map(.id)) as $ids |
  [ $ts[] | .id as $tid | .deps[] | select(. as $d | $ids | index($d) | not) | "\($tid)→\(.)" ] |
  (if length==0 then "none" else join(", ") end)' "$TASKS_JSON")"
tf_assert_eq "no dangling deps" "none" "$dang"
tf_group_end

tf_group_begin; tf_test "every task has non-empty id/title/engine/accept/scope"
bad="$(jq -r '.tasks[] | select((.id//"")=="" or (.title//"")=="" or (.engine//"")=="" or (.accept//"")=="" or (.scope|length)==0) | .id' "$TASKS_JSON")"
tf_assert_eq "all tasks complete" "" "$bad"
tf_group_end

tf_group_begin; tf_test "foundation tasks FC-1..4 present with correct deps"
tf_assert_eq "FC-1 deps" "[]"          "$(jq -c '.tasks[]|select(.id=="FC-1")|.deps' "$TASKS_JSON")"
tf_assert_eq "FC-2 deps" '["FC-1"]'    "$(jq -c '.tasks[]|select(.id=="FC-2")|.deps' "$TASKS_JSON")"
tf_assert_eq "FC-4 deps" "[]"          "$(jq -c '.tasks[]|select(.id=="FC-4")|.deps' "$TASKS_JSON")"
tf_assert_eq "DM-1 deps" '["DM-0"]'    "$(jq -c '.tasks[]|select(.id=="DM-1")|.deps' "$TASKS_JSON")"
tf_assert_eq "DM-9 deps expanded" '["DM-2","DM-3","DM-4","DM-5","DM-6","DM-7","DM-8"]' "$(jq -c '.tasks[]|select(.id=="DM-9")|.deps' "$TASKS_JSON")"
tf_group_end

tf_test_summary
