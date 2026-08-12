#!/usr/bin/env bash
# test-harness.sh — minimal test framework for taskfleet.
# Defines: tf_test <name> (group), tf_assert <cmd...>, tf_assert_eq <a> <b>,
#           and a PASS/FAIL counter. Sourced by each test file.
TF_TESTS_RUN=0
TF_TESTS_PASS=0
TF_TESTS_FAIL=0
TF_CURRENT_GROUP=""

tf_test() {  # start a named test group
  TF_CURRENT_GROUP="$*"
  TF_TESTS_RUN=$((TF_TESTS_RUN + 1))
}

tf_pass() { printf '  \033[32mPASS\033[0m %s\n' "$TF_CURRENT_GROUP"; TF_TESTS_PASS=$((TF_TESTS_PASS + 1)); }
tf_fail() { printf '  \033[31mFAIL\033[0m %s\n    %s\n' "$TF_CURRENT_GROUP" "$*"; TF_TESTS_FAIL=$((TF_TESTS_FAIL + 1)); }

# tf_assert <description> <condition-cmd...>  — runs cmd, passes if exit 0
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

# tf_group_begin / tf_group_end — wrap a test() group to count pass/fail
tf_group_begin() { TF_GROUP_FAILED=0; }
tf_group_end() {
  if [[ "$TF_GROUP_FAILED" == "0" ]]; then tf_pass; else tf_fail "$TF_CURRENT_GROUP"; fi
}

tf_test_summary() {
  local rc=0
  echo ""
  echo "──────────────────────────────────────────"
  printf 'Tests: %d run, \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' \
    "$TF_TESTS_RUN" "$TF_TESTS_PASS" "$TF_TESTS_FAIL"
  [[ "$TF_TESTS_FAIL" -gt 0 ]] && rc=1
  return $rc
}
