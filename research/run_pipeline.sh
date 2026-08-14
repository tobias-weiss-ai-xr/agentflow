#!/usr/bin/env bash
# Run the taskfleet-research competitive landscape pipeline.
#
# Usage:
#   ./run_pipeline.sh            — full fetch → classify → dedup → digest
#   ./run_pipeline.sh --dry-run  — fetch + classify + score (no writes)
#   ./run_pipeline.sh --fetch-only  — fetch and append to repos.yaml, skip digest
#   ./run_pipeline.sh --digest-only — re-digest existing repos.yaml
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

LOG="$ROOT/data/run.log"
mkdir -p "$ROOT/data"

# Bootstrap venv on first run
if [[ ! -x "$ROOT/.venv/bin/python" ]]; then
    echo "[run] venv missing — creating..."
    python3 -m venv "$ROOT/.venv"
    "$ROOT/.venv/bin/pip" install -q -r "$ROOT/requirements.txt"
fi

echo "[run] $(date '+%F %T') — starting pipeline (args: $*)"
"$ROOT/.venv/bin/python" "$ROOT/run_pipeline.py" "$@"
status=$?
echo "[run] $(date '+%F %T') — finished (exit $status)" >> "$LOG"
exit $status
