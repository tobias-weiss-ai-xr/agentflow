#!/usr/bin/env bash
# transparency.sh — pipeline transparency log and decision log.
#
# Inspired by moe-sovereign's pipeline transparency log and decision log
# with mandatory rationale. Implemented as append-only TSV (O(1) writes,
# grep/awk for reads) — zero jq on the hot path.
#
# Two logs:
#   dispatch.tsv  — per-dispatch events (dispatch, gate, retry, merge, trust)
#   decisions.tsv — per-decision events with mandatory rationale
#
# Env: TF_TRANSPARENCY_LOG (directory, default: $TF_STATE_DIR/logs)

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

TF_TRANSPARENCY_LOG="${TF_TRANSPARENCY_LOG:-$TF_LOG_DIR}"
mkdir -p "$TF_TRANSPARENCY_LOG"

# ---------------------------------------------------------------------------
# Dispatch log (append-only TSV)
# ---------------------------------------------------------------------------

# tf_log_dispatch <task_id> <event> <details...>
# Events: dispatch, gate, retry, merge, merge_fail, no_op, trust, scope_violation
# Format: timestamp\ttask_id\tevent\tworker\ttier\tattempt\tdetails
tf_log_dispatch() {
  local id="$1" event="$2"; shift 2
  local details="$*"
  local worker tier attempt
  # Read from cache if available (hot path), otherwise from status
  if tf_cache_valid 2>/dev/null; then
    worker="$(tf_cache_status_get "$id" .worker 2>/dev/null || echo "")"
  else
    worker="$(tf_status_get "$id" .worker 2>/dev/null || echo "")"
  fi
  attempt="$(tf_status_get "$id" .attempts 2>/dev/null || echo 0)"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$id" "$event" "${worker:-}" "${tier:-}" "${attempt:-0}" "$details" \
    >> "$TF_TRANSPARENCY_LOG/dispatch.tsv"
}

# tf_log_dispatch_raw <timestamp> <task_id> <event> <worker> <tier> <attempt> <details>
# Low-level append for callers that already have the data.
tf_log_dispatch_raw() {
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$@" >> "$TF_TRANSPARENCY_LOG/dispatch.tsv"
}

# ---------------------------------------------------------------------------
# Decision log (append-only TSV with mandatory rationale)
# ---------------------------------------------------------------------------

# tf_log_decision <task_id> <type> <rationale> [metadata...]
# Types: DISPATCH, RETRY, MERGE_SKIP, SCOPE_DEFER, TIER_OVERRIDE, AFFINITY,
#        GATE_FAIL, GATE_PASS, WORKER_SKIP, SPECULATIVE, AUTO_RETRY
tf_log_decision() {
  local id="$1" type="$2" rationale="$3"; shift 3 2>/dev/null || true
  local metadata="${*:-}"
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$id" "$type" "$rationale" "$metadata" \
    >> "$TF_TRANSPARENCY_LOG/decisions.tsv"
}

# ---------------------------------------------------------------------------
# Diagnostics / reporting
# ---------------------------------------------------------------------------

# tf_dispatch_report → summarize dispatch log by worker, event, and timing
tf_dispatch_report() {
  local log="$TF_TRANSPARENCY_LOG/dispatch.tsv"
  [[ ! -f "$log" ]] && { echo "no dispatch log found"; return; }
  echo "=== Dispatch Log Report ==="
  echo "Total events: $(wc -l < "$log")"
  echo ""
  echo "By event type:"
  awk -F'\t' '{count[$3]++} END{for (e in count) printf "  %-20s %d\n", e, count[e]}' "$log" | sort
  echo ""
  echo "By worker:"
  awk -F'\t' '$4 != "" {count[$4]++} END{for (w in count) printf "  %-20s %d\n", w, count[w]}' "$log" | sort
  echo ""
  echo "Recent events (last 10):"
  tail -10 "$log" | awk -F'\t' '{printf "  %s %s %s %s\n", $1, $2, $3, $7}'
}

# tf_decision_report → summarize decision log
tf_decision_report() {
  local log="$TF_TRANSPARENCY_LOG/decisions.tsv"
  [[ ! -f "$log" ]] && { echo "no decision log found"; return; }
  echo "=== Decision Log Report ==="
  echo "Total decisions: $(wc -l < "$log")"
  echo ""
  echo "By type:"
  awk -F'\t' '{count[$3]++} END{for (t in count) printf "  %-20s %d\n", t, count[t]}' "$log" | sort
  echo ""
  echo "Recent decisions (last 10):"
  tail -10 "$log" | awk -F'\t' '{printf "  %s [%s] %s — %s\n", $1, $2, $3, $4}'
}
