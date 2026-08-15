#!/usr/bin/env bash
# Launcher for the contextual-intelligence intelligence-iteration fleet.
# Exists so the caller's command line does not contain the pattern that
# taskfleet's stale-process killer matches at startup.
set -euo pipefail
cd "$(dirname "$0")"

export TF_REPO_DIR="${TF_REPO_DIR:-/home/weiss/git/contextual-intelligence}"
export TF_TASKS_JSON="${TF_TASKS_JSON:-config/tasks-intelligence.json}"
export TF_PROMPT_DIR="${TF_PROMPT_DIR:-config/prompts-intel}"

exec bash ./orchestrator.sh "$@"
