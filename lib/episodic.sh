#!/usr/bin/env bash
# episodic.sh — episodic memory for task dispatch.
#
# Logs successful task completions as compact TSV episodes. On dispatch,
# recalls similar past episodes (by engine + scope-file overlap) and injects
# "past attempts" context into the worker prompt.
#
# Inspired by moe-sovereign's episodic_memory.py (Neo4j GraphRAG) but
# implemented as append-only TSV with grep-based recall — zero jq on the
# hot path.
#
# TSV format: engine\tscope_hash\tworker\ttier\twon\twall_clock_s\terror_category\ttimestamp
# Stored in $TF_STATE_DIR/episodes.tsv (cap: 500 entries, LRU eviction).

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

TF_EPISODES_FILE="${TF_EPISODES_FILE:-$TF_STATE_DIR/episodes.tsv}"
TF_EPISODES_MAX="${TF_EPISODES_MAX:-500}"

# ---------------------------------------------------------------------------
# Write: append an episode after task completion.
# ---------------------------------------------------------------------------

# tf_episode_record <task_id> <worker> <won> <wall_clock_s> <error_category>
# Called after gate verification. Reads task metadata from cache or jq.
tf_episode_record() {
  local id="$1" worker="$2" won="$3" wall_clock="${4:-0}" error_cat="${5:-}"
  local engine tier scope scope_hash ts
  # Read from cache if available, otherwise jq (off-hot-path, fine)
  if tf_cache_valid 2>/dev/null; then
    engine="$(tf_cache_task_field "$id" 2)"
    tier="$(tf_cache_task_field "$id" 3)"
    # Cache stores scope as comma-separated; normalise to newline-separated
    # so the scope_hash matches the non-cache (jq) path.
    scope="$(tf_cache_task_field "$id" 5 | tr ',' '\n')"
  else
    engine="$(tf_task_field "$id" .engine 2>/dev/null || echo "t")"
    tier="$(tf_task_field "$id" .model_tier 2>/dev/null || echo "standard")"
    scope="$(tf_task_field "$id" '.scope[]' 2>/dev/null | head -5)"
  fi
  [[ -z "$engine" ]] && engine="t"
  [[ -z "$tier" ]] && tier="standard"
  # scope_hash: compact hash of scope files for similarity matching
  scope_hash="$(echo "$scope" | md5sum | cut -c1-8)"
  ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  # Append (O(1))
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$engine" "$scope_hash" "$worker" "$tier" "$won" "$wall_clock" "$error_cat" "$ts" \
    >> "$TF_EPISODES_FILE"
  # Cap: keep last TF_EPISODES_MAX entries
  local count
  count="$(wc -l < "$TF_EPISODES_FILE" 2>/dev/null || echo 0)"
  if [[ "$count" -gt "$TF_EPISODES_MAX" ]]; then
    local tmp
    tmp="$(mktemp)"
    tail -n "$TF_EPISODES_MAX" "$TF_EPISODES_FILE" > "$tmp"
    mv "$tmp" "$TF_EPISODES_FILE"
  fi
}

# ---------------------------------------------------------------------------
# Read: recall similar episodes for a task.
# ---------------------------------------------------------------------------

# tf_episode_recall <task_id> → prints 1-3 matching episodes as text block
# Matches by engine (exact) and scope_hash (exact) or scope-file overlap.
tf_episode_recall() {
  local id="$1"
  [[ ! -f "$TF_EPISODES_FILE" ]] && return 0
  local engine scope scope_hash
  if tf_cache_valid 2>/dev/null; then
    engine="$(tf_cache_task_field "$id" 2)"
    # Cache stores scope as comma-separated; normalise to newline-separated
    # so the scope_hash matches tf_episode_record (which uses the same normalisation).
    scope="$(tf_cache_task_field "$id" 5 | tr ',' '\n')"
  else
    engine="$(tf_task_field "$id" .engine 2>/dev/null || echo "t")"
    scope="$(tf_task_field "$id" '.scope[]' 2>/dev/null | head -5)"
  fi
  [[ -z "$engine" ]] && engine="t"
  scope_hash="$(echo "$scope" | md5sum | cut -c1-8)"
  # Match by engine + scope_hash (exact), fall back to engine only.
  # grep is ~1ms for 500 lines — no jq.
  local matches
  matches="$(grep -P "^${engine}\t${scope_hash}\t" "$TF_EPISODES_FILE" 2>/dev/null | tail -3)"
  if [[ -z "$matches" ]]; then
    matches="$(grep -P "^${engine}\t" "$TF_EPISODES_FILE" 2>/dev/null | tail -3)"
  fi
  [[ -z "$matches" ]] && return 0
  # Format as a compact text block for prompt injection
  local line worker won wall_clock
  echo "## Past episodes (similar tasks):"
  while IFS=$'\t' read -r ep_engine ep_hash ep_worker ep_tier ep_won ep_wall ep_err ep_ts; do
    [[ -z "$ep_worker" ]] && continue
    if [[ "$ep_won" == "1" ]]; then
      echo "  - ✅ $ep_worker succeeded in ${ep_wall}s (engine: $ep_engine, tier: $ep_tier)"
    else
      echo "  - ❌ $ep_worker failed (engine: $ep_engine, error: ${ep_err:-unknown})"
    fi
  done <<< "$matches"
}

# ---------------------------------------------------------------------------
# Diagnostics
# ---------------------------------------------------------------------------

# tf_episode_recall_by_scope <scope_file> <engine> → matching episodes by scope
tf_episode_recall_by_scope() {
  local scope_file="$1" engine="${2:-}"
  [[ ! -f "$TF_EPISODES_FILE" ]] && return 0
  local scope_hash
  scope_hash="$(echo "$scope_file" | md5sum | cut -c1-8)"
  local matches
  if [[ -n "$engine" ]]; then
    matches="$(grep -P "^${engine}\t${scope_hash}\t" "$TF_EPISODES_FILE" 2>/dev/null | tail -3)"
  else
    matches="$(grep -P "\t${scope_hash}\t" "$TF_EPISODES_FILE" 2>/dev/null | tail -3)"
  fi
  [[ -z "$matches" ]] && return 0
  local line
  while IFS=$'\t' read -r ep_engine ep_hash ep_worker ep_tier ep_won ep_wall ep_err ep_ts; do
    [[ -z "$ep_worker" ]] && continue
    if [[ "$ep_won" == "1" ]]; then
      echo "  - ✅ $ep_worker succeeded (engine: $ep_engine, scope: $scope_file)"
    else
      echo "  - ❌ $ep_worker failed (engine: $ep_engine, error: ${ep_err:-unknown})"
    fi
  done <<< "$matches"
}

# tf_episode_recall_by_error <error_category> → matching episodes by error
tf_episode_recall_by_error() {
  local error_cat="$1"
  [[ ! -f "$TF_EPISODES_FILE" ]] && return 0
  local matches
  # Error category is field 7
  matches="$(awk -F'\t' -v cat="$error_cat" '$7==cat {print}' "$TF_EPISODES_FILE" 2>/dev/null | tail -3)"
  [[ -z "$matches" ]] && return 0
  local line
  while IFS=$'\t' read -r ep_engine ep_hash ep_worker ep_tier ep_won ep_wall ep_err ep_ts; do
    [[ -z "$ep_worker" ]] && continue
    if [[ "$ep_won" == "1" ]]; then
      echo "  - ✅ $ep_worker recovered from $error_cat (engine: $ep_engine)"
    else
      echo "  - ❌ $ep_worker failed with $error_cat (engine: $ep_engine)"
    fi
  done <<< "$matches"
}

# tf_episode_win_rate <worker> → win rate (0.000-1.000)
tf_episode_win_rate() {
  local worker="$1"
  [[ ! -f "$TF_EPISODES_FILE" ]] && { echo "0.500"; return; }
  local total wins
  total="$(awk -F'\t' -v w="$worker" '$3==w {count++} END{print count+0}' "$TF_EPISODES_FILE")"
  wins="$(awk -F'\t' -v w="$worker" '$3==w && $5==1 {count++} END{print count+0}' "$TF_EPISODES_FILE")"
  if [[ "$total" -eq 0 ]]; then
    echo "0.500"
  else
    LC_ALL=C awk "BEGIN{printf \"%.3f\", $wins/$total}"
  fi
}

# tf_episode_stats → print episode statistics
tf_episode_stats() {
  [[ ! -f "$TF_EPISODES_FILE" ]] && { echo "no episodes recorded"; return; }
  local total wins losses
  total="$(wc -l < "$TF_EPISODES_FILE")"
  wins="$(awk -F'\t' '$5==1 {count++} END{print count+0}' "$TF_EPISODES_FILE")"
  losses=$((total - wins))
  echo "Episodes: $total ($wins wins, $losses losses)"
  echo "By engine:"
  awk -F'\t' '{eng[$1]++; won[$1]+=$5} END{for (e in eng) printf "  %s: %d episodes, %d wins (%.0f%%)\n", e, eng[e], won[e], (eng[e]>0 ? won[e]/eng[e]*100 : 0)}' "$TF_EPISODES_FILE" | sort
}
