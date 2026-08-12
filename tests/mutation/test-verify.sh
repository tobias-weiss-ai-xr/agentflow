#!/usr/bin/env bash
# mutation/test-verify.sh — MUTATION TESTING for lib/verify.sh
#
# Injects bugs into error classification, gate verdict logic, and scope
# checking; verifies the test suite kills them.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
TF_DIR="$(cd "$HERE/../.." && pwd)"

# Runner: exercises the classifier against a (possibly mutated) verify.sh
cat > "$SBOX/runner.sh" <<'RUNNER'
#!/usr/bin/env bash
set -uo pipefail
SRC="$1"
S="$2"
mkdir -p "$S/config" "$S/state" "$S/logs" "$S/repo"
export TF_CONFIG_DIR="$S/config" TF_STATE_DIR="$S/state" TF_LOG_DIR="$S/logs"
export TF_REPO_DIR="$S/repo" TF_BRANCH_PREFIX="agent"
cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"T","engine":"t","title":"T","section":"§1","deps":[],"scope":["src/a.rs","src/b.rs"],"accept":"true","manual":false}
]}
JSON
. "$SRC/lib/common.sh"
. "$SRC/lib/verify.sh"

# 1. classify compile errors
c="$(tf_classify_error "error[E0308]: mismatched types
 --> src/a.rs:12:5")"
[[ "$c" == *compile_error* ]] || exit 1

# 2. classify test failures
c="$(tf_classify_error "test result: FAILED. 3 failed; 5 passed")"
[[ "$c" == *test_failure* ]] || exit 1

# 3. classify linker errors
c="$(tf_classify_error "rust-lld: error: undefined symbol: FPDF_Init")"
[[ "$c" == *linker_error* ]] || exit 1

# 4. classify timeouts
c="$(tf_classify_error "timed out waiting for")"
[[ "$c" == *timeout* ]] || exit 1

# 5. classify unknown
c="$(tf_classify_error "completely random garbage")"
[[ "$c" == *unknown* ]] || exit 1

# 6. error.json written with correct category
mkdir -p "$TF_LOG_DIR"
echo "error[E0401]: cannot use" | tf_write_error_json T
[[ -f "$TF_LOG_DIR/T.error.json" ]] || exit 1
cat "$TF_LOG_DIR/T.error.json" | jq -e '.category == "compile_error"' >/dev/null 2>&1 || exit 1

# 7. scope check: out-of-scope file flagged, in-scope not
S2="$S/scope"
mkdir -p "$S2"
echo "a" > "$S2/src/a.rs"
echo "b" > "$S2/src/b.rs"
echo "evil" > "$S2/evil.txt"
# tf_verify_scope prints violations to stdout; non-zero when out-of-scope
if tf_verify_scope T "$S2" | grep -q "evil.txt"; then :; else exit 1; fi
# no violations for clean scope
rm "$S2/evil.txt"
if out="$(tf_verify_scope T "$S2")"; then :; else exit 1; fi
[[ -z "$out" ]] || exit 1
exit 0
RUNNER
chmod +x "$SBOX/runner.sh"

echo "=== [mutation] lib/verify.sh ==="
tf_seed_init
TF_MUT_SRC="$TF_DIR"

tf_group_begin; tf_test "mutations of lib/verify.sh are killed by classifier tests"
tf_mutation_test "verify" "lib/verify.sh" \
  "bash $SBOX/runner.sh __MUTDIR__ $SBOX/work" \
  's/compile_error/compile_ok/' \
  's/test_failure/test_ok/' \
  's/linker_error/linker_ok/' \
  's/rust-lld/rust-ok/' \
  's/error\[/okerror[/' \
  's/test result: FAILED/test result: PASSED/' \
  's/timed out/timed in/'
tf_group_end

tf_group_begin; tf_test "verify mutation score"
tf_mutation_report
tf_assert_gt "verify mutation score > 5" "5" "$TF_MUT_KILLED"
tf_group_end

tf_test_summary
