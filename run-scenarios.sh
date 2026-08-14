#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="/home/weissto_local/git/courses"

export TF_REPO_DIR="$REPO_DIR"
export TF_STATE_DIR="$SCRIPT_DIR/.state-scenarios"
export TF_TASKS_JSON="$SCRIPT_DIR/config/tasks-scenarios.json"
export TF_WORKERS_JSON="$SCRIPT_DIR/config/workers-courses.json"
export TF_PROMPT_DIR="$SCRIPT_DIR/prompts"
export TF_WORKER="${1:-deepseek-v4-flash-local}"
export TF_MAX_PARALLEL="${2:-1}"
export TF_POLL_INTERVAL="${3:-10}"

exec "$SCRIPT_DIR/orchestrator.sh" \
  --worker "$TF_WORKER" \
  --max-parallel "$TF_MAX_PARALLEL" \
  --poll "$TF_POLL_INTERVAL" \
  --log "$TF_STATE_DIR/orchestrator.log" \
  "$@"
