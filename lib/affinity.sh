#!/usr/bin/env bash
# affinity.sh — worker-task affinity routing for taskfleet.
#
# Learns per-worker, per-engine win rates from the receipt ledger and uses
# them to route tasks to the worker most likely to succeed on that task type.
# Falls back to config order when no history exists.
#
# A "win" is a task whose final_status (from the receipt `closed` record) is
# "done". The worker is attributed from the last `begin` record for that task
# (the attempt that produced the final outcome). Engines come from tasks.json.
#
# Two scoring modes:
#   TF_AFFINITY=1 (default) — raw win-rate (greedy, original behavior)
#   TF_AFFINITY=2           — UCB1 (Upper Confidence Bound, exploration+exploitation)
#
# UCB1 replaces Thompson sampling (which requires python3's random.betavariate,
# 96ms/call) with pure awk arithmetic (~5ms/call). UCB1 gives the same
# exploration/exploitation balance: score = mean + sqrt(2 * ln(N) / n).
# Cold start: score = 1.0 (forces exploration of untried workers).

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

TF_RECEIPT_DIR="${TF_RECEIPT_DIR:-$TF_STATE_DIR/receipts}"

# ---------------------------------------------------------------------------
# Receipt aggregation: build a per-(worker, engine) outcome table.
# Each line: "worker<TAB>engine<TAB>wins<TAB>total"
# ---------------------------------------------------------------------------
# tf_affinity_table → prints TSV of worker, engine, wins, total across all
# receipt files. Uses only closed records for the outcome and the last begin
# record per task for attribution.
tf_affinity_table() {
  tf_require_jq || return 1
  local files
  files="$(ls "$TF_RECEIPT_DIR"/*.ndjson 2>/dev/null)"
  [[ -z "$files" ]] && return 0

  # Read all receipts into one JSON array, then compute per-task:
  #   final_status from the closed record
  #   worker from the LAST begin record for that task
  # Then join with tasks.json engines and aggregate.
  jq -s --argfile tasks "$TASKS_JSON" '
    def task_engine($id):
      ($tasks.tasks[] | select(.id == $id) | .engine // "unknown") // "unknown";
    # group receipts by task
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
  ' -r $files 2>/dev/null
}

# ---------------------------------------------------------------------------
# tf_affinity_score <worker> <task_id> → prints a fractional score in [0,1].
#
# Returns the win rate for that worker on the task's engine, falling back to:
#   - the worker's overall win rate if no engine-specific history
#   - 0.5 (neutral) if no history at all
#
# When TF_AFFINITY=2 (UCB1 mode), returns UCB1 score instead of raw win-rate.
# UCB1: score = mean + sqrt(2 * ln(N_total) / n), where N_total = total
# observations across all workers for this engine. Cold start: 1.0.
# Uses cached affinity table when available (awk, ~15ms) instead of
# rebuilding the table per call (jq, ~251ms).
# ---------------------------------------------------------------------------
tf_affinity_score() {
  local worker="$1" task_id="$2"
  local engine
  # Use cache for engine lookup (awk, ~1ms) when available
  if tf_cache_valid 2>/dev/null; then
    engine="$(tf_cache_task_field "$task_id" 2)"
  else
    engine="$(tf_task_field "$task_id" .engine 2>/dev/null || echo "unknown")"
  fi
  [[ -z "$engine" ]] && engine="unknown"

  # UCB1 mode (TF_AFFINITY=2): exploration + exploitation
  if [[ "${TF_AFFINITY:-1}" == "2" ]]; then
    tf_ucb1_score "$worker" "$task_id" "$engine"
    return
  fi

  # Default mode (TF_AFFINITY=1): raw win-rate
  local row engine_wins engine_total all_wins all_total
  # Use cached affinity table when available (avoids rebuilding per call)
  if tf_cache_valid 2>/dev/null && [[ -s "$TF_CACHE_DIR/affinity.tsv" ]]; then
    row="$(awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $3"\t"$4; exit}' "$TF_CACHE_DIR/affinity.tsv")"
  else
    row="$(tf_affinity_table | awk -F'\t' -v w="$worker" -v e="$engine" \
      '$1==w && $2==e {print $3"\t"$4}')"
  fi
  if [[ -n "$row" ]]; then
    engine_wins="${row%%$'\t'*}"
    engine_total="${row##*$'\t'}"
    if [[ "$engine_total" -gt 0 ]]; then
      echo "$(LC_ALL=C awk "BEGIN{printf \"%.3f\", $engine_wins/$engine_total}")"
      return
    fi
  fi
  # Fall back to worker's overall win rate
  if tf_cache_valid 2>/dev/null && [[ -s "$TF_CACHE_DIR/affinity.tsv" ]]; then
    row="$(awk -F'\t' -v w="$worker" '$1==w {wins+=$3; total+=$4} END{print wins"\t"total}' "$TF_CACHE_DIR/affinity.tsv")"
  else
    row="$(tf_affinity_table | awk -F'\t' -v w="$worker" \
      '$1==w {wins+=$3; total+=$4} END{print wins"\t"total}')"
  fi
  if [[ -n "$row" ]]; then
    all_wins="${row%%$'\t'*}"
    all_total="${row##*$'\t'}"
    if [[ "$all_total" -gt 0 ]]; then
      echo "$(LC_ALL=C awk "BEGIN{printf \"%.3f\", $all_wins/$all_total}")"
      return
    fi
  fi
  echo "0.500"
}

# ---------------------------------------------------------------------------
# tf_ucb1_score <worker> <task_id> <engine> → UCB1 score (higher = better)
# Pure awk arithmetic — no python3, no jq. ~5ms per call.
# UCB1: score = mean + sqrt(2 * ln(N_total) / n)
# Cold start: score = 1.0 (forces exploration of untried workers).
# ---------------------------------------------------------------------------
tf_ucb1_score() {
  local worker="$1" task_id="$2" engine="${3:-unknown}"
  local wins total n_total
  # Use cached affinity table when available
  if tf_cache_valid 2>/dev/null && [[ -s "$TF_CACHE_DIR/affinity.tsv" ]]; then
    wins="$(awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $3; exit}' "$TF_CACHE_DIR/affinity.tsv")"
    total="$(awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $4; exit}' "$TF_CACHE_DIR/affinity.tsv")"
    n_total="$(awk -F'\t' -v e="$engine" '$2==e {sum+=$4} END{print sum+0}' "$TF_CACHE_DIR/affinity.tsv")"
  else
    local row
    row="$(tf_affinity_table | awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $3"\t"$4}')"
    wins="${row%%$'\t'*}"
    total="${row##*$'\t'}"
    n_total="$(tf_affinity_table | awk -F'\t' -v e="$engine" '$2==e {sum+=$4} END{print sum+0}')"
  fi
  wins="${wins:-0}"
  total="${total:-0}"
  # Cold start: no history → prioritize exploration
  if [[ "$total" -eq 0 ]]; then
    echo "1.000"
    return
  fi
  # N_total = total observations across all workers for this engine
  [[ "$n_total" -le 0 ]] && n_total=1
  # UCB1: mean + sqrt(2 * ln(N) / n) — pure awk, no python3
  LC_ALL=C awk -v w="$wins" -v n="$total" -v N="$n_total" \
    'BEGIN{printf "%.3f", w/n + sqrt(2 * log(N) / n)}'
}

# ---------------------------------------------------------------------------
# tf_best_worker_for <task_id> <candidate workers...> → prints best worker name
#
# Picks the candidate with the highest affinity score for the task. Ties break
# in the order candidates are given (which mirrors config order — a stable
# fallback). If no candidates, prints nothing.
# ---------------------------------------------------------------------------
tf_best_worker_for() {
  local task_id="$1"; shift
  local best="" best_score="-1"
  local cand score
  for cand in "$@"; do
    [[ -z "$cand" ]] && continue
    score="$(tf_affinity_score "$cand" "$task_id")"
    # awk float comparison; score is like 0.500
    if LC_ALL=C awk "BEGIN{exit !($score > $best_score)}"; then
      best="$cand"
      best_score="$score"
    fi
  done
  [[ -n "$best" ]] && echo "$best"
}

# ---------------------------------------------------------------------------
# tf_affinity_rank <task_id> <candidate workers...> → prints ranking (best first)
# ---------------------------------------------------------------------------
tf_affinity_rank() {
  local task_id="$1"; shift
  local cand score
  for cand in "$@"; do
    [[ -z "$cand" ]] && continue
    score="$(tf_affinity_score "$cand" "$task_id")"
    printf '%s\t%s\n' "$cand" "$score"
  done | LC_ALL=C sort -t$'\t' -k2,2nr | cut -f1
}
