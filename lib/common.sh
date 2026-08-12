#!/usr/bin/env bash
# common.sh — shared constants and helpers for taskfleet.
# Sourced by all other lib/*.sh and orchestrator.sh. Do not execute directly.

set -uo pipefail

# ---------------------------------------------------------------------------
# Paths (resolve relative to this file so the script is location-independent)
# ---------------------------------------------------------------------------
TF_DIR="${TF_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
TF_CONFIG_DIR="${TF_CONFIG_DIR:-$TF_DIR/config}"
TF_STATE_DIR="${TF_STATE_DIR:-$TF_DIR/state}"
TF_LOG_DIR="${TF_LOG_DIR:-$TF_STATE_DIR/logs}"
TF_PROMPT_DIR="${TF_PROMPT_DIR:-$TF_DIR/prompts}"

# The git repo being modified. Defaults to 2 levels up from taskfleet/ (typical
# layout: repo/scripts/taskfleet/). Override via TF_REPO_DIR env var.
TF_REPO_DIR="${TF_REPO_DIR:-$(cd "$TF_DIR/../.." && pwd)}"

# Worktrees live inside the repo at .tf-worktrees/ (gitignored).
# Override via TF_WORKTREE_ROOT if you prefer them outside the repo.
TF_WORKTREE_ROOT="${TF_WORKTREE_ROOT:-$TF_REPO_DIR/.tf-worktrees}"

# Config files
WORKERS_JSON="$TF_CONFIG_DIR/workers.json"
TASKS_JSON="$TF_CONFIG_DIR/tasks.json"
STATUS_JSON="$TF_STATE_DIR/task-status.json"
RUNSTATE_JSON="$TF_STATE_DIR/run-state.json"   # pid/workers-in-use, transient

# Branch prefix for agent work
TF_BRANCH_PREFIX="${TF_BRANCH_PREFIX:-tf}"

# Ensure runtime dirs exist
mkdir -p "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_WORKTREE_ROOT"

# Optional: export extra env vars for acceptance gates.
# Set TF_GATE_ENV in your project's .env or shell to inject project-specific
# variables (e.g. RUSTUP_TOOLCHAIN=nightly, NODE_ENV=test).
if [[ -n "${TF_GATE_ENV:-}" ]]; then
  eval "export $TF_GATE_ENV"
fi

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------
tf_log() {
  local level="$1"; shift
  local ts
  ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '%s [%s] %s\n' "$ts" "$level" "$*" >&2
}
tf_info()  { tf_log "INFO"  "$@"; }
tf_warn()  { tf_log "WARN"  "$@"; }
tf_error() { tf_log "ERROR" "$@"; }

# Backward-compatible aliases (wo_* → tf_*)
wo_log()  { tf_log  "$@"; }
wo_info() { tf_info "$@"; }
wo_warn() { tf_warn "$@"; }
wo_error(){ tf_error "$@"; }

# ---------------------------------------------------------------------------
# JSON helpers (require jq)
# ---------------------------------------------------------------------------
tf_require_jq() {
  if ! command -v jq >/dev/null 2>&1; then
    tf_error "jq is required but not on PATH. Install: apt install jq"
    return 1
  fi
}
wo_require_jq() { tf_require_jq "$@"; }

# Read a field from tasks.json for a given task id (raw, preserving type).
#   tf_task_field <task_id> <jq_path>   e.g. tf_task_field DM-3 .accept
tf_task_field() {
  jq -r --arg id "$1" '.tasks[] | select(.id==$id) | '"$2" "$TASKS_JSON"
}
wo_task_field() { tf_task_field "$@"; }

# Get the full task object as JSON
tf_task_json() {
  jq --arg id "$1" '.tasks[] | select(.id==$id)' "$TASKS_JSON"
}
wo_task_json() { tf_task_json "$@"; }

# List all task ids
tf_all_task_ids() {
  jq -r '.tasks[].id' "$TASKS_JSON"
}
wo_all_task_ids() { tf_all_task_ids "$@"; }

# Worker lookup: tf_worker <name> → JSON object
tf_worker() {
  jq --arg name "$1" '.workers[] | select(.name==$name and .enabled==true)' "$WORKERS_JSON"
}
wo_worker() { tf_worker "$@"; }

tf_worker_field() {
  jq -r --arg name "$1" '.workers[] | select(.name==$name) | '"$2" "$WORKERS_JSON"
}
wo_worker_field() { tf_worker_field "$@"; }

# List enabled worker names
tf_worker_names() {
  jq -r '.workers[] | select(.enabled==true) | .name' "$WORKERS_JSON"
}
wo_worker_names() { tf_worker_names "$@"; }

# Default config value
tf_default() {
  jq -r --arg k "$1" '.defaults[$k] // empty' "$WORKERS_JSON"
}
wo_default() { tf_default "$@"; }
