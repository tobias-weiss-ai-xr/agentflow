#!/usr/bin/env bash
# cache.sh — TSV-based caching for the scheduling hot path.
#
# The scheduling loop calls tf_task_field, tf_status_get, tf_scope_files_for,
# and tf_affinity_score hundreds of times per round. Each call spawns a jq
# process (~251ms). With 20 tasks, that's 100+ jq calls = 25+ seconds per
# round — exceeding the 15s poll interval.
#
# This module builds flat TSV caches once per scheduling round, then provides
# grep/awk-based lookups (~1-15ms each) as drop-in replacements.
#
# Cache files live in $TF_STATE_DIR/cache/ and are rebuilt each round by
# tf_cache_build(). They are safe to delete between runs.
#
# Performance: 102 jq calls (25.6s) → 3 jq calls + ~30 grep/awk calls (~1.2s)
#
# Env vars:
#   TF_USE_CACHE=1  Enable cache-aware lookups (default: enabled when cache exists)

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

TF_CACHE_DIR="${TF_STATE_DIR:-$TF_DIR/state}/cache"
mkdir -p "$TF_CACHE_DIR"

# ---------------------------------------------------------------------------
# Cache builders (call once per scheduling round)
# ---------------------------------------------------------------------------

# tf_cache_build — rebuild all caches. Call at the start of each scheduling round.
# Cost: 3 jq calls (tasks + status + affinity) + 1 awk pass (scope) ≈ 800ms total.
# This replaces 100+ per-field jq calls that previously cost 25+ seconds.
tf_cache_build() {
  tf_cache_build_tasks
  tf_cache_build_status
  tf_cache_build_scope
  tf_cache_build_affinity
}

# Task cache: id\tengine\ttier\tdeps\tscope\taccept\ttitle\tndeps\tnscope
# One jq call replaces N × tf_task_field calls.
tf_cache_build_tasks() {
  jq -r '.tasks[] | [
    .id,
    (.engine // "t"),
    (.model_tier // "standard"),
    ((.deps // []) | join(",")),
    ((.scope // []) | join(",")),
    (.accept // ""),
    (.title // ""),
    ((.deps // []) | length | tostring),
    ((.scope // []) | length | tostring)
  ] | @tsv' "$TASKS_JSON" > "$TF_CACHE_DIR/tasks.tsv"
}

# Status cache: id\tstatus\tattempts\tnext_retry_at\terror_category\tworker\ttimeout_multiplier
# One jq call replaces N × tf_status_get calls.
tf_cache_build_status() {
  jq -r 'to_entries[] | select(.value | type=="object" and has("status")) | [
    .key,
    .value.status,
    (.value.attempts // 0 | tostring),
    (.value.next_retry_at // ""),
    (.value.error_category // ""),
    (.value.worker // ""),
    (.value.timeout_multiplier // "" | tostring)
  ] | @tsv' "$STATUS_JSON" > "$TF_CACHE_DIR/status.tsv"
}

# Scope cache: scope_file\ttask_id (for running/verifying tasks only)
# Built from task cache + status cache (no jq — pure awk).
tf_cache_build_scope() {
  : > "$TF_CACHE_DIR/scope.tsv"
  [[ ! -s "$TF_CACHE_DIR/status.tsv" ]] && return 0
  [[ ! -s "$TF_CACHE_DIR/tasks.tsv" ]] && return 0
  local id status attempts nr_err cat worker tmult scope files f
  while IFS=$'\t' read -r id status attempts nr_err cat worker tmult; do
    [[ "$status" == "running" || "$status" == "verifying" ]] || continue
    scope="$(awk -F'\t' -v id="$id" '$1==id {print $5; exit}' "$TF_CACHE_DIR/tasks.tsv" 2>/dev/null)"
    [[ -z "$scope" ]] && continue
    # Split scope on commas and emit each file
    local IFS=','
    read -ra files <<< "$scope"
    for f in "${files[@]}"; do
      [[ -n "$f" ]] && printf '%s\t%s\n' "$f" "$id"
    done
  done < "$TF_CACHE_DIR/status.tsv" >> "$TF_CACHE_DIR/scope.tsv"
}

# Affinity cache: worker\tengine\twins\ttotal
# One jq call (reads all receipts) replaces N × tf_affinity_table calls.
tf_cache_build_affinity() {
  local files
  files="$(ls "$TF_RECEIPT_DIR"/*.ndjson 2>/dev/null)"
  if [[ -z "$files" ]]; then
    : > "$TF_CACHE_DIR/affinity.tsv"
    return 0
  fi
  jq -s --argfile tasks "$TASKS_JSON" '
    def task_engine($id):
      ($tasks.tasks[] | select(.id == $id) | .engine // "unknown") // "unknown";
    map({key: .task_id, val: .}) | group_by(.key)
    | map({
        task_id: .[0].key,
        closed: ([.[].val | select(.type? == "closed")] | last),
        last_begin: ([.[].val | select(.type? == null and .status? == "running")] | last)
      })
    | map(select(.closed != null and .last_begin != null)
      | {
          worker: .last_begin.worker,
          engine: task_engine(.task_id),
          status: .closed.final_status
        })
    | group_by(.worker + "\u0000" + .engine)
    | map({
        worker: .[0].worker,
        engine: .[0].engine,
        wins: ([.[] | select(.status == "done")] | length),
        total: length
      })
    | .[]
    | "\(.worker)\t\(.engine)\t\(.wins)\t\(.total)"
  ' -r $files > "$TF_CACHE_DIR/affinity.tsv" 2>/dev/null
}

# ---------------------------------------------------------------------------
# Cache-aware lookups (grep/awk, ~1-15ms each)
# Fall back to jq if cache is not built (off-hot-path calls).
# ---------------------------------------------------------------------------

# tf_cache_valid → 0 if cache files exist and are usable
tf_cache_valid() {
  [[ -s "$TF_CACHE_DIR/tasks.tsv" ]] && [[ -s "$TF_CACHE_DIR/status.tsv" ]]
}

# tf_cache_task_field <task_id> <field_index>
# Field indices: 1=id, 2=engine, 3=tier, 4=deps, 5=scope, 6=accept, 7=title, 8=ndeps, 9=nscope
tf_cache_task_field() {
  local id="$1" idx="$2"
  if [[ ! -s "$TF_CACHE_DIR/tasks.tsv" ]]; then
    # Fall back to jq
    case "$idx" in
      2) tf_task_field "$id" .engine 2>/dev/null || echo "t" ;;
      3) tf_task_field "$id" .model_tier 2>/dev/null || echo "standard" ;;
      4) tf_task_field "$id" '.deps[]' 2>/dev/null ;;
      5) tf_task_field "$id" '.scope[]' 2>/dev/null ;;
      6) tf_task_field "$id" .accept 2>/dev/null ;;
      7) tf_task_field "$id" .title 2>/dev/null ;;
      8) tf_task_field "$id" '.deps | length' 2>/dev/null ;;
      9) tf_task_field "$id" '.scope | length' 2>/dev/null ;;
      *) return 1 ;;
    esac
    return
  fi
  awk -F'\t' -v id="$id" -v idx="$idx" '$1==id {print $idx; exit}' "$TF_CACHE_DIR/tasks.tsv"
}

# tf_cache_status_get <task_id> <field_name>
# Field names: .status, .attempts, .next_retry_at, .error_category, .worker, .timeout_multiplier
tf_cache_status_get() {
  local id="$1" field="$2"
  if [[ ! -s "$TF_CACHE_DIR/status.tsv" ]]; then
    tf_status_get "$id" "$field"
    return
  fi
  local idx
  case "$field" in
    .status)              idx=2 ;;
    .attempts)            idx=3 ;;
    .next_retry_at)       idx=4 ;;
    .error_category)      idx=5 ;;
    .worker)              idx=6 ;;
    .timeout_multiplier)  idx=7 ;;
    *) tf_status_get "$id" "$field"; return ;;
  esac
  awk -F'\t' -v id="$id" -v idx="$idx" '$1==id {print $idx; exit}' "$TF_CACHE_DIR/status.tsv"
}

# tf_cache_scope_files <task_id> → newline-separated scope files
tf_cache_scope_files() {
  local id="$1"
  if [[ ! -s "$TF_CACHE_DIR/tasks.tsv" ]]; then
    tf_scope_files_for "$id"
    return
  fi
  local scope
  scope="$(awk -F'\t' -v id="$id" '$1==id {print $5; exit}' "$TF_CACHE_DIR/tasks.tsv")"
  [[ -z "$scope" ]] && return 0
  echo "$scope" | tr ',' '\n'
}

# tf_cache_scope_conflicts <task_id> → running task IDs that share scope
# Uses the pre-built scope cache (awk on TSV, ~1ms) instead of N × jq.
tf_cache_scope_conflicts() {
  local id="$1"
  if [[ ! -s "$TF_CACHE_DIR/scope.tsv" ]]; then
    tf_scope_conflicts "$id"
    return
  fi
  local my_files
  my_files="$(tf_cache_scope_files "$id" | sort -u)"
  [[ -z "$my_files" ]] && return 0
  local seen=""
  while IFS=$'\t' read -r file running_id; do
    [[ "$running_id" == "$id" ]] && continue
    if echo "$my_files" | grep -qxF "$file"; then
      # Deduplicate running IDs
      if ! grep -qxF "$running_id" <<< "$seen"; then
        echo "$running_id"
        seen="$seen"$'\n'"$running_id"
      fi
    fi
  done < "$TF_CACHE_DIR/scope.tsv"
}

# tf_cache_is_ready <task_id> → 0 if ready, 1 if not
# Cache-aware version of tf_is_ready. Checks status and deps from TSV cache.
tf_cache_is_ready() {
  local id="$1"
  if ! tf_cache_valid; then
    tf_is_ready "$id"
    return $?
  fi
  local status
  status="$(awk -F'\t' -v id="$id" '$1==id {print $2; exit}' "$TF_CACHE_DIR/status.tsv")"
  if [[ "$status" != "ready" ]]; then
    # Check if failed with expired retry
    if [[ "$status" == "failed" ]]; then
      local nr
      nr="$(awk -F'\t' -v id="$id" '$1==id {print $4; exit}' "$TF_CACHE_DIR/status.tsv")"
      [[ -n "$nr" ]] || return 1
      [[ "$nr" < "$(date -u +%Y-%m-%dT%H:%M:%SZ)" ]] || return 1
      return 0
    fi
    return 1
  fi
  # All deps must be done
  local deps
  deps="$(awk -F'\t' -v id="$id" '$1==id {print $4; exit}' "$TF_CACHE_DIR/tasks.tsv")"
  [[ -z "$deps" ]] && return 0
  local all_done=1 dep dep_status
  local IFS=','
  read -ra dep_arr <<< "$deps"
  for dep in "${dep_arr[@]}"; do
    [[ -z "$dep" ]] && continue
    dep_status="$(awk -F'\t' -v id="$dep" '$1==id {print $2; exit}' "$TF_CACHE_DIR/status.tsv")"
    [[ "$dep_status" == "done" ]] || all_done=0
  done
  [[ $all_done -eq 1 ]]
}

# tf_cache_affinity_score <worker> <task_id> → score in [0,1]
# Uses cached affinity table (awk, ~15ms) instead of jq per call.
tf_cache_affinity_score() {
  local worker="$1" task_id="$2"
  local engine
  engine="$(tf_cache_task_field "$task_id" 2)"
  [[ -z "$engine" ]] && engine="unknown"
  if [[ ! -s "$TF_CACHE_DIR/affinity.tsv" ]]; then
    tf_affinity_score "$worker" "$task_id"
    return
  fi
  local row engine_wins engine_total all_wins all_total
  row="$(awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $3"\t"$4; exit}' "$TF_CACHE_DIR/affinity.tsv")"
  if [[ -n "$row" ]]; then
    engine_wins="${row%%$'\t'*}"
    engine_total="${row##*$'\t'}"
    if [[ "$engine_total" -gt 0 ]]; then
      LC_ALL=C awk "BEGIN{printf \"%.3f\", $engine_wins/$engine_total}"
      return
    fi
  fi
  # Fall back to worker's overall win rate
  row="$(awk -F'\t' -v w="$worker" '$1==w {wins+=$3; total+=$4} END{print wins"\t"total}' "$TF_CACHE_DIR/affinity.tsv")"
  if [[ -n "$row" ]]; then
    all_wins="${row%%$'\t'*}"
    all_total="${row##*$'\t'}"
    if [[ "$all_total" -gt 0 ]]; then
      LC_ALL=C awk "BEGIN{printf \"%.3f\", $all_wins/$all_total}"
      return
    fi
  fi
  echo "0.500"
}

# tf_cache_all_task_ids → all task IDs from cache
tf_cache_all_task_ids() {
  if [[ ! -s "$TF_CACHE_DIR/tasks.tsv" ]]; then
    tf_all_task_ids
    return
  fi
  awk -F'\t' '{print $1}' "$TF_CACHE_DIR/tasks.tsv"
}

# tf_cache_running_task_ids → IDs of running/verifying tasks from cache
tf_cache_running_task_ids() {
  if [[ ! -s "$TF_CACHE_DIR/status.tsv" ]]; then
    tf_running_task_ids
    return
  fi
  awk -F'\t' '$2=="running" || $2=="verifying" {print $1}' "$TF_CACHE_DIR/status.tsv"
}

# tf_cache_count_status <status> → count of tasks with given status
tf_cache_count_status() {
  local status="$1"
  if [[ ! -s "$TF_CACHE_DIR/status.tsv" ]]; then
    tf_count_status "$status"
    return
  fi
  awk -F'\t' -v s="$status" '$2==s {count++} END{print count+0}' "$TF_CACHE_DIR/status.tsv"
}

# tf_cache_task_depth <task_id> → depth from precomputed depths
tf_cache_task_depth() {
  local id="$1"
  if [[ ! -f "$TF_STATE_DIR/task-depths.json" ]]; then
    tf_get_task_depth "$id"
    return
  fi
  jq -r --arg id "$id" '.[$id] // 0' "$TF_STATE_DIR/task-depths.json" 2>/dev/null
}
