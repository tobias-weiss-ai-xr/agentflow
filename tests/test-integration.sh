#!/usr/bin/env bash
# test-integration.sh — full tf_dispatch_one pipeline end-to-end with a STUB pi.
#
# Proves: worktree create → pi invoked in worktree → worker prompt rendered →
# accept gate run → status set → merge to main → branch/worktree cleanup.
# Uses a fake `pi` on PATH so we exercise the harness wiring, not the LLM.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_REPO_DIR="$SBOX/repo"
export TF_WORKTREE_ROOT="$TF_REPO_DIR/.tf-worktrees"
export TF_STATE_DIR="$SBOX/state"; export TF_LOG_DIR="$SBOX/logs"
export TF_BRANCH_PREFIX="agent"
export TF_MAX_PARALLEL=1
mkdir -p "$TF_REPO_DIR/.tf-worktrees" "$TF_STATE_DIR/logs"

git -C "$TF_REPO_DIR" init -q -b main
git -C "$TF_REPO_DIR" config user.email t@t.t; git -C "$TF_REPO_DIR" config user.name test
echo base > "$TF_REPO_DIR/README.md"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm init

export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"
echo '{"workers":[{"name":"stub","provider":"p","model":"m","enabled":true}],
       "defaults":{"max_attempts":2,"retry_cooldown_s":1,"accept_timeout_s":30,"dispatch_timeout_s":60}}' \
  > "$TF_CONFIG_DIR/workers.json"

# A fake pi: it "implements" the task by writing the file named in the prompt's
# scope block, then commits. We embed the scope→filename mapping in tasks.json.
# The stub parses the prompt file (passed as @path) to find the scope file.
STUBBIN="$SBOX/bin"; mkdir -p "$STUBBIN"
cat > "$STUBBIN/pi" <<'STUB'
#!/usr/bin/env bash
# Minimal fake pi headless: ignores --provider/--model/-p, reads prompt file.
set -uo pipefail
prompt_file=""
for a in "$@"; do
  case "$a" in @*) prompt_file="${a:1}" ;; esac
done
[[ -z "$prompt_file" ]] && { echo "stub: no prompt file" >&2; exit 1; }
scope_file="$(grep -oE '[a-z_]+\.rs' "$prompt_file" | head -1)"
task_id="$(grep -oE 'Worker Task: [A-Z0-9-]+' "$prompt_file" | awk '{print $3}')"
echo "STUB pi: task=$task_id scope=$scope_file"
case "$task_id" in
  GOOD)
    echo "// impl" > "$scope_file"       # GOOD writes its file → gate `test -f a.rs` passes
    git add -A && git commit -qm "feat($task_id): stub impl" ;;
  BAD)
    : ;;                                # BAD writes nothing → gate `test -f b.rs` fails
esac
exit 0
STUB
chmod +x "$STUBBIN/pi"
export PATH="$STUBBIN:$PATH"

# Worker prompt dir: copy the real template
export TF_PROMPT_DIR="$HERE/../prompts"

. "$HERE/../lib/common.sh"
. "$HERE/../lib/status.sh"
. "$HERE/../lib/worktree.sh"
. "$HERE/../lib/verify.sh"
. "$HERE/../lib/dispatch.sh"

# Both tasks defined upfront with file-based gates (clean isolation):
#   GOOD writes a.rs → `test -f a.rs` passes
#   BAD  writes nothing → `test -f b.rs` fails
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"_meta":{"task_count":2},"tasks":[
{"id":"GOOD","engine":"t","title":"good task","section":"§1","deps":[],
 "scope":["a.rs"],"accept":"test -f a.rs","manual":false},
{"id":"BAD","engine":"t","title":"bad task","section":"§1","deps":[],
 "scope":["b.rs"],"accept":"test -f b.rs","manual":false}
]}
JSON
tf_status_init

echo "=== Integration: full dispatch_one pipeline (stub pi) ==="

tf_group_begin; tf_test "GOOD: dispatch → gate PASS → merge → done"
tf_dispatch_one GOOD stub
tf_assert_eq "status done"  "done" "$(tf_status_get GOOD .status)"
tf_assert "a.rs merged to main"    git -C "$TF_REPO_DIR" show main:a.rs >/dev/null
tf_assert_not "worktree removed"   test -d "$TF_REPO_DIR/.tf-worktrees/GOOD"
tf_assert_not "branch deleted"     git -C "$TF_REPO_DIR" rev-parse --verify --quiet agent/GOOD
tf_group_end

tf_group_begin; tf_test "BAD: dispatch → gate FAIL → status failed (attempt 1)"
tf_dispatch_one BAD stub
tf_assert_eq "status failed" "failed" "$(tf_status_get BAD .status)"
tf_assert_eq "attempts incremented" "1" "$(tf_status_get BAD .attempts)"
tf_assert_not "b.rs absent from main"   git -C "$TF_REPO_DIR" cat-file -e main:b.rs
tf_assert_not "worktree cleaned up"  test -d "$TF_REPO_DIR/.tf-worktrees/BAD"
tf_group_end

tf_test_summary
