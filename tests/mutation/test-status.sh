#!/usr/bin/env bash
# mutation/test-status.sh — MUTATION TESTING for lib/status.sh
#
# SOTA paradigm: inject real bugs into a sandboxed copy of the source and
# verify the test suite KILLS them. Surviving mutations reveal test gaps.
#
# Each mutation is a sed expression applied to a copy of lib/status.sh.
# The runner sources that copy and exercises the state machine. If the
# mutated code makes the tests pass anyway, the mutation SURVIVED — the
# tests are not strong enough to catch that class of bug.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT

# The mutation runner: sets up a fresh sandbox pointing at a given source
# tree, runs the state-machine test script, returns its exit code.
TF_DIR="$(cd "$HERE/../.." && pwd)"

# Build a self-contained state-machine test that sources the (possibly
# mutated) lib/status.sh. It returns 0 iff the state machine behaves
# correctly under the mutation.
cat > "$SBOX/runner.sh" <<'RUNNER'
#!/usr/bin/env bash
set -uo pipefail
# $1 = path to source tree (with possibly-mutated lib/)
SRC="$1"
S="$2"
mkdir -p "$S/config" "$S/state" "$S/logs" "$S/repo"
export TF_CONFIG_DIR="$S/config"
export TF_STATE_DIR="$S/state"
export TF_LOG_DIR="$S/logs"
export TF_REPO_DIR="$S/repo"
export TF_BRANCH_PREFIX="agent"
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false},
  {"id":"B","engine":"t","title":"B","section":"§1","deps":["A"],"scope":["y"],"accept":"true","manual":false},
  {"id":"C","engine":"t","title":"C","section":"§1","deps":[],"scope":["z"],"accept":"true","manual":false}
]}
JSON
. "$SRC/lib/common.sh"
. "$SRC/lib/status.sh"

tf_status_init

# 1. init → all ready
[[ "$(tf_status_get A .status)" == "ready" ]] || exit 1
[[ "$(tf_status_get B .status)" == "ready" ]] || exit 1

# 2. lifecycle: ready → running → done
tf_status_set A running
[[ "$(tf_status_get A .status)" == "running" ]] || exit 1
tf_done_task A
[[ "$(tf_status_get A .status)" == "done" ]] || exit 1

# 3. failure increments attempts + schedules retry
tf_fail_task C "boom"
[[ "$(tf_status_get C .attempts)" == "1" ]] || exit 1
[[ "$(tf_status_get C .status)" == "failed" ]] || exit 1
[[ -n "$(tf_status_get C .next_retry_at)" ]] || exit 1

# 4. dependency resolution: B depends on A (done) → ready
tf_is_ready B || exit 1

# 5. counting
[[ "$(tf_count_status done)" == "1" ]] || exit 1   # only A
[[ "$(tf_count_status failed)" == "1" ]] || exit 1  # only C

# 6. status JSON stays valid
jq -e . "$STATUS_JSON" >/dev/null 2>&1 || exit 1
exit 0
RUNNER
chmod +x "$SBOX/runner.sh"

echo "=== [mutation] lib/status.sh ==="
tf_seed_init

TF_MUT_SRC="$TF_DIR"
tf_group_begin; tf_test "mutations of lib/status.sh are killed by state-machine tests"
tf_mutation_test "status" "lib/status.sh" \
  "bash $SBOX/runner.sh __MUTDIR__ $SBOX/work" \
  's/\.status="done"/.status="DONE"/' \
  's/status: "done"/status: "DONE"/' \
  's/attempts\\": 0/attempts\\": 1/' \
  's/attempts=$((attempts + 1))/attempts=$((attempts + 2))/' \
  's/if \[\[ $dep_status == "done" \]\]/if [[ $dep_status == "DONE" ]]/' \
  's/tf_status_get "\\$dep" .status/tf_status_get "$dep" .bogus/' \
  's/\[\[ "$nr" < "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \]\]/true/' \
  's/next_retry_at": null/next_retry_at": "1999-01-01T00:00:00Z"/'
tf_group_end

# Report how many mutations the tests caught
tf_group_begin; tf_test "mutation score report"
tf_mutation_report
tf_assert_gt "mutation score kills most" "5" "$TF_MUT_KILLED"
tf_group_end

tf_test_summary
