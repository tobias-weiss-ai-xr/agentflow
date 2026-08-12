#!/usr/bin/env bash
# run-all-tests.sh — run the full taskfleet self-test suite.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

echo "############################################################"
echo "# taskfleet self-test suite"
echo "############################################################"

TOTAL_PASS=0; TOTAL_FAIL=0; rc=0
for t in "$HERE"/test-*.sh; do
  [[ -x "$t" ]] || t="bash $t"
  echo
  echo "======== $(basename "${t##* }") ========"
  out="$(eval "$t" 2>&1)"
  echo "$out" | grep -vE "^[0-9]{4}-[0-9]"
  # extract final summary line
  line="$(echo "$out" | grep -E "Tests: [0-9]+ run" | tail -1)"
  p="$(echo "$line" | grep -oE "[0-9]+ passed" | grep -oE "[0-9]+")"
  f="$(echo "$line" | grep -oE "[0-9]+ failed" | grep -oE "[0-9]+")"
  p="${p:-0}"; f="${f:-0}"
  TOTAL_PASS=$((TOTAL_PASS + p)); TOTAL_FAIL=$((TOTAL_FAIL + f))
  [[ "$f" -gt 0 ]] && rc=1
done

echo
echo "############################################################"
printf '# SUITE TOTAL: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$TOTAL_PASS" "$TOTAL_FAIL"
echo "############################################################"
exit $rc
