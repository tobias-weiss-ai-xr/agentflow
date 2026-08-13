#!/usr/bin/env bash
# orchestrator.sh — main loop for taskfleet.
#
# Continuously dispatches ready tasks to free workers until all are done (or
# permanently failed / blocked). Each worker runs in an isolated git worktree.
#
# Usage:
#   orchestrator.sh                 # run until done
#   orchestrator.sh --once          # one dispatch round, then exit
#   orchestrator.sh --dry-run       # show what would run, change nothing
#   orchestrator.sh --status        # print the status board and exit
#   orchestrator.sh --worker NAME   # restrict to a single worker
#   orchestrator.sh --task ID       # dispatch exactly one task (ignore others)
#   orchestrator.sh --poll SECONDS  # sleep between rounds (default 15)
#
# Env:
#   TF_MAX_PARALLEL  (default = number of enabled workers)
#   TF_MAX_ROUNDS    (default unlimited)
#   TF_MERGE_LOCK    (default state/merge.lock)

set -uo pipefail

TF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$TF_DIR/lib/common.sh"
# shellcheck source=lib/status.sh
. "$TF_DIR/lib/status.sh"
# shellcheck source=lib/worktree.sh
. "$TF_DIR/lib/worktree.sh"
# shellcheck source=lib/dispatch.sh
. "$TF_DIR/lib/dispatch.sh"

tf_require_jq || exit 1
command -v pi >/dev/null 2>&1       || { tf_error "pi not on PATH"; exit 1; }
git -C "$TF_REPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
  || { tf_error "TF_REPO_DIR ($TF_REPO_DIR) is not a git repository. Set TF_REPO_DIR to the repo root."; exit 1; }

# --- Startup cleanup: ensure main worktree is pristine ---
# Failed merges or interrupted runs can leave modified tracked files and
# untracked artifacts in the main checkout. These block subsequent merges
# ("Your local changes would be overwritten") and cascade into deadlocks.
# worktrees/ and state/ are gitignored, so git clean won't touch them.
(
  cd "$TF_REPO_DIR"
  git checkout --quiet main 2>/dev/null || true
  if [[ -n "$(git status --porcelain)" ]]; then
    tf_warn "main worktree was dirty at startup — cleaning"
    git reset --hard --quiet HEAD
    git clean --quiet -fd
  fi
)

# ---------------------------------------------------------------------------
# Args
# ---------------------------------------------------------------------------
TF_MODE="run"            # run|once|dry-run|status
TF_POLL=15
TF_WORKER_FILTER=""
TF_TASK_FILTER=""
TF_MAX_ROUNDS=0          # 0 = unlimited
TF_MAX_PARALLEL="${TF_MAX_PARALLEL:-$(tf_worker_names | wc -l)}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --once)     TF_MODE="once"; shift ;;
    --dry-run)  TF_MODE="dry-run"; shift ;;
    --status)   TF_MODE="status"; shift ;;
    --worker)   TF_WORKER_FILTER="$2"; shift 2 ;;
    --task)     TF_TASK_FILTER="$2"; shift 2 ;;
    --poll)     TF_POLL="$2"; shift 2 ;;
    --max-rounds) TF_MAX_ROUNDS="$2"; shift 2 ;;
    --max-parallel) TF_MAX_PARALLEL="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
      exit 0 ;;
    *) tf_error "unknown arg: $1"; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------
# Entry
# ---------------------------------------------------------------------------

# Robustness: kill stale orchestrator/pi processes from interrupted runs
# BEFORE touching any state, so orphaned dispatch subshells can't pollute
# this run (duplicate work, phantom run-state, deadlocks).
tf_kill_stale_processes

tf_status_init

if [[ "$TF_MODE" == "status" ]]; then
  tf_status_board
  exit 0
fi

# Robustness: recover from interrupted-run artifacts — stale worktrees and
# branches (kill -9 leftovers break fresh worktree creation) and run-state
# entries whose dispatch process is dead (phantom "running" tasks).
# Preserve branches flagged for conflict-retry (keep-branch).
local_preserved="$(jq -r 'to_entries[] | select(.value.last_error != null and (.value.last_error | contains("merge conflict"))) | .value.branch' "$STATUS_JSON" 2>/dev/null | grep -v '^null$' | tr '\n' ' ')"
tf_recover_stale_worktrees "$local_preserved"
tf_reset_dead_runstate

# Robustness: validate the task ledger (broken gates / unknown deps cost
# attempts otherwise). Advisory — warn but don't abort.
tf_validate_tasks || true

TF_MERGE_LOCK="${TF_MERGE_LOCK:-$TF_STATE_DIR/merge.lock}"
mkdir -p "$TF_STATE_DIR"

tf_info "taskfleet starting — mode=$TF_MODE poll=${TF_POLL}s parallel=$TF_MAX_PARALLEL"

# ---------------------------------------------------------------------------
# Worker availability: a worker is free iff not currently assigned to a
# running/verifying task. We track in-process pids in run-state.json.
# ---------------------------------------------------------------------------
tf_runstate_init() {
  [[ -f "$RUNSTATE_JSON" ]] || echo '{}' > "$RUNSTATE_JSON"
}

# record a running task: tf_runstate_set <task_id> <pid> <worker>
tf_runstate_set() {
  local id="$1" pid="$2" worker="$3"
  local tmp
  tmp="$(mktemp)"
  jq --arg id "$id" --arg pid "$pid" --arg w "$worker" \
    '.[$id] = {pid: ($pid|tonumber), worker: $w, started: now | todate}' \
    "$RUNSTATE_JSON" > "$tmp"
  mv "$tmp" "$RUNSTATE_JSON"
}

# remove a task from run-state
tf_runstate_clear() {
  local id="$1"
  local tmp
  tmp="$(mktemp)"
  jq --arg id "$id" 'del(.[$id])' "$RUNSTATE_JSON" > "$tmp"
  mv "$tmp" "$RUNSTATE_JSON"
}

# list worker names currently busy
tf_busy_workers() {
  tf_require_jq || return 1
  jq -r '[.[] | .worker] | unique | .[]' "$RUNSTATE_JSON" 2>/dev/null
}

# count in-flight tasks
tf_inflight_count() {
  jq 'length' "$RUNSTATE_JSON" 2>/dev/null || echo 0
}

# A worker is free if enabled, in filter, and not busy.
tf_free_workers() {
  local busy
  busy="$(tf_busy_workers)"
  while IFS= read -r w; do
    [[ -z "$w" ]] && continue
    [[ -n "$TF_WORKER_FILTER" && "$w" != "$TF_WORKER_FILTER" ]] && continue
    # not in busy list?
    if ! grep -qxF "$w" <<< "$busy"; then
      echo "$w"
    fi
  done < <(tf_worker_names)
}

# ---------------------------------------------------------------------------
# Reap finished background dispatches.
# ---------------------------------------------------------------------------
tf_reap() {
  local id pid status
  # iterate over a snapshot so we can mutate run-state
  local ids
  ids="$(jq -r 'keys[]' "$RUNSTATE_JSON" 2>/dev/null)" || return 0
  while IFS= read -r id; do
    [[ -z "$id" ]] && continue
    pid="$(jq -r --arg id "$id" '.[$id].pid' "$RUNSTATE_JSON")"
    if ! kill -0 "$pid" 2>/dev/null; then
      # process finished — wait to reap zombie + get status
      wait "$pid" 2>/dev/null
      local rc=$?
      status="$(tf_status_get "$id" .status)"
      tf_info "reaped $id (pid $pid, rc=$rc, status=$status)"
      tf_runstate_clear "$id"
    fi
  done <<< "$ids"
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------
tf_run() {
  local round=0
  while true; do
    round=$((round + 1))
    tf_reap

    # termination check
    local done_n failed_n running_n ready_n total_n
    done_n="$(tf_count_status done)"
    failed_n="$(tf_count_status failed)"
    running_n="$(tf_count_status running)"
    total_n="$(jq '[to_entries[] | select(.value | type=="object" and has("status"))] | length' "$STATUS_JSON")"

    tf_info "round $round — done=$done_n failed=$failed_n running=$running_n total=$total_n"

    if [[ $done_n -eq $total_n ]]; then
      tf_info "ALL TASKS DONE 🎉"
      tf_status_board
      return 0
    fi

    # detect deadlock: nothing running, nothing ready, not all done
    local ready_ids
    if [[ -n "$TF_TASK_FILTER" ]]; then
      tf_is_ready "$TF_TASK_FILTER" && ready_ids="$TF_TASK_FILTER" || ready_ids=""
    else
      ready_ids="$(tf_ready_task_ids)"
    fi
    local inflight
    inflight="$(tf_inflight_count)"

    if [[ -z "$ready_ids" && "$inflight" -eq 0 ]]; then
      # Nothing dispatchable and nothing in flight.
      # Check whether this is a true deadlock (failed tasks blocking ready)
      # or just permanent failures (no tasks can ever become ready).
      local blocked_by_failed blocked_n perm_failed_n
      perm_failed_n="$(tf_count_status failed)"

      if [[ $perm_failed_n -gt 0 ]]; then
        # Robustness (L6): before declaring deadlock, give INFRA-failed tasks
        # (worktree creation, gate env, provider health) a graceful retry —
        # these are transient and shouldn't permanently block the pipeline.
        # Only model-failure categories (compile/test/no_op) stay failed.
        local auto_retry=0
        while IFS= read -r fid; do
          [[ -z "$fid" ]] && continue
          local cat2 att2
          cat2="$(tf_status_get "$fid" .error_category)"
          att2="$(tf_status_get "$fid" .attempts)"
          case "$cat2" in
            unknown|rate_limit|auth_error|network_error|missing_lib_target)
              tf_warn "$fid: infra/provider failure [$cat2] — auto-resetting for graceful retry"
              tf_status_set "$fid" ready '.attempts=0 | .last_error=null | .next_retry_at=null | .error_category=null | .error_summary=null' 2>/dev/null || true
              auto_retry=1
              ;;
          esac
        done < <(jq -r 'to_entries[] | select(.value.status=="failed") | .key' "$STATUS_JSON")
        if [[ "$auto_retry" -eq 1 ]]; then
          tf_info "auto-reset infra-failed tasks; continuing (round $round)"
          continue
        fi
        tf_error "DEADLOCK: $perm_failed_n task(s) permanently failed, blocking $((total_n - done_n - perm_failed_n)) remaining"
        tf_status_board

        # Print per-failure diagnosis
        tf_info "=== Failure diagnosis ==="
        while IFS= read -r fid; do
          [[ -z "$fid" ]] && continue
          local cat sum att
          cat="$(tf_status_get "$fid" .error_category)"
          sum="$(tf_status_get "$fid" .error_summary)"
          att="$(tf_status_get "$fid" .attempts)"
          tf_info "$fid (attempt $att): [$cat] $sum"
          # Show what downstream tasks are blocked
          while IFS= read -r tid2; do
            [[ -z "$tid2" ]] && continue
            local deps2
            deps2="$(tf_task_field "$tid2" '.deps[]')"
            grep -qxF "$fid" <<< "$deps2" && tf_info "  └─ blocks $tid2"
          done < <(tf_all_task_ids)
        done < <(jq -r 'to_entries[] | select(.value.status=="failed") | .key' "$STATUS_JSON")
        tf_info "=== End diagnosis ==="
        return 1
      else
        tf_warn "no ready or running tasks but not all done — possible blocked dependency"
        tf_status_board
        return 1
      fi
    fi

    # dispatch: pair ready tasks with free workers, up to TF_MAX_PARALLEL
    if [[ "$TF_MODE" == "dry-run" ]]; then
      local printed=0
      while IFS= read -r tid; do
        [[ -z "$tid" ]] && continue
        # pick first free worker for display
        local fw
        fw="$(tf_free_workers | head -1)"
        tf_dispatch_one_dryrun "$tid" "${fw:-<any>}"
        printed=$((printed + 1))
      done <<< "$ready_ids"
      [[ $printed -eq 0 ]] && tf_info "(no ready tasks right now)"
      return 0
    fi

    while IFS= read -r tid; do
      [[ -z "$tid" ]] && continue
      [[ "$inflight" -ge "$TF_MAX_PARALLEL" ]] && break
      # pick the first HEALTHY free worker (skip dead/rate-limited endpoints)
      local fw
      while IFS= read -r cand; do
        [[ -z "$cand" ]] && continue
        if tf_worker_healthy "$cand"; then
          fw="$cand"; break
        else
          tf_warn "worker $cand endpoint unhealthy — skipping this round"
        fi
      done < <(tf_free_workers)
      [[ -z "$fw" ]] && { tf_info "no free healthy workers, waiting"; break; }
      # dispatch in background. Fine-grained locks (status + merge) protect
      # the shared state; the long pi run is fully parallel across worktrees.
      tf_dispatch_one "$tid" "$fw" &
      local bg_pid=$!
      tf_runstate_set "$tid" "$bg_pid" "$fw"
      tf_info "launched $tid on worker=$fw (pid $bg_pid)"
      inflight=$((inflight + 1))
      sleep 1   # stagger launches so worktree creation doesn't race
    done <<< "$ready_ids"
    if [[ "$TF_MODE" == "once" ]]; then
      tf_info "--once: dispatched one round, exiting"
      return 0
    fi
    if [[ "$TF_MAX_ROUNDS" -gt 0 && "$round" -ge "$TF_MAX_ROUNDS" ]]; then
      tf_info "reached --max-rounds $TF_MAX_ROUNDS, exiting"
      return 0
    fi

    sleep "$TF_POLL"
  done
}

tf_runstate_init
tf_run
