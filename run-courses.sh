#!/usr/bin/env bash
# Wrapper to run taskfleet for courses learning material generation
set -euo pipefail

export TF_REPO_DIR="/home/weissto_local/git/courses"
export TF_TASKS_JSON="/home/weissto_local/git/agentflow/config/tasks-courses.json"
export TF_WORKERS_JSON="/home/weissto_local/git/agentflow/config/workers-courses.json"
export TF_STATE_DIR="/home/weissto_local/git/agentflow/.state-courses"
export TF_LOG_DIR="/home/weissto_local/git/agentflow/.state-courses/logs"
mkdir -p "$TF_STATE_DIR" "$TF_LOG_DIR"

exec "$(dirname "$0")/orchestrator.sh" "$@"
