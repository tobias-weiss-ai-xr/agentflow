#!/usr/bin/env bash
# fuzz/test-verify.sh — fuzz testing for error classifier
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"; mkdir -p "$TF_CONFIG_DIR"
export TF_STATE_DIR="$SBOX/state"; export TF_LOG_DIR="$SBOX/logs"
mkdir -p "$TF_LOG_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[{"name":"w1","provider":"p","model":"m","enabled":true}],"defaults":{}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/worktree.sh"
. "$HERE/../../lib/verify.sh"

echo "=== [fuzz] verify.sh — error classifier ==="

# Helper: check that result contains ": " (structured output)
has_structured_output() { [[ "$1" == *": "* ]]; }

# Fuzzer 1: random bytes
tf_group_begin; tf_test "fuzz: tf_classify_error on random binary data"
fuzz_random_bytes() {
  local log="$SBOX/fb-$$_$RANDOM.log"
  dd if=/dev/urandom bs=128 count=1 2>/dev/null > "$log"
  tf_classify_error "$log" >/dev/null 2>&1
}
tf_property "classify_random_bytes_never_crashes" fuzz_random_bytes 20
tf_group_end

# Fuzzer 2: random printable content
tf_group_begin; tf_test "fuzz: tf_classify_error on random printable strings"
fuzz_printable() {
  local log="$SBOX/fp-$$_$RANDOM.log"
  dd if=/dev/urandom bs=200 count=1 2>/dev/null > "$log"
  tf_classify_error "$log" >/dev/null 2>&1
}
tf_property "classify_printable_never_crashes" fuzz_printable 20
tf_group_end

# Fuzzer 3: known edge-case inputs
tf_group_begin; tf_test "fuzz: tf_classify_error on edge-case inputs"
EDGE_CASES=(
  "" " " "error" "error:" "error[]" "undefined"
  "undefined symbol" "timeout" "FAIL" "PASS"
  "test result: FAILED" "cargo test" "pnpm ERR!"
)
for input in "${EDGE_CASES[@]}"; do
  log="$SBOX/fe-$$_$RANDOM.log"
  printf '%s' "$input" > "$log"
  result="$(tf_classify_error "$log")"
  tf_assert "output for (${input:0:30})" has_structured_output "$result"
done
tf_group_end

# Fuzzer 4: mixed real error patterns
tf_group_begin; tf_test "fuzz: tf_classify_error on mixed real error patterns"
fuzz_mixed_errors() {
  local log="$SBOX/fm-$$_$RANDOM.log"
  local patterns=(
    "error[E0308]: mismatched types"
    "error[E0277]: trait bound not satisfied"
    "error: could not compile"
    "rust-lld: error: undefined symbol"
    "error: package ID specification did not match"
    "test result: FAILED"
    "thread main panicked at src/lib.rs"
    "Compiling wo-foo v0.1.0"
  )
  local n=$((RANDOM % 5 + 1))
  local i
  for ((i = 0; i < n; i++)); do
    echo "${patterns[$((RANDOM % ${#patterns[@]}))]} line $i"
  done > "$log"
  tf_classify_error "$log" >/dev/null 2>&1
}
tf_property "classify_mixed_errors_never_crashes" fuzz_mixed_errors 20
tf_group_end

# Fuzzer 5: large log files
tf_group_begin; tf_test "fuzz: tf_classify_error on large logs (1000 lines)"
fuzz_large() {
  local log="$SBOX/fl-$$_$RANDOM.log"
  local i
  for ((i = 0; i < 1000; i++)); do
    echo "line $i of output for testing classification behavior"
  done > "$log"
  tf_classify_error "$log" >/dev/null 2>&1
}
tf_property "classify_large_never_crashes" fuzz_large 5
tf_group_end

# Fuzzer 6: tf_error_snippet never crashes
tf_group_begin; tf_test "fuzz: tf_error_snippet on random inputs"
fuzz_snippet() {
  local log="$SBOX/fs-$$_$RANDOM.log"
  dd if=/dev/urandom bs=300 count=1 2>/dev/null > "$log"
  tf_error_snippet "$log" >/dev/null 2>&1
}
tf_property "snippet_never_crashes" fuzz_snippet 20
tf_group_end

# Fuzzer 7: unicode content
tf_group_begin; tf_test "fuzz: tf_classify_error on unicode content"
fuzz_unicode() {
  local log="$SBOX/fu-$$_$RANDOM.log"
  echo "error: something went wrong in processing" > "$log"
  echo "Japanese text" >> "$log"
  tf_classify_error "$log" >/dev/null 2>&1
}
tf_property "classify_unicode_never_crashes" fuzz_unicode 10
tf_group_end

tf_test_summary
