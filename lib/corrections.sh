#!/usr/bin/env bash
# corrections.sh — self-correction few-shot store.
#
# Inspired by moe-sovereign's self_correction.py (Redis-stored few-shot
# examples per error category). Implemented as append-only TSV files with
# grep-based recall — zero jq on the hot path.
#
# After a failed-then-succeeded task, the correction (what changed between
# the failed and successful attempt) is stored. On retry, relevant past
# corrections are injected into the prompt.
#
# TSV format: timestamp\tengine\terror_category\tcorrection\ttask_id
# Stored in $TF_STATE_DIR/corrections/{category}.tsv (cap: 10 per category).

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

TF_CORRECTIONS_DIR="${TF_STATE_DIR}/corrections}"
mkdir -p "$TF_CORRECTIONS_DIR"

# tf_corrections_record <error_category> <engine> <correction> <task_id>
# Called after a successful retry. Appends to the category-specific TSV.
tf_corrections_record() {
  local category="$1" engine="$2" correction="$3" task_id="${4:-}"
  local file="$TF_CORRECTIONS_DIR/${category}.tsv"
  local ts
  ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  # Sanitize: replace tabs/newlines in correction
  correction="$(echo "$correction" | tr '\t\n' '  ')"
  printf '%s\t%s\t%s\t%s\t%s\n' "$ts" "$engine" "$category" "$correction" "$task_id" >> "$file"
  # Cap at 10 entries per category (LRU)
  local count
  count="$(wc -l < "$file" 2>/dev/null || echo 0)"
  if [[ "$count" -gt 10 ]]; then
    local tmp
    tmp="$(mktemp)"
    tail -n 10 "$file" > "$tmp"
    mv "$tmp" "$file"
  fi
}

# tf_corrections_recall <error_category> <engine> → prints 1-3 matching corrections
# grep-based recall (~1ms for 10-line files). No jq.
tf_corrections_recall() {
  local category="$1" engine="${2:-}"
  local file="$TF_CORRECTIONS_DIR/${category}.tsv"
  [[ ! -f "$file" ]] && return 0
  local matches
  if [[ -n "$engine" ]]; then
    # Match by engine first, fall back to any engine
    matches="$(grep -P "^\S+\t${engine}\t" "$file" 2>/dev/null | tail -3)"
    [[ -z "$matches" ]] && matches="$(tail -3 "$file")"
  else
    matches="$(tail -3 "$file")"
  fi
  [[ -z "$matches" ]] && return 0
  local ts eng cat correction tid
  while IFS=$'\t' read -r ts eng cat correction tid; do
    [[ -z "$correction" ]] && continue
    echo "  - $correction"
  done <<< "$matches"
}

# tf_corrections_extract <task_id> <worktree_path> <error_category>
# Extracts a correction snippet from the diff between the failed attempt's
# error and the successful gate. Called after a successful retry.
tf_corrections_extract() {
  local id="$1" wt="$2" category="$3"
  local correction=""
  case "$category" in
    compile_error)
      # Extract the first fixed file from the diff
      correction="$(cd "$wt" && git diff --name-only "${TF_BASE_BRANCH}"...HEAD 2>/dev/null | head -3 | tr '\n' ' ')"
      correction="Fixed compile error in: $correction"
      ;;
    test_failure)
      correction="$(cd "$wt" && git diff --stat "${TF_BASE_BRANCH}"...HEAD 2>/dev/null | tail -1)"
      correction="Fixed test failure: $correction"
      ;;
    no_op)
      correction="Produced required changes after explicit scope instruction"
      ;;
    merge_conflict)
      correction="Resolved merge conflict by rebasing onto latest main"
      ;;
    timeout)
      correction="Completed within timeout by focusing on primary scope file"
      ;;
    *)
      correction="Resolved $category"
      ;;
  esac
  echo "$correction"
}
