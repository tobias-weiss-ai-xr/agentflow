#!/usr/bin/env bash
# run-all-tests.sh — SOTA test runner for taskfleet.
#
# Runs all test suites: unit, property, fuzz, stress, golden, integration, manifest.
# Usage:
#   bash tests/run-all-tests.sh              # run all
#   bash tests/run-all-tests.sh unit         # run only unit
#   bash tests/run-all-tests.sh unit fuzz    # run unit + fuzz
#   TF_TAP=1 bash tests/run-all-tests.sh     # TAP output for CI
#   TF_UPDATE_GOLDEN=1 bash tests/run-all-tests.sh  # update golden files
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=test-harness.sh
. "$HERE/test-harness.sh"

TOTAL_RUN=0
TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_SKIP=0

run_suite() {
  local suite_file="$1"
  local suite_name
  suite_name="$(basename "$suite_file" .sh)"
  echo ""
  if [[ "${TF_TAP:-0}" == "1" ]]; then
    echo "# Suite: $suite_name"
  else
    echo "╔══════════════════════════════════════════════════════════╗"
    printf '║ %-58s ║\n' "tests/$(basename "$(dirname "$suite_file")")/$suite_name"
    echo "╚══════════════════════════════════════════════════════════╝"
  fi
  local run_before="$TF_TESTS_RUN"
  bash "$suite_file"
  local rc=$?
  local run_after="$TF_TESTS_RUN"
  local suite_run=$((run_after - run_before))
  TOTAL_RUN=$((TOTAL_RUN + suite_run))
  return $rc
}

# Determine which suites to run
FILTER="${1:-all}"
shift || true

# Define suites in order
SUITES=(
  "unit/test-common.sh"
  "unit/test-status.sh"
  "unit/test-verify.sh"
  "unit/test-worktree.sh"
  "property/test-dispatch.sh"
  "fuzz/test-verify.sh"
  "stress/test-concurrent.sh"
  "golden/test-status-board.sh"
)

# Add integration + legacy suites
SUITES+=(
  "test-dispatch.sh"
  "test-verify.sh"
  "test-worktree.sh"
  "test-status.sh"
  "test-integration.sh"
  "test-manifest.sh"
)

echo "╔══════════════════════════════════════════════════════════╗"
printf '║ %-58s ║\n' "taskfleet test suite"
printf '║ %-58s ║\n' "filter=$FILTER TF_TAP=${TF_TAP:-0} TF_UPDATE_GOLDEN=${TF_UPDATE_GOLDEN:-0}"
echo "╚══════════════════════════════════════════════════════════╝"

SUITE_FAILS=0
for suite in "${SUITES[@]}"; do
  # Apply filter
  if [[ "$FILTER" == "all" ]] || echo "$suite" | grep -q "$FILTER"; then
    run_suite "$HERE/$suite" || SUITE_FAILS=$((SUITE_FAILS + 1))
  fi
done

echo ""
echo "╔══════════════════════════════════════════════════════════╗"
printf '║ %-58s ║\n' "FINAL RESULTS"
echo "╠══════════════════════════════════════════════════════════╣"
printf '║  Suites: %d  Failed: %d                                  ║\n' "$(echo "${SUITES[@]}" | wc -w)" "$SUITE_FAILS"
echo "╠══════════════════════════════════════════════════════════╣"

if [[ "${TF_TAP:-0}" == "1" ]]; then
  echo "1..$TF_TESTS_RUN"
  echo "# Total: $TF_TESTS_RUN run, $TF_TESTS_PASS passed, $TF_TESTS_FAIL failed"
else
  printf '║  Total:  \033[%dm%d passed\033[0m, \033[%dm%d failed\033[0m' \
    $((TF_TESTS_FAIL == 0 ? 32 : 31)) "$TF_TESTS_PASS" \
    $((TF_TESTS_FAIL > 0 ? 31 : 32)) "$TF_TESTS_FAIL"
  [[ $TF_TESTS_SKIP -gt 0 ]] && printf ', \033[33m%d skipped\033[0m' "$TF_TESTS_SKIP"
  echo ""
fi
echo "╚══════════════════════════════════════════════════════════╝"

exit $SUITE_FAILS
