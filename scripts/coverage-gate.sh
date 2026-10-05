#!/usr/bin/env bash
#
# coverage-gate.sh — fail the build when line coverage drops.
#
# The ratchet threshold lives in .coverage-min (single source of truth, NOT
# duplicated in the workflow). It may only be RAISED over time — see
# docs/coverage.md. This script fails if total workspace line coverage falls
# below that floor.
set -euo pipefail

# Resolve the repo root from this script's location so the gate works from any
# working directory (locally and in CI).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MIN_FILE="${REPO_ROOT}/.coverage-min"

if [[ ! -f "${MIN_FILE}" ]]; then
  echo "coverage-gate: missing threshold file: ${MIN_FILE}" >&2
  exit 2
fi

# Read the first non-empty, non-comment line and trim surrounding whitespace.
MIN="$(grep -vE '^[[:space:]]*(#|$)' "${MIN_FILE}" | head -n1 | tr -d '[:space:]')"

if [[ -z "${MIN}" ]]; then
  echo "coverage-gate: ${MIN_FILE} contains no threshold value" >&2
  exit 2
fi

if ! [[ "${MIN}" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
  echo "coverage-gate: invalid threshold '${MIN}' in ${MIN_FILE} (expected a number)" >&2
  exit 2
fi

cd "${REPO_ROOT}"

echo "coverage-gate: enforcing >= ${MIN}% total line coverage (from .coverage-min)"

if ! cargo llvm-cov --workspace --show-missing-lines --fail-under-lines "${MIN}"; then
  cat >&2 <<EOF

coverage-gate: FAILED — total line coverage is below the ${MIN}% floor set in .coverage-min.

What to do:
  * Add tests for the new/changed untested code, then re-run: ./scripts/coverage-gate.sh
  * Only if the drop is a deliberate, reviewed trade-off, raise/relax the floor
    in .coverage-min — touch it as few times as possible and never lower it
    silently to make a build pass.
  * This threshold is a ratchet: it may only be RAISED over time.
  See docs/coverage.md for details.
EOF
  exit 1
fi

echo "coverage-gate: OK — total line coverage is at or above ${MIN}%"
