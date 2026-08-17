#!/usr/bin/env bash
# complexity.sh — heuristic complexity classification for auto-tier assignment.
#
# Inspired by moe-sovereign's complexity_estimator.py (AIC/zlib compressibility
# + token count + domain markers) but implemented in pure bash with gzip
# compressibility as a zero-LLM-call proxy.
#
# Classifies tasks into: trivial | moderate | complex
# Maps to model tiers:    booster  | standard  | deep
#
# Runs ONCE at task load (not per dispatch). Cached in the task cache.
# Env var: TF_AUTO_TIER=1 to enable automatic tier override.

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# tf_compute_complexity <description> <scope_count> <dep_count> → prints tier
# Heuristics:
#   - Word count > 50 AND scope_count > 5 → complex
#   - Compressibility ratio < 0.15 (information-dense) → complex
#   - Word count > 20 OR scope_count > 2 → moderate
#   - Dep count > 3 → moderate (complex dependency chain)
#   - Otherwise → trivial
tf_compute_complexity() {
  local desc="$1" scope_count="${2:-0}" dep_count="${3:-0}"
  local words compressed ratio
  words="$(echo "$desc" | wc -w)"
  # gzip compressibility: compressed_size / original_size
  # Low ratio = information-dense = complex
  local original_size
  original_size="$(echo "$desc" | wc -c)"
  compressed="$(echo "$desc" | gzip -c 2>/dev/null | wc -c)"
  if [[ "$original_size" -gt 0 ]]; then
    ratio="$(LC_ALL=C awk "BEGIN{printf \"%.2f\", $compressed / $original_size}")"
  else
    ratio="1.0"
  fi
  # Domain markers that indicate complexity
  local complex_markers=0
  echo "$desc" | grep -qiE 'refactor|architecture|security|migration|distribut|concurren|async|race.condition|deadlock' && complex_markers=1
  # Classification (use awk for float comparison)
  local is_complex is_moderate
  is_complex=$(awk "BEGIN{print ($words > 50 && $scope_count > 5) || ($ratio < 0.15) || ($complex_markers == 1 && $scope_count > 3) ? 1 : 0}")
  is_moderate=$(awk "BEGIN{print ($words > 20 || $scope_count > 2 || $dep_count > 3) ? 1 : 0}")
  if [[ "$is_complex" == "1" ]]; then
    echo "complex"
  elif [[ "$is_moderate" == "1" ]]; then
    echo "moderate"
  else
    echo "trivial"
  fi
}

# tf_complexity_to_tier <complexity> → prints model_tier
tf_complexity_to_tier() {
  case "$1" in
    trivial)  echo "booster" ;;
    moderate) echo "standard" ;;
    complex)  echo "deep" ;;
    *)        echo "standard" ;;
  esac
}

# tf_auto_tier <task_id> → prints recommended tier (or empty if auto-tier is off)
# Reads task description and scope from cache (or jq), computes complexity,
# maps to tier. Only active when TF_AUTO_TIER=1.
tf_auto_tier() {
  [[ "${TF_AUTO_TIER:-0}" == "1" ]] || return 0
  local id="$1" title scope_count dep_count
  if tf_cache_valid 2>/dev/null; then
    title="$(tf_cache_task_field "$id" 7)"
    scope_count="$(tf_cache_task_field "$id" 9)"
    dep_count="$(tf_cache_task_field "$id" 8)"
  else
    title="$(tf_task_field "$id" .title 2>/dev/null || echo "")"
    scope_count="$(tf_task_field "$id" '.scope | length' 2>/dev/null || echo 0)"
    dep_count="$(tf_task_field "$id" '.deps | length' 2>/dev/null || echo 0)"
  fi
  [[ -z "$title" ]] && return 0
  local complexity
  complexity="$(tf_compute_complexity "$title" "$scope_count" "$dep_count")"
  tf_complexity_to_tier "$complexity"
}

# tf_complexity_score <task_id> → numeric complexity score (0-100)
# Combines word count, scope count, dep count, and compressibility into a
# single integer score. Higher = more complex.
tf_complexity_score() {
  local id="$1" title scope_count dep_count
  if tf_cache_valid 2>/dev/null; then
    title="$(tf_cache_task_field "$id" 7)"
    scope_count="$(tf_cache_task_field "$id" 9)"
    dep_count="$(tf_cache_task_field "$id" 8)"
  else
    title="$(tf_task_field "$id" .title 2>/dev/null || echo "")"
    scope_count="$(tf_task_field "$id" '.scope | length' 2>/dev/null || echo 0)"
    dep_count="$(tf_task_field "$id" '.deps | length' 2>/dev/null || echo 0)"
  fi
  [[ -z "$title" ]] && { echo "0"; return; }
  local words compressed original_size ratio score
  words="$(echo "$title" | wc -w)"
  original_size="$(echo "$title" | wc -c)"
  compressed="$(echo "$title" | gzip -c 2>/dev/null | wc -c)"
  if [[ "$original_size" -gt 0 ]]; then
    ratio="$(LC_ALL=C awk "BEGIN{printf \"%.2f\", $compressed / $original_size}")"
  else
    ratio="1.0"
  fi
  # Score: word_count * 2 + scope_count * 5 + dep_count * 3 + (1 - ratio) * 20
  score=$(( words * 2 + scope_count * 5 + dep_count * 3 ))
  local density
  density="$(LC_ALL=C awk "BEGIN{printf \"%d\", (1 - $ratio) * 20}")"
  score=$((score + density))
  echo "$score"
}

# tf_complexity_compress_ratio <text> → compressibility ratio (0.0-1.0)
# Lower = more information-dense = more complex.
tf_complexity_compress_ratio() {
  local text="$1"
  local original_size compressed
  original_size="$(echo "$text" | wc -c)"
  compressed="$(echo "$text" | gzip -c 2>/dev/null | wc -c)"
  if [[ "$original_size" -gt 0 ]]; then
    LC_ALL=C awk "BEGIN{printf \"%.2f\", $compressed / $original_size}"
  else
    echo "1.0"
  fi
}
