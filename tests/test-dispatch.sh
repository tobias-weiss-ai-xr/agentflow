#!/usr/bin/env bash
# test-dispatch.sh — prompt rendering + dry-run dispatch (no pi invocation).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/test-harness.sh"

# Isolated sandbox; reuse REAL tasks.json/workers.json for fidelity
SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"
cp "$HERE/../config/tasks.json" "$TF_CONFIG_DIR/tasks.json"
cp "$HERE/../config/workers.json" "$TF_CONFIG_DIR/workers.json"
export TF_STATE_DIR="$SBOX/state"; export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"; mkdir -p "$TF_REPO_DIR"
git -C "$TF_REPO_DIR" init -q -b main >/dev/null 2>&1
git -C "$TF_REPO_DIR" config user.email t@t.t; git -C "$TF_REPO_DIR" config user.name test
echo base > "$TF_REPO_DIR/README.md"; git -C "$TF_REPO_DIR" add -A; git -C "$TF_REPO_DIR" commit -qm init

. "$HERE/../lib/common.sh"
. "$HERE/../lib/status.sh"
. "$HERE/../lib/worktree.sh"
. "$HERE/../lib/verify.sh"
. "$HERE/../lib/dispatch.sh"

echo "=== Dispatch / prompt-render tests ==="

tf_group_begin; tf_test "tf_render_prompt fills all placeholders for FC-1"
prompt="$(tf_render_prompt FC-1)"
tf_assert "contains task id FC-1"     grep -q "FC-1" <<< "$prompt"
tf_assert "contains title (Path)"     grep -q "Path" <<< "$prompt"
tf_assert "contains engine foundation" grep -q "foundation" <<< "$prompt"
tf_assert "contains scope file path.rs" grep -q "path.rs" <<< "$prompt"
tf_assert "contains accept command"   grep -q "cargo test -p wo-common path::" <<< "$prompt"
tf_assert "contains commit message"   grep -q "feat(FC-1)" <<< "$prompt"
# no unfilled placeholders left
tf_assert_not "no unfilled {{ }} placeholders" grep -q "{{" <<< "$prompt"
tf_group_end

tf_group_begin; tf_test "tf_render_prompt for a multi-dep task (DM-9)"
prompt="$(tf_render_prompt DM-9)"
tf_assert "contains DM-9 id"          grep -q "DM-9" <<< "$prompt"
tf_assert "contains editable_model scope" grep -q "model.rs" <<< "$prompt"
tf_assert_not "no unfilled placeholders" grep -q "{{" <<< "$prompt"
tf_group_end

tf_group_begin; tf_test "tf_dispatch_one_dryrun prints plan without running pi"
out="$(tf_dispatch_one_dryrun DM-3 zai 2>/dev/null)"
tf_assert "names worker zai"          grep -q "worker=zai" <<< "$out"
tf_assert "names provider"            grep -q "zai/" <<< "$out"
tf_assert "shows accept command"      grep -q "cargo test -p wo-ooxml-ops" <<< "$out"
tf_group_end

tf_group_begin; tf_test "worker lookup resolves enabled workers only"
names="$(tf_worker_names)"
enabled_n="$(jq '[.workers[]|select(.enabled)]|length' "$WORKERS_JSON")"
tf_assert_eq "enabled worker count" "$enabled_n" "$(echo "$names" | wc -l)"
tf_assert "zai present"    grep -qxF zai <<< "$names"
tf_assert "local-flash present" grep -qxF local-flash <<< "$names"
# disabled worker would be filtered
tf_assert_eq "zai model" "glm-5-turbo" "$(tf_worker_field zai .model)"
tf_assert_eq "zai endpoint" "https://api.z.ai/api/coding/paas/v4" "$(tf_worker_field zai .endpoint)"
tf_group_end

tf_test_summary
