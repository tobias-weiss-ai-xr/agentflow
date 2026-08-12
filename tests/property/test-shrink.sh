#!/usr/bin/env bash
# property/test-shrink.sh — PROPERTY-BASED TESTING with COUNTEREXAMPLE SHRINKING.
#
# SOTA paradigm: generate random inputs, check a property; when a property
# fails, shrink the failing input to a minimal counterexample (delta
# debugging) so the bug is easy to diagnose.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config" TF_STATE_DIR="$SBOX/state" TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo" TF_BRANCH_PREFIX="agent"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_REPO_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A","engine":"t","title":"A","section":"§1","deps":[],"scope":["x"],"accept":"true","manual":false}
]}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/verify.sh"  # tf_classify_error lives here (lazy-sourced by status.sh only on fail)

echo "=== [property] shrinking ==="
tf_seed_init

# ---------------------------------------------------------------------------
# Demo property: a string fails if it contains the substring "BAD"
# (simulating a real property that holds for valid inputs)
# ---------------------------------------------------------------------------
fails_on_bad() {
  [[ "$1" != *BAD* ]]
}

tf_group_begin; tf_test "tf_shrink reduces a failing input to minimal counterexample"
# A long failing input: shrinks to the smallest failing case ("BAD")
long="$(printf 'x%.0s' {1..500})BAD$(printf 'y%.0s' {1..500})"
minimal="$(tf_shrink fails_on_bad "$long")"
tf_assert "minimal still fails property" bash -c "! fails_on_bad '$minimal'"
tf_assert_lt "minimal is short (<= 8)" "9" "${#minimal}"
tf_assert_contains "minimal keeps the bug" "BAD" "$minimal"
printf '    \033[36mSHRANK\033[0m  %d chars → %d chars (%q)\n' "${#long}" "${#minimal}" "$minimal"
tf_group_end

# ---------------------------------------------------------------------------
# Property: shrink output is always a suffix-splice of the input (correctness
# of the shrinker itself): the result must still fail the property.
# ---------------------------------------------------------------------------
tf_group_begin; tf_test "property: shrink always returns a still-failing input"
prop_shrink_valid() {
  local len=$(( $(tf_rand 100) + 5 ))
  local s="" chars="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"
  local i
  for ((i = 0; i < len; i++)); do
    s+="${chars:$(tf_rand 62):1}"
  done
  # guarantee it fails: append BAD
  s+="BAD"
  local m
  m="$(tf_shrink fails_on_bad "$s")"
  [[ "$m" == *BAD* ]]
}
tf_property "shrink_output_still_fails" prop_shrink_valid 10
tf_group_end

# ---------------------------------------------------------------------------
# Property: status JSON remains valid under random ops, seeded for
# reproducibility (TF_SEED already set).
# ---------------------------------------------------------------------------
tf_group_begin; tf_test "property: seeded random status ops keep JSON valid (reproducible)"
STATUS_JSON="$SBOX/state/prop-$$.json"
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"P1","engine":"t","title":"P1","section":"§1","deps":[],"scope":["a"],"accept":"true","manual":false},
  {"id":"P2","engine":"t","title":"P2","section":"§1","deps":[],"scope":["b"],"accept":"true","manual":false}
]}
JSON
tf_status_init
# seeded deterministic ops
seed="$TF_SEED"
for i in 1 2 3 4 5 6 7 8; do
  r="$(tf_rand 3)"
  case "$r" in
    0) tf_done_task "P$(( $(tf_rand 2) + 1 ))" >/dev/null 2>&1 ;;
    1) tf_fail_task "P$(( $(tf_rand 2) + 1 ))" "prop" >/dev/null 2>&1 ;;
    2) tf_status_set "P$(( $(tf_rand 2) + 1 ))" running >/dev/null 2>&1 ;;
  esac
done
tf_assert_valid_json "ledger valid after seeded ops" "$STATUS_JSON"
tf_assert "seed unchanged" test "$seed" = "$TF_SEED"
tf_group_end

# ---------------------------------------------------------------------------
# Property: tf_classify_error returns non-empty for arbitrary garbage
# (fuzz-backed property — the classifier must be total).
# ---------------------------------------------------------------------------
tf_group_begin; tf_test "property: classifier never empty on random bytes"
prop_classify_total() {
  local len=$(( $(tf_rand 80) ))
  local junk="" chars="abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_[]{}():;,.!?/\\\"'\`~@#$%^&*|+=<>\t\n"
  local i
  for ((i = 0; i < len; i++)); do
    junk+="${chars:$(tf_rand ${#chars}):1}"
  done
  local out
  out="$(tf_classify_error "$junk" 2>/dev/null)"
  [[ -n "$out" ]]
}
tf_property "classifier_total_on_random" prop_classify_total 15
tf_group_end

tf_test_summary
