#!/usr/bin/env bash
# unit/test-affinity.sh — unit tests for lib/affinity.sh (worker-task routing)
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$HERE/../test-harness.sh"

SBOX="$(mktemp -d)"
trap 'rm -rf "$SBOX"' EXIT
export TF_CONFIG_DIR="$SBOX/config"
export TF_STATE_DIR="$SBOX/state"
export TF_LOG_DIR="$SBOX/logs"
export TF_REPO_DIR="$SBOX/repo"
export TF_RECEIPT_DIR="$SBOX/receipts"
mkdir -p "$TF_CONFIG_DIR" "$TF_STATE_DIR" "$TF_LOG_DIR" "$TF_RECEIPT_DIR"

cat > "$TF_CONFIG_DIR/workers.json" <<'JSON'
{"workers":[
  {"name":"git-bot","provider":"openai","model":"gpt-4o","enabled":true},
  {"name":"claude-sonnet","provider":"anthropic","model":"claude-sonnet-4-20250514","enabled":true}
],
 "defaults":{"max_attempts":3,"retry_cooldown_s":1,"accept_timeout_s":5}}
JSON

. "$HERE/../../lib/common.sh"
. "$HERE/../../lib/status.sh"
. "$HERE/../../lib/receipt.sh"
. "$HERE/../../lib/affinity.sh"

# Tasks: A1/A2 are probe rust tasks; R1..R20 are outcome-history rust tasks;
# P1..P5 are outcome-history python tasks.
cat > "$TF_CONFIG_DIR/tasks.json" <<'JSON'
{"tasks":[
  {"id":"A1","engine":"rust","title":"rust probe","section":"s","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
  {"id":"A2","engine":"rust","title":"rust probe 2","section":"s","deps":[],"scope":["a.rs"],"accept":"true","manual":false},
JSON
for i in $(seq 1 20); do
  printf '  {"id":"R%s","engine":"rust","title":"rust %s","section":"s","deps":[],"scope":["a.rs"],"accept":"true","manual":false},\n' "$i" "$i"
done >> "$TF_CONFIG_DIR/tasks.json"
for i in $(seq 1 5); do
  printf '  {"id":"P%s","engine":"python","title":"py %s","section":"s","deps":[],"scope":["b.py"],"accept":"true","manual":false}%s\n' "$i" "$i" "$([ "$i" -eq 5 ] && echo "" || echo ",")"
done >> "$TF_CONFIG_DIR/tasks.json"
printf ']}\n' >> "$TF_CONFIG_DIR/tasks.json"

# assert tasks.json is valid JSON
jq empty "$TF_CONFIG_DIR/tasks.json" || { echo "invalid tasks.json"; exit 1; }

tf_reset() {
  STATUS_JSON="$SBOX/state/reset-$$.json"
  rm -f "$TF_RECEIPT_DIR"/*.ndjson
  tf_status_init
}

# Helper: write a begin record + closed record for a task (one completed attempt)
# tf_write_outcome <task_id> <worker> <final_status>
tf_write_outcome() {
  local id="$1" worker="$2" final="$3"
  local rfile="$TF_RECEIPT_DIR/2025-01-15.ndjson"
  jq -nc --arg id "$id" --arg w "$worker" \
    '{task_id:$id, worker:$w, status:"running"}' >> "$rfile"
  jq -nc --arg id "$id" --arg final "$final" \
    '{type:"closed", task_id:$id, final_status:$final}' >> "$rfile"
}

echo "=== [unit] affinity.sh ==="

# ---- engine inferred from tasks.json ----
tf_group_begin; tf_test "tf_task_field reads engine from tasks.json"
tf_reset
tf_assert_eq "A1 engine is rust" "rust" "$(tf_task_field A1 .engine)"
tf_group_end

# ---- affinity table aggregates win rates ----
tf_group_begin; tf_test "tf_affinity_table aggregates per-worker per-engine outcomes"
tf_reset
# git-bot: 8/10 rust wins (R1..R10)
for i in $(seq 1 10); do
  if [[ $i -le 8 ]]; then tf_write_outcome "R$i" git-bot done; else tf_write_outcome "R$i" git-bot failed; fi
done
# claude: 2/10 rust wins (R11..R20)
for i in $(seq 11 20); do
  if [[ $i -le 12 ]]; then tf_write_outcome "R$i" claude-sonnet done; else tf_write_outcome "R$i" claude-sonnet failed; fi
done
# claude: 5/5 python wins (P1..P5)
for i in $(seq 1 5); do tf_write_outcome "P$i" claude-sonnet done; done

git_rust="$(tf_affinity_table | awk -F'\t' '$1=="git-bot" && $2=="rust" {print $3"/"$4}')"
claude_rust="$(tf_affinity_table | awk -F'\t' '$1=="claude-sonnet" && $2=="rust" {print $3"/"$4}')"
claude_py="$(tf_affinity_table | awk -F'\t' '$1=="claude-sonnet" && $2=="python" {print $3"/"$4}')"
tf_assert_eq "git-bot rust win rate 8/10" "8/10" "$git_rust"
tf_assert_eq "claude rust win rate 2/10" "2/10" "$claude_rust"
tf_assert_eq "claude python win rate 5/5" "5/5" "$claude_py"
tf_group_end

# ---- affinity score ----
tf_group_begin; tf_test "tf_affinity_score returns correct win rates"
tf_reset
for i in $(seq 1 10); do
  if [[ $i -le 8 ]]; then tf_write_outcome "R$i" git-bot done; else tf_write_outcome "R$i" git-bot failed; fi
done
for i in $(seq 11 20); do
  if [[ $i -le 12 ]]; then tf_write_outcome "R$i" claude-sonnet done; else tf_write_outcome "R$i" claude-sonnet failed; fi
done
score_git="$(tf_affinity_score git-bot A1)"
score_claude="$(tf_affinity_score claude-sonnet A1)"
tf_assert "git-bot score ~0.8" echo "$score_git" | grep -q '^0\.8'
tf_assert "claude score ~0.2" echo "$score_claude" | grep -q '^0\.2'
tf_group_end

# ---- best worker routing prefers high-affinity ----
tf_group_begin; tf_test "tf_best_worker_for picks high-affinity rust worker"
tf_reset
for i in $(seq 1 10); do
  if [[ $i -le 8 ]]; then tf_write_outcome "R$i" git-bot done; else tf_write_outcome "R$i" git-bot failed; fi
done
for i in $(seq 11 20); do
  if [[ $i -le 12 ]]; then tf_write_outcome "R$i" claude-sonnet done; else tf_write_outcome "R$i" claude-sonnet failed; fi
done
best="$(tf_best_worker_for A1 git-bot claude-sonnet)"
tf_assert_eq "git-bot chosen for rust" "git-bot" "$best"
tf_group_end

# ---- no history falls back to neutral / config order ----
tf_group_begin; tf_test "tf_best_worker_for falls back to config order with no history"
tf_reset
best="$(tf_best_worker_for A1 git-bot claude-sonnet)"
tf_assert_eq "first candidate chosen (config order)" "git-bot" "$best"
tf_assert_eq "neutral score with no receipts" "0.500" "$(tf_affinity_score git-bot A1)"
tf_group_end

# ---- python task routed to claude (engine-specific affinity) ----
tf_group_begin; tf_test "python task routes to claude despite config order"
tf_reset
for i in $(seq 1 10); do
  if [[ $i -le 8 ]]; then tf_write_outcome "R$i" git-bot done; else tf_write_outcome "R$i" git-bot failed; fi
done
for i in $(seq 11 20); do
  if [[ $i -le 12 ]]; then tf_write_outcome "R$i" claude-sonnet done; else tf_write_outcome "R$i" claude-sonnet failed; fi
done
for i in $(seq 1 5); do tf_write_outcome "P$i" claude-sonnet done; done
# P1 is python; claude is 5/5 on python, git-bot has no python history (falls
# back to overall rust-heavy 0.8) — but engine-specific 1.0 beats 0.8 fallback.
best="$(tf_best_worker_for P1 git-bot claude-sonnet)"
tf_assert_eq "claude chosen for python" "claude-sonnet" "$best"
tf_group_end

# ---- rank ordering best-first ----
tf_group_begin; tf_test "tf_affinity_rank orders best-first"
tf_reset
for i in $(seq 1 10); do
  if [[ $i -le 9 ]]; then tf_write_outcome "R$i" git-bot done; else tf_write_outcome "R$i" git-bot failed; fi
done
for i in $(seq 11 20); do
  if [[ $i -le 4 ]]; then tf_write_outcome "R$i" claude-sonnet done; else tf_write_outcome "R$i" claude-sonnet failed; fi
done
ranked="$(tf_affinity_rank A1 claude-sonnet git-bot)"
tf_assert_eq "git-bot first" "git-bot" "$(echo "$ranked" | head -1)"
tf_assert_eq "claude second" "claude-sonnet" "$(echo "$ranked" | sed -n 2p)"
tf_group_end

tf_test_summary
