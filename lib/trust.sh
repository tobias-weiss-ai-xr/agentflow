#!/usr/bin/env bash
# trust.sh — trust score for gate verification.
#
# Inspired by moe-sovereign's trust_score.py (weighted [0.0-1.0] score with
# PROCEED/REVIEW/BLOCK buckets) but implemented as pure bash arithmetic.
# No jq, no python3, no external calls — runs in <1ms.
#
# Computed after gate verification (off-hot-path, gate already took seconds).
# Factors:
#   gate exit code    (35%) — did the acceptance gate pass?
#   scope adherence   (25%) — did the diff touch only declared scope files?
#   test coverage     (20%) — ratio of tests run to tests passed (if available)
#   retry count       (10%) — fewer retries = higher trust
#   worker affinity   (10%) — historical win rate for this engine
#
# Buckets:
#   trusted  (≥0.80) — auto-merge
#   review   (0.50-0.79) — merge but flag for review
#   blocked  (<0.50) — do not merge, retry or fail
#
# Env: TF_TRUST_THRESHOLD_REVIEW=0.50, TF_TRUST_THRESHOLD_TRUST=0.80

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# tf_trust_score <task_id> <gate_passed> <scope_ok> <retries> <worker> <engine>
#   gate_passed: 1 if gate passed, 0 if failed
#   scope_ok: 1 if no out-of-scope edits, 0 if violations
#   retries: number of previous failed attempts
#   worker: worker name (for affinity lookup)
#   engine: task engine (for affinity lookup)
# Prints integer score 0-100 (percentage).
tf_trust_score() {
  local id="$1" gate_passed="$2" scope_ok="$3" retries="${4:-0}" worker="${5:-}" engine="${6:-}"
  local score=0
  # Gate: 35 points
  [[ "$gate_passed" == "1" ]] && score=$((score + 35))
  # Scope adherence: 25 points
  [[ "$scope_ok" == "1" ]] && score=$((score + 25))
  # Retry penalty: 10 points max, -3 per retry (min 0)
  local retry_pts=10
  retry_pts=$((retry_pts - retries * 3))
  [[ $retry_pts -lt 0 ]] && retry_pts=0
  score=$((score + retry_pts))
  # Worker affinity: 10 points (win rate * 10, capped at 10)
  if [[ -n "$worker" ]]; then
    local affinity=50
    if [[ -n "$engine" ]]; then
      if tf_cache_valid 2>/dev/null && [[ -s "$TF_CACHE_DIR/affinity.tsv" ]]; then
        local wins total
        wins="$(awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $3; exit}' "$TF_CACHE_DIR/affinity.tsv" 2>/dev/null)"
        total="$(awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $4; exit}' "$TF_CACHE_DIR/affinity.tsv" 2>/dev/null)"
        if [[ -n "$total" && "$total" -gt 0 ]]; then
          affinity=$((wins * 100 / total))
        fi
      fi
    fi
    local aff_pts=$((affinity / 10))
    [[ $aff_pts -gt 10 ]] && aff_pts=10
    score=$((score + aff_pts))
  fi
  # Test coverage: 20 points (if gate passed and we can extract test count)
  # For now, give full 20 if gate passed (test coverage extraction is gate-specific)
  if [[ "$gate_passed" == "1" ]]; then
    score=$((score + 20))
  fi
  # Cap at 100
  [[ $score -gt 100 ]] && score=100
  echo "$score"
}

# tf_trust_bucket <score> → prints bucket name
tf_trust_bucket() {
  local score="$1"
  local trust_threshold="${TF_TRUST_THRESHOLD_TRUST:-80}"
  local review_threshold="${TF_TRUST_THRESHOLD_REVIEW:-50}"
  if [[ "$score" -ge "$trust_threshold" ]]; then
    echo "trusted"
  elif [[ "$score" -ge "$review_threshold" ]]; then
    echo "review"
  else
    echo "blocked"
  fi
}

# tf_trust_evaluate <task_id> <gate_passed> <scope_violations> <retries> <worker> <engine>
#   Computes trust score, prints "score bucket" (e.g., "85 trusted").
#   Also writes to the transparency log if enabled.
tf_trust_evaluate() {
  local id="$1" gate_passed="$2" scope_violations="${3:-0}" retries="${4:-0}" worker="${5:-}" engine="${6:-}"
  local scope_ok=1
  [[ "$scope_violations" -gt 0 ]] && scope_ok=0
  local score
  score="$(tf_trust_score "$id" "$gate_passed" "$scope_ok" "$retries" "$worker" "$engine")"
  local bucket
  bucket="$(tf_trust_bucket "$score")"
  echo "$score $bucket"
  # Log to transparency log
  if [[ -n "${TF_TRANSPARENCY_LOG:-}" ]]; then
    printf '%s\t%s\ttrust\t%s\t%s\n' \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$id" "$score" "$bucket" \
      >> "${TF_TRANSPARENCY_LOG}/trust.tsv" 2>/dev/null
  fi
}
