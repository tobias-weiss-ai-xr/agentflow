#!/usr/bin/env bash
# stress/test-concurrent.sh — stress tests for concurrent dispatch + merge
# Tests race conditions, lock contention, and parallel worktree operations.
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

# Stress 1: parallel worktree creation
tf_group_begin; tf_test "10 parallel worktree creations don't corrupt state"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"S1","engine":"t","title":"S1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
  {"id":"S2","engine":"t","title":"S2","section":"§1","deps":[],"scope":["b.rs"],"accept":"true","manual":false},
  {"id":"S3","engine":"t","title":"S3","section":"§1","deps":[],"scope":["c.rs"],"accept":"true","manual":false},
  {"id":"S4","engine":"t","title":"S4","section":"§1","deps":[],"scope":["d.rs"],"accept":"true","manual":false},
  {"id":"S5","engine":"t","title":"S5","section":"§1","deps":[],"scope":["e.rs"],"accept":"true","manual":false},
  {"id":"S6","engine":"t","title":"S6","section":"§1","deps":[],"scope":["f.rs"],"accept":"true","manual":false},
  {"id":"S7","engine":"t","title":"S7","section":"§1","deps":[],"scope":["g.rs"],"accept":"true","manual":false},
  {"id":"S8","engine":"t","title":"S8","section":"§1","deps":[],"scope":["h.rs"],"accept":"true","manual":false},
  {"id":"S9","engine":"t","title":"S9","section":"§1","deps":[],"scope":["i.rs"],"accept":"true","manual":false},
  {"id":"S10","engine":"t","title":"S10","section":"§1","deps":[],"scope":["j.rs"],"accept":"true","manual":false}
]}
JSON
STATUS_JSON="$SBOX/state/parallel-create.json"
tf_status_init

pids=()
for id in S1 S2 S3 S4 S5 S6 S7 S8 S9 S10; do
  (tf_worktree_create "$id" >/dev/null && echo "$id:ok" > "$TF_LOG_DIR/create-$id.result" || echo "$id:fail" > "$TF_LOG_DIR/create-$id.result") &
  pids+=($!)
done
for pid in "${pids[@]}"; do wait "$pid"; done

failed=0
for id in S1 S2 S3 S4 S5 S6 S7 S8 S9 S10; do
  r="$(cat "$TF_LOG_DIR/create-$id.result")"
  if [[ "$r" != "${id}:ok" ]]; then
    echo "    \033[31mBAD\033[0m  $id: $r"
    failed=$((failed + 1))
    TF_GROUP_FAILED=1
  fi
  tf_worktree_remove "$id" 2>/dev/null
done
tf_assert_eq "all 10 created" "0" "$failed"
tf_group_end

# Stress 2: parallel status writes (lock contention)
tf_group_begin; tf_test "100 parallel status writes don't corrupt JSON"
STATUS_JSON="$SBOX/state/parallel-status.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"P1","engine":"t","title":"P1","section":"§1","deps":[],"scope":["a.rs"],"accept":"true","manual":false}
]}
JSON
tf_status_init

pids=()
for i in $(seq 1 100); do
  (tf_fail_task P1 "stress-write-$i" 2>/dev/null) &
  pids+=($!)
done
for pid in "${pids[@]}"; do wait "$pid"; done

# JSON must still be valid
tf_assert "status JSON is valid" jq -e . "$STATUS_JSON" >/dev/null
tf_assert "attempts field is a number" \
  jq -e '.P1.attempts | type == "number"' "$STATUS_JSON" >/dev/null
tf_group_end

# Stress 3: parallel merges (serialization via merge lock)
tf_group_begin; tf_test "10 parallel merges all succeed via merge lock"
STATUS_JSON="$SBOX/state/parallel-merge.json"
# First, create and commit in each worktree sequentially
for i in $(seq 1 10); do
  id="M$i"
  cat > "$TF_CONFIG_DIR/tasks.json" <<JSON
{"tasks":[{"id":"$id","engine":"t","title":"Merge $i","section":"§1","deps":[],"scope":["file$i.txt"],"accept":"true","manual":false}]}
JSON
  WT="$(tf_worktree_create "$id")"
  echo "content-$i" > "$WT/file$i.txt"
  git -C "$WT" add -A && git -C "$WT" commit -qm "feat($id): file$i"
  tf_worktree_remove "$id"
done

# Now merge all in parallel
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"M1","engine":"t","title":"M1","section":"§1","deps":[],"scope":["file1.txt"],"accept":"true","manual":false},
  {"id":"M2","engine":"t","title":"M2","section":"§1","deps":[],"scope":["file2.txt"],"accept":"true","manual":false},
  {"id":"M3","engine":"t","title":"M3","section":"§1","deps":[],"scope":["file3.txt"],"accept":"true","manual":false},
  {"id":"M4","engine":"t","title":"M4","section":"§1","deps":[],"scope":["file4.txt"],"accept":"true","manual":false},
  {"id":"M5","engine":"t","title":"M5","section":"§1","deps":[],"scope":["file5.txt"],"accept":"true","manual":false},
  {"id":"M6","engine":"t","title":"M6","section":"§1","deps":[],"scope":["file6.txt"],"accept":"true","manual":false},
  {"id":"M7","engine":"t","title":"M7","section":"§1","deps":[],"scope":["file7.txt"],"accept":"true","manual":false},
  {"id":"M8","engine":"t","title":"M8","section":"§1","deps":[],"scope":["file8.txt"],"accept":"true","manual":false},
  {"id":"M9","engine":"t","title":"M9","section":"§1","deps":[],"scope":["file9.txt"],"accept":"true","manual":false},
  {"id":"M10","engine":"t","title":"M10","section":"§1","deps":[],"scope":["file10.txt"],"accept":"true","manual":false}
]}
JSON

pids=()
for i in $(seq 1 10); do
  (tf_worktree_merge "M$i" 2>/dev/null && echo "M$i:ok" > "$TF_LOG_DIR/merge-$i.result" || echo "M$i:fail" > "$TF_LOG_DIR/merge-$i.result") &
  pids+=($!)
done
for pid in "${pids[@]}"; do wait "$pid"; done

failed=0
for i in $(seq 1 10); do
  r="$(cat "$TF_LOG_DIR/merge-M$i.result" 2>/dev/null || echo "M$i:missing")"
  if [[ "$r" != "M$i:ok" ]]; then
    echo "    \033[31mBAD\033[0m  M$i: $r"
    failed=$((failed + 1))
    TF_GROUP_FAILED=1
  fi
  tf_worktree_delete_branch "M$i" 2>/dev/null
done
tf_assert_eq "all 10 merged" "0" "$failed"

# Verify all files on main
for i in $(seq 1 10); do
  tf_assert "file$i.txt on main" git -C "$TF_REPO_DIR" show "main:file$i.txt" >/dev/null
done
tf_group_end

# Stress 4: rapid create-merge-remove cycles
tf_group_begin; tf_test "20 rapid create-commit-merge-remove cycles"
STATUS_JSON="$SBOX/state/rapid-cycle.json"
for i in $(seq 1 20); do
  cat > "$TF_CONFIG_DIR/tasks.json" <<JSON
{"tasks":[{"id":"RC$i","engine":"t","title":"RC$i","section":"§1","deps":[],"scope":["rapid$i.txt"],"accept":"true","manual":false}]}
JSON
  WT="$(tf_worktree_create "RC$i")"
  echo "v$i" > "$WT/rapid$i.txt"
  git -C "$WT" add -A && git -C "$WT" commit -qm "feat(RC$i)"
  tf_worktree_merge "RC$i"
  tf_worktree_remove "RC$i" 2>/dev/null
  tf_worktree_delete_branch "RC$i" 2>/dev/null
done
tf_assert "all 20 files on main" \
  test "$(git -C "$TF_REPO_DIR" ls-tree main --name-only | grep -c 'rapid')" -eq 20
tf_group_end

tf_test_summary
