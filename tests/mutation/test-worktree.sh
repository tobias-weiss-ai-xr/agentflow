#!/usr/bin/env bash
# mutation/test-worktree.sh — MUTATION TESTING for lib/worktree.sh
#
# Injects bugs into worktree lifecycle (create/merge/remove/delete_branch)
# and verifies the test suite kills them.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
TF_DIR="$(cd "$HERE/../.." && pwd)"

cat > "$SBOX/runner.sh" <<'RUNNER'
#!/usr/bin/env bash
set -uo pipefail
SRC="$1"
S="$2"
mkdir -p "$S/config" "$S/state" "$S/logs" "$S/repo"
cd "$S/repo" || exit 1
git init -q -b main 2>/dev/null
git config user.email t@t.t; git config user.name test
echo base > README.md; git add -A; git commit -qm init

export TF_CONFIG_DIR="$S/config" TF_STATE_DIR="$S/state" TF_LOG_DIR="$S/logs"
export TF_REPO_DIR="$S/repo" TF_BRANCH_PREFIX="agent" TF_WORKTREE_ROOT="$S/repo/.tf-worktrees"
export TF_MERGE_LOCK="$S/state/merge.lock"
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"T1","engine":"t","title":"T1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
. "$SRC/lib/common.sh"
. "$SRC/lib/worktree.sh"

# 1. create worktree + branch
WT="$(tf_worktree_create T1)"
[[ -d "$WT" ]] || exit 1
git -C "$S/repo" rev-parse --verify agent/T1 >/dev/null 2>&1 || exit 1

# 2. edit + commit in worktree
echo "work" > "$WT/a.rs"
(cd "$WT" && git add -A && git commit -qm "feat(T1): work") || exit 1

# 3. merge brings commit to main
tf_worktree_merge T1 || exit 1
git -C "$S/repo" log --oneline main | grep -q "feat(T1)" || exit 1

# 4. remove deletes worktree dir
tf_worktree_remove T1 || exit 1
[[ ! -d "$WT" ]] || exit 1

# 5. delete_branch removes branch
tf_worktree_delete_branch T1
git -C "$S/repo" rev-parse --verify agent/T1 >/dev/null 2>&1 && exit 1

# 6. gitignore for in-repo worktrees
grep -q ".tf-worktrees" "$S/repo/.gitignore" || exit 1

exit 0
RUNNER
chmod +x "$SBOX/runner.sh"

echo "=== [mutation] lib/worktree.sh ==="
tf_seed_init
TF_MUT_SRC="$TF_DIR"

tf_group_begin; tf_test "mutations of lib/worktree.sh are killed by lifecycle tests"
tf_mutation_test "worktree" "lib/worktree.sh" \
  "bash $SBOX/runner.sh __MUTDIR__ $SBOX/work" \
  's/worktree add -b/worktree add -B/' \
  's/--force/--no-force/' \
  's/git merge --ff-only/git merge --no-ff/' \
  's/git reset --hard --quiet main/git reset --soft --quiet main/' \
  's/rm -rf "$wt"/rm -rf "$wt.bak"/' \
  's/git branch -D/git branch -d/' \
  's/delete_branch/keep_branch/'
tf_group_end

tf_group_begin; tf_test "worktree mutation score"
tf_mutation_report
tf_assert_gt "worktree mutation score > 5" "5" "$TF_MUT_KILLED"
tf_group_end

tf_test_summary
