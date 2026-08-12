#!/usr/bin/env bash
# test-verify.sh — acceptance-gate runner: PASS, FAIL, TIMEOUT, scope-drift.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_REPO_DIR="$SBOX/repo"; export TF_WORKTREE_ROOT="$TF_REPO_DIR/.tf-worktrees"
export TF_STATE_DIR="$SBOX/state"; export TF_LOG_DIR="$SBOX/logs"
export TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_REPO_DIR/.tf-worktrees" "$TF_STATE_DIR" "$TF_LOG_DIR"
git -C "$TF_REPO_DIR" init -q -b main
git -C "$TF_REPO_DIR" config user.email t@t.t; git -C "$TF_REPO_DIR" config user.name test
echo base > "$TF_REPO_DIR/README.md"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm init

export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"
echo '{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
       "defaults":{"max_attempts":2,"retry_cooldown_s":1,"accept_timeout_s":3}}' \
  > "$TF_CONFIG_DIR/workers.json"
# three tasks: passing, failing, slow (timeout)
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"_meta":{"task_count":3},"tasks":[
{"id":"PASS","engine":"t","title":"passes","section":"§1","deps":[],"scope":["a.rs"],"accept":"test -f marker","manual":false},
{"id":"FAIL","engine":"t","title":"fails","section":"§1","deps":[],"scope":["b.rs"],"accept":"false","manual":false},
{"id":"SLOW","engine":"t","title":"slow","section":"§1","deps":[],"scope":["c.rs"],"accept":"sleep 10","manual":false},
{"id":"NOSCOPE","engine":"t","title":"no-accept","section":"§1","deps":[],"scope":["d.rs"],"accept":"","manual":false}
]}
JSON

. "$HERE/../lib/common.sh"
. "$HERE/../lib/status.sh"
. "$HERE/../lib/worktree.sh"
. "$HERE/../lib/verify.sh"

echo "=== Verify / acceptance-gate tests ==="

tf_group_begin; tf_test "PASS: accept command exits 0 → verdict PASS"
WT="$(tf_worktree_create PASS)"
touch "$WT/marker"   # so `test -f marker` succeeds
v="$(tf_verify PASS "$WT")"
tf_assert_eq "verdict" "PASS" "$v"
tf_worktree_remove PASS
tf_group_end

tf_group_begin; tf_test "FAIL: accept command exits non-zero → verdict FAIL"
WT="$(tf_worktree_create FAIL)"
v="$(tf_verify FAIL "$WT")" || true
tf_assert "fail verdict starts with 'FAIL: exit '" test "${v#FAIL: exit }" != "$v"
tf_worktree_remove FAIL
tf_group_end

tf_group_begin; tf_test "TIMEOUT: accept command exceeds accept_timeout_s → verdict FAIL timeout"
WT="$(tf_worktree_create SLOW)"
v="$(tf_verify SLOW "$WT")" || true
tf_assert_eq "timeout verdict" "FAIL: timeout after 3s" "$v"
tf_worktree_remove SLOW
tf_group_end

tf_group_begin; tf_test "SKIP: empty accept command → manual sign-off (SKIP, exit 0)"
WT="$(tf_worktree_create NOSCOPE)"
v="$(tf_verify NOSCOPE "$WT")"
tf_assert_eq "verdict" "SKIP: no accept command (manual task)" "$v"
tf_worktree_remove NOSCOPE
tf_group_end

tf_group_begin; tf_test "scope-drift check flags out-of-scope edits (advisory)"
# Re-scope PASS task to only allow a.rs, then commit an edit to z.rs in its branch
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"_meta":{"task_count":1},"tasks":[
{"id":"PASS","engine":"t","title":"passes","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}]}
JSON
WT="$(tf_worktree_create PASS)"
echo impl > "$WT/a.rs"; echo drift > "$WT/z.rs"
git -C "$WT" add -A && git -C "$WT" commit -qm "edits"
drift="$(tf_verify_scope PASS "$WT")"
tf_assert_eq "z.rs flagged as out-of-scope" "z.rs" "$drift"
tf_worktree_remove PASS
tf_group_end

tf_test_summary
