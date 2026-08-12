#!/usr/bin/env bash
# stress/test-concurrent.sh — stress tests for concurrent operations
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_REPO_DIR="$SBOX/repo"
export TF_WORKTREE_ROOT="$SBOX/repo/.tf-worktrees"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_BRANCH_PREFIX="agent"
export TF_PROMPT_DIR="$HERE/../../prompts"
mkdir -p "$TF_REPO_DIR" "$TF_WORKTREE_ROOT" "$TF_STATE_DIR" "$TF_LOG_DIR"

git -C "$TF_REPO_DIR" init -q -b main
git -C "$TF_REPO_DIR" config user.email t@t.t
git -C "$TF_REPO_DIR" config user.name test
echo base > "$TF_REPO_DIR/README.md"
git -C "$TF_REPO_DIR" add -A && git -C "$TF_REPO_DIR" commit -qm init

export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":2,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/worktree.sh"
. "$HERE/../../lib/verify.sh"

echo "=== [stress] concurrent operations ==="

# Stress 1: 10 sequential worktree creations + removals (correctness under reuse)
tf_group_begin; tf_test "10 worktree create-commit-remove cycles don't corrupt state"
STATUS_JSON="$SBOX/state/stress-create.json"
ok=0
for i in $(seq 1 10); do
  id="S$i"
  cat > "$TF_CONFIG_DIR/tasks.json" <<JSON
{"tasks":[{"id":"$id","engine":"t","title":"Stress $i","section":"§1","deps":[],"scope":["stress$i.txt"],"accept":"true","manual":false}]}
JSON
  WT="$(tf_worktree_create "$id" 2>/dev/null)"
  if [[ -d "$WT" ]]; then
    echo "content-$i" > "$WT/stress$i.txt"
    git -C "$WT" add -A && git -C "$WT" commit -qm "feat($id): stress$i" 2>/dev/null
    tf_worktree_remove "$id" 2>/dev/null
    ok=$((ok + 1))
  fi
done
tf_assert_eq "all 10 created and removed" "10" "$ok"
# Cleanup branches
for i in $(seq 1 10); do
  tf_worktree_delete_branch "S$i" 2>/dev/null
done
tf_group_end

# Stress 2: parallel status writes (lock contention)
tf_group_begin; tf_test "100 parallel status writes don't corrupt JSON"
STATUS_JSON="$SBOX/state/parallel-status.json"
echo '{"tasks":[{"id":"P1","engine":"t","title":"P1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false}]}' > "$TF_CONFIG_DIR/tasks.json"
tf_status_init >/dev/null 2>&1
pids=()
for i in $(seq 1 100); do
  (set +u; tf_status_set P1 running 2>/dev/null) &
  pids+=($!)
done
for pid in "${pids[@]}"; do wait "$pid"; done
tf_assert "status JSON is valid" jq -e . "$STATUS_JSON" >/dev/null
tf_assert "attempts field is a number" \
  jq -e '.P1.attempts | type == "number"' "$STATUS_JSON" >/dev/null
tf_group_end

# Stress 3: sequential merges (tests merge correctness after worktree remove)
tf_group_begin; tf_test "10 sequential merges all succeed"
STATUS_JSON="$SBOX/state/parallel-merge.json"
for i in $(seq 1 10); do
  id="M$i"
  cat > "$TF_CONFIG_DIR/tasks.json" <<JSON
{"tasks":[{"id":"$id","engine":"t","title":"Merge $i","section":"§1","deps":[],"scope":["file$i.txt"],"accept":"true","manual":false}]}
JSON
  WT="$(tf_worktree_create "$id" 2>/dev/null)"
  echo "content-$i" > "$WT/file$i.txt"
  git -C "$WT" add -A && git -C "$WT" commit -qm "feat($id): file$i"
  tf_worktree_remove "$id" 2>/dev/null
done

failed=0
for i in $(seq 1 10); do
  tf_worktree_merge "M$i" >/dev/null 2>&1 || failed=$((failed + 1))
  tf_worktree_delete_branch "M$i" 2>/dev/null
done
tf_assert_eq "all 10 merged" "0" "$failed"

for i in $(seq 1 10); do
  tf_assert "file$i.txt on main" git -C "$TF_REPO_DIR" show "main:file$i.txt" >/dev/null
done
tf_group_end

# Stress 4: rapid create-commit-merge-remove cycles (end-to-end pipeline)
tf_group_begin; tf_test "20 rapid create-commit-merge-remove cycles"
STATUS_JSON="$SBOX/state/rapid-cycles.json"
count=0
for i in $(seq 1 20); do
  id="RC$i"
  cat > "$TF_CONFIG_DIR/tasks.json" <<JSON
{"tasks":[{"id":"$id","engine":"t","title":"Rapid $i","section":"§1","deps":[],"scope":["rapid$i.txt"],"accept":"true","manual":false}]}
JSON
  WT="$(tf_worktree_create "$id" 2>/dev/null)"
  echo "rapid-$i" > "$WT/rapid$i.txt"
  git -C "$WT" add -A && git -C "$WT" commit -qm "feat($id): rapid$i"
  tf_worktree_remove "$id" 2>/dev/null
  tf_worktree_merge "$id" >/dev/null 2>&1
  tf_worktree_delete_branch "$id" 2>/dev/null
done
for i in $(seq 1 20); do
  git -C "$TF_REPO_DIR" show "main:rapid$i.txt" >/dev/null 2>&1 && count=$((count + 1))
done
tf_assert_eq "all 20 files on main" "20" "$count"
tf_group_end

tf_test_summary
