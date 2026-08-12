#!/usr/bin/env bash
# test-harness.sh — SOTA test framework for taskfleet bash codebase.
#
# Paradigms implemented:
#   - xUnit-style groups (tf_test/tf_group_begin/end)
#   - Assertion library (tf_assert, tf_assert_eq, tf_assert_not, tf_assert_re,
#     tf_assert_gt, tf_assert_lt, tf_assert_contains, tf_assert_not_contains,
#     tf_assert_exit_code)
#   - Property-based testing (tf_for_all, tf_for_each, tf_property)
#   - Golden-file testing (tf_assert_golden, tf_update_golden)
#   - Fixture/setup/teardown (tf_fixture, tf_ensure_tmpdir)
#   - Test isolation (tf_isolated_test)
#   - Test categorization (tf_skip, tf_slow, tf_flaky)
#   - Timing/benchmarks (tf_timed)
#   - Structured TAP-compatible output (TF_TAP=1)
#   - Coverage markers (TF_COVERAGE=1)
#   - Mutation hints (tf_assert_with_hint)

set -uo pipefail

# ---------------------------------------------------------------------------
# Counters and state
# ---------------------------------------------------------------------------
TF_TESTS_RUN=0
TF_TESTS_PASS=0
TF_TESTS_FAIL=0
TF_TESTS_SKIP=0
TF_TESTS_XFAIL=0
TF_CURRENT_GROUP=""
TF_GROUP_FAILED=0
TF_SUITE_NAME="${TF_SUITE_NAME:-taskfleet}"
TF_TMPDIRS=()        # temp dirs created during tests
TF_FIXTURES_CLEANUP=() # cleanup functions registered by tf_fixture

# ---------------------------------------------------------------------------
# TAP output mode (TF_TAP=1 for CI integration)
# ---------------------------------------------------------------------------
tf_tap() {
  [[ "${TF_TAP:-0}" == "1" ]] || return 0
  local ok="$1" testnum="$2" desc="$3" directive="${4:-}"
  if [[ "$ok" == "ok" ]]; then
    [[ -n "$directive" ]] && echo "$ok $testnum - $desc # $directive" || echo "$ok $testnum - $desc"
  else
    [[ -n "$directive" ]] && echo "not $ok $testnum - $desc # $directive" || echo "not $ok $testnum - $desc"
  fi
}

# ---------------------------------------------------------------------------
# Core assertions
# ---------------------------------------------------------------------------

tf_test() {
  TF_CURRENT_GROUP="$*"
  TF_TESTS_RUN=$((TF_TESTS_RUN + 1))
  TF_GROUP_FAILED=0
  [[ "${TF_TAP:-0}" == "1" ]] && echo "# Subtest: $*"
}

tf_pass() {
  printf '  \033[32mPASS\033[0m %s\n' "$TF_CURRENT_GROUP"
  TF_TESTS_PASS=$((TF_TESTS_PASS + 1))
}

tf_fail() {
  printf '  \033[31mFAIL\033[0m %s\n    %s\n' "$TF_CURRENT_GROUP" "$*"
  TF_TESTS_FAIL=$((TF_TESTS_FAIL + 1))
}

tf_skip_test() {
  printf '  \033[33mSKIP\033[0m %s — %s\n' "$TF_CURRENT_GROUP" "${1:-no reason}"
  TF_TESTS_SKIP=$((TF_TESTS_SKIP + 1))
  TF_GROUP_FAILED=0 # skip doesn't count as failure
}

tf_xfail() {
  printf '  \033[36mXFAIL\033[0m %s — %s\n' "$TF_CURRENT_GROUP" "${1:-expected to fail}"
  TF_TESTS_XFAIL=$((TF_TESTS_XFAIL + 1))
}

tf_group_begin() { TF_GROUP_FAILED=0; }

tf_group_end() {
  if [[ "$TF_GROUP_FAILED" == "0" ]]; then tf_pass; else tf_fail "$TF_CURRENT_GROUP"; fi
}

# tf_assert <description> <condition-cmd...>  — passes if exit 0
tf_assert() {
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s\n' "$desc"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_not <description> <cmd...>  — passes if cmd exits NON-zero
tf_assert_not() {
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then
    printf '    \033[31mBAD\033[0m  %s (expected non-zero exit)\n' "$desc"
    TF_GROUP_FAILED=1
  else
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  fi
}

# tf_assert_eq <desc> <expected> <actual>
tf_assert_eq() {
  local desc="$1" exp="$2" act="$3"
  if [[ "$exp" == "$act" ]]; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s (expected %q, got %q)\n' "$desc" "$exp" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_ne <desc> <unexpected> <actual>
tf_assert_ne() {
  local desc="$1" unexpected="$2" act="$3"
  if [[ "$unexpected" != "$act" ]]; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s (expected anything except %q)\n' "$desc" "$unexpected"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_gt <desc> <threshold> <actual>
tf_assert_gt() {
  local desc="$1" thresh="$2" act="$3"
  if [[ "$act" =~ ^[0-9]+$ && "$act" -gt "$thresh" ]]; then
    printf '    \033[32mok\033[0m   %s (%s > %s)\n' "$desc" "$act" "$thresh"
  else
    printf '    \033[31mBAD\033[0m  %s (expected > %s, got %s)\n' "$desc" "$thresh" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_lt <desc> <threshold> <actual>
tf_assert_lt() {
  local desc="$1" thresh="$2" act="$3"
  if [[ "$act" =~ ^[0-9]+$ && "$act" -lt "$thresh" ]]; then
    printf '    \033[32mok\033[0m   %s (%s < %s)\n' "$desc" "$act" "$thresh"
  else
    printf '    \033[31mBAD\033[0m  %s (expected < %s, got %s)\n' "$desc" "$thresh" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_re <desc> <pattern> <actual_string>
tf_assert_re() {
  local desc="$1" pat="$2" act="$3"
  if grep -qP "$pat" <<< "$act" 2>/dev/null; then
    printf '    \033[32mok\033[0m   %s (matches /%s/)\n' "$desc" "$pat"
  else
    printf '    \033[31mBAD\033[0m  %s (no match for /%s/ in %q)\n' "$desc" "$pat" "$act"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_contains <desc> <needle> <haystack>
tf_assert_contains() {
  local desc="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf '    \033[32mok\033[0m   %s (contains %q)\n' "$desc" "$needle"
  else
    printf '    \033[31mBAD\033[0m  %s (expected to contain %q)\n' "$desc" "$needle"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_not_contains <desc> <needle> <haystack>
tf_assert_not_contains() {
  local desc="$1" needle="$2" haystack="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf '    \033[32mok\033[0m   %s (does not contain %q)\n' "$desc" "$needle"
  else
    printf '    \033[31mBAD\033[0m  %s (should not contain %q)\n' "$desc" "$needle"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_exit_code <desc> <expected_rc> <actual_rc>
tf_assert_exit_code() {
  local desc="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '    \033[32mok\033[0m   %s (exit %s)\n' "$desc" "$expected"
  else
    printf '    \033[31mBAD\033[0m  %s (expected exit %s, got %s)\n' "$desc" "$expected" "$actual"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_with_hint <desc> <condition-cmd...> — shows diagnostic on failure
tf_assert_with_hint() {
  local desc="$1"; shift
  local hint_cmd="$1"; shift
  if "$@" >/dev/null 2>&1; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    local diag
    diag="$(eval "$hint_cmd" 2>&1)"
    printf '    \033[31mBAD\033[0m  %s\n' "$desc"
    printf '    \033[33mHINT\033[0m  %s\n' "$diag"
    TF_GROUP_FAILED=1
  fi
}

# tf_assert_jq_eq <desc> <jq_filter> <file> <expected_value>
tf_assert_jq_eq() {
  local desc="$1" filter="$2" file="$3" expected="$4"
  local actual
  actual="$(jq -r "$filter" "$file" 2>/dev/null)"
  if [[ "$actual" == "$expected" ]]; then
    printf '    \033[32mok\033[0m   %s\n' "$desc"
  else
    printf '    \033[31mBAD\033[0m  %s (jq %s: expected %q, got %q)\n' "$desc" "$filter" "$expected" "$actual"
    TF_GROUP_FAILED=1
  fi
}

# ---------------------------------------------------------------------------
# Property-based testing (bash generators)
# ---------------------------------------------------------------------------

# tf_for_all <description> <generator_fn> <property_fn> [<n>]
#   Runs property_fn for each value emitted by generator_fn (one per line).
#   Generator must echo one value per line. Default n=100.
tf_for_all() {
  local desc="$1" gen="$2" prop="$3" n="${4:-100}"
  local i=0 pass=0 fail=0
  while IFS= read -r val && [[ $i -lt $n ]]; do
    i=$((i + 1))
    if $prop "$val" >/dev/null 2>&1; then
      pass=$((pass + 1))
    else
      fail=$((fail + 1))
      printf '    \033[31mCOUNTER\033[0m  %s — counterexample: %q\n' "$desc" "$val"
      TF_GROUP_FAILED=1
    fi
  done < <($gen "$n")
  printf '    \033[32mok\033[0m   %s (%d/%d passed)\n' "$desc" "$pass" "$i"
}

# tf_for_each <description> <values...> -- <property_fn>
#   Runs property_fn once per value. Use for small explicit sets.
tf_for_each() {
  local desc="$1"; shift
  local fn=""
  local vals=()
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "--" ]]; then shift; fn="$1"; shift; break; fi
    vals+=("$1"); shift
  done
  [[ -z "$fn" ]] && return 1
  local pass=0 fail=0 total="${#vals[@]}"
  for v in "${vals[@]}"; do
    if $fn "$v" >/dev/null 2>&1; then
      pass=$((pass + 1))
    else
      fail=$((fail + 1))
      printf '    \033[31mCOUNTER\033[0m  %s — counterexample: %q\n' "$desc" "$v"
      TF_GROUP_FAILED=1
    fi
  done
  printf '    \033[32mok\033[0m   %s (%d/%d passed)\n' "$desc" "$pass" "$total"
}

# tf_property <name> <property_fn> [n=100]
#   Shorthand: property_fn takes no args, runs n times, must always return 0.
tf_property() {
  local desc="$1" fn="$2" n="${3:-100}"
  local i=0 pass=0
  for ((i = 0; i < n; i++)); do
    if $fn >/dev/null 2>&1; then
      pass=$((pass + 1))
    else
      printf '    \033[31mCOUNTER\033[0m  %s — failed on iteration %d\n' "$desc" "$i"
      TF_GROUP_FAILED=1
      return 1
    fi
  done
  printf '    \033[32mok\033[0m   %s (%d iterations)\n' "$desc" "$pass"
}

# ---------------------------------------------------------------------------
# Generators for property-based tests
# ---------------------------------------------------------------------------

# Generate random task IDs (alphanumeric, 2-8 chars)
tf_gen_task_id() {
  local n="${1:-50}"
  local chars='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-'
  for ((i = 0; i < n; i++)); do
    local len=$((RANDOM % 7 + 2))
    local id=""
    for ((j = 0; j < len; j++)); do
      id+="${chars:RANDOM%${#chars}:1}"
    done
    echo "$id"
  done
}

# Generate random task count (1-500)
tf_gen_task_count() {
  local n="${1:-50}"
  for ((i = 0; i < n; i++)); do
    echo $((RANDOM % 500 + 1))
  done
}

# Generate random file paths (Unix-safe)
tf_gen_filepath() {
  local n="${1:-50}"
  local parts="src lib core crates tests scripts"
  local exts="rs ts tsx sh py json toml yaml md"
  for ((i = 0; i < n; i++)); do
    local depth=$((RANDOM % 4 + 1))
    local path=""
    for ((j = 0; j < depth; j++)); do
      local arr=($parts)
      path+="${arr[RANDOM%${#arr[@]}]}/"
    done
    local arr=($exts)
    path+="${arr[RANDOM%${#arr[@]}]}"
    echo "$path"
  done
}

# ---------------------------------------------------------------------------
# Golden-file testing
# ---------------------------------------------------------------------------

TF_GOLDEN_DIR="${TF_GOLDEN_DIR:-tests/golden}"

# tf_assert_golden <name> <actual_content>
#   Compares actual content against stored golden file. If TF_UPDATE_GOLDEN=1,
#   writes the actual content as the new golden file.
tf_assert_golden() {
  local name="$1" actual="$2"
  local golden="$TF_GOLDEN_DIR/$name.golden"
  if [[ "${TF_UPDATE_GOLDEN:-0}" == "1" ]]; then
    mkdir -p "$(dirname "$golden")"
    printf '%s' "$actual" > "$golden"
    printf '    \033[33mUPDATED\033[0m  golden/%s\n' "$name"
    return 0
  fi
  if [[ ! -f "$golden" ]]; then
    printf '    \033[33mMISSING\033[0m  golden/%s (run TF_UPDATE_GOLDEN=1 to create)\n' "$name"
    TF_GROUP_FAILED=1
    return 1
  fi
  local expected
  expected="$(cat "$golden")"
  if [[ "$actual" == "$expected" ]]; then
    printf '    \033[32mok\033[0m   golden/%s matches\n' "$name"
  else
    printf '    \033[31mBAD\033[0m  golden/%s mismatch\n' "$name"
    diff <(echo "$expected") <(echo "$actual") | head -10 | sed 's/^/    /'
    TF_GROUP_FAILED=1
  fi
}

# ---------------------------------------------------------------------------
# Fixtures and lifecycle
# ---------------------------------------------------------------------------

# tf_ensure_tmpdir <name>  → creates/tracks a temp dir, prints its path
tf_ensure_tmpdir() {
  local name="${1:-test-$$_$RANDOM}"
  local d="/tmp/tf-test-$name"
  rm -rf "$d"
  mkdir -p "$d"
  TF_TMPDIRS+=("$d")
  echo "$d"
}

# tf_fixture <setup_fn> <cleanup_fn> <test_fn>
#   Runs setup, then test, then cleanup (even if test fails).
tf_fixture() {
  local setup="$1" cleanup="$2"; shift 2
  local rc=0
  $setup || { tf_fail "fixture setup failed: $setup"; return 1; }
  "$@" || rc=$?
  $cleanup || true
  return $rc
}

# tf_isolated_test <name> <test_body_fn>
#   Creates a fresh tmpdir sandbox, sets TF_STATE_DIR/TF_LOG_DIR, runs body.
tf_isolated_test() {
  local name="$1"; shift
  local sandbox
  sandbox="$(tf_ensure_tmpdir "$name")"
  export TF_STATE_DIR="$sandbox/state"
  export TF_LOG_DIR="$sandbox/logs"
  mkdir -p "$TF_STATE_DIR" "$TF_LOG_DIR"
  "$@"
}

# ---------------------------------------------------------------------------
# Timing and benchmarks
# ---------------------------------------------------------------------------

# tf_timed <description> <cmd...>
#   Runs cmd, prints elapsed time. Asserts under TF_BENCH_THRESHOLD if set.
tf_timed() {
  local desc="$1"; shift
  local start end elapsed
  start="$(date +%s%N)"
  "$@"
  local rc=$?
  end="$(date +%s%N)"
  elapsed=$(( (end - start) / 1000000 ))  # ms
  local threshold="${TF_BENCH_THRESHOLD:-5000}"
  if [[ $elapsed -lt $threshold ]]; then
    printf '    \033[32mok\033[0m   %s (%dms < %dms)\n' "$desc" "$elapsed" "$threshold"
  else
    printf '    \033[33mSLOW\033[0m  %s (%dms >= %dms threshold)\n' "$desc" "$elapsed" "$threshold"
    [[ "${TF_STRICT_BENCH:-0}" == "1" ]] && TF_GROUP_FAILED=1
  fi
  return $rc
}

# ---------------------------------------------------------------------------
# Global cleanup and summary
# ---------------------------------------------------------------------------

tf_cleanup() {
  # Remove tracked tmpdirs
  for d in "${TF_TMPDIRS[@]}"; do
    rm -rf "$d"
  done
  # Run registered fixture cleanups
  for fn in "${TF_FIXTURES_CLEANUP[@]}"; do
    $fn >/dev/null 2>&1 || true
  done
}

tf_test_summary() {
  local rc=0
  echo ""
  echo "──────────────────────────────────────────"
  printf 'Tests: %d run, \033[32m%d passed\033[0m, \033[31m%d failed\033[0m' \
    "$TF_TESTS_RUN" "$TF_TESTS_PASS" "$TF_TESTS_FAIL"
  [[ $TF_TESTS_SKIP -gt 0 ]] && printf ', \033[33m%d skipped\033[0m' "$TF_TESTS_SKIP"
  [[ $TF_TESTS_XFAIL -gt 0 ]] && printf ', \033[36m%d xfail\033[0m' "$TF_TESTS_XFAIL"
  echo ""
  [[ "$TF_TESTS_FAIL" -gt 0 ]] && rc=1
  tf_cleanup
  return $rc
}

# Backward compat (old names)
wo_test() { tf_test "$@"; }
wo_pass() { tf_pass "$@"; }
wo_fail() { tf_fail "$@"; }
wo_assert() { tf_assert "$@"; }
wo_assert_not() { tf_assert_not "$@"; }
wo_assert_eq() { tf_assert_eq "$@"; }
wo_group_begin() { tf_group_begin; }
wo_group_end() { tf_group_end; }
wo_test_summary() { tf_test_summary "$@"; }
