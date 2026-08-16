#!/bin/bash
# run-matrix.sh — TDD campaign: Matrix appservice registration on opendesk-nix
# via taskfleet (MA-1 synapse config, MA-2 e2e test, MA-3 verify-ics.sh).
# Usage: ./run-matrix.sh [status|dry-run|run|run-task <id>|attach <id>]

set -euo pipefail

TF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export TF_REPO_DIR="${TF_REPO_DIR:-/home/weissto_local/git/opendesk_git/opendesk-nix}"
export TF_TASKS_JSON="$TF_DIR/config/tasks-matrix.json"
export TF_WORKERS_JSON="$TF_DIR/config/workers-matrix.json"
export TF_PROMPT_DIR="$TF_DIR/prompts-matrix"
export TF_STATE_DIR="${TF_STATE_DIR:-$TF_DIR/state-matrix}"
export TF_BASE_BRANCH="main"
export TF_GATE_ENV="${TF_GATE_ENV:-}"

case "${1:-status}" in
  status)   shift; exec "$TF_DIR/orchestrator.sh" --status "$@" ;;
  dry-run)  shift; exec "$TF_DIR/orchestrator.sh" --dry-run "$@" ;;
  run)      shift; exec "$TF_DIR/orchestrator.sh" "$@" ;;
  run-task) shift; exec "$TF_DIR/orchestrator.sh" --once --task "${1:?run-task needs a task id}" ;;
  attach)   shift; exec "$TF_DIR/orchestrator.sh" attach "${1:?attach needs a task id}" ;;
  *) echo "usage: $0 [status|dry-run|run|run-task <id>|attach <id>]"; exit 1 ;;
esac
