#!/usr/bin/env bash
# bayes.sh — Bayesian worker-quality + expected-cost model for cost/energy-efficient
# routing in taskfleet.
#
# Rationale (why this is "predictably worthwhile"):
#   Every dispatch spends energy and (for paid clouds) money. Picking the worker
#   that MINIMIZES the EXPECTED cost to reach a successful outcome is the
#   principled, low-risk choice: it never spends more than necessary, and it
#   degrades gracefully (with no history it falls back to neutral, so cost-aware
#   free-first behaviour is preserved).
#
# Model:
#   Outcome of each (worker, engine) pairing is Bernoulli(success). We keep a
#   Beta(α,β) posterior with a uniform Beta(1,1) prior:
#       α = wins + 1,   β = fails + 1
#   derived from the affinity ledger (past wins/total). From this we read:
#       P(success) mean        = α / (α + β)
#       P(success) lower bound  = conservative 5% credible bound
#   Expected cost-to-done (the routing score) treats task success as a geometric
#   process with absorbing failure:
#       E[cost] = cost_weight * (1 / P(success))
#   where cost_weight = 0 for free/local workers (loopback/private endpoint or an
#   explicit `.free` flag) and a small relative price for paid clouds. The
#   cheapest-sufficient worker wins. Because free workers have cost_weight ≈ 0,
#   they dominate unless a paid worker is dramatically more reliable — exactly the
#   "prefer free, escalate to paid only when justified by evidence" policy.
#
# This is the Bayesian counterpart to the UCB1 bandit in affinity.sh: UCB1
# optimizes EXPLORATION/EXPLOITATION of win-rate; this optimizes EXPECTED COST,
# folding price in. The dispatch-round loop itself is an absorbing Markov chain
# (ready→running→done|failed→retry|permanent-fail); the attempt cap
# (TF_MAX_ATTEMPTS) is its guaranteed-termination absorber.
#
# Depends on: common.sh (tf_worker_field), affinity.sh (tf_affinity_table, cache).
# Pure awk/jq — NO bc (which may be absent on minimal hosts).

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"
# shellcheck source=affinity.sh
. "$(dirname "${BASH_SOURCE[0]}")/affinity.sh"

# tf_worker_outcomes <worker> <engine> → "wins total" from the affinity ledger.
tf_worker_outcomes() {
  local worker="$1" engine="$2" row
  if tf_cache_valid 2>/dev/null && [[ -s "$TF_CACHE_DIR/affinity.tsv" ]]; then
    row="$(awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $3"\t"$4; exit}' "$TF_CACHE_DIR/affinity.tsv")"
  else
    row="$(tf_affinity_table 2>/dev/null | awk -F'\t' -v w="$worker" -v e="$engine" '$1==w && $2==e {print $3"\t"$4; exit}')"
  fi
  [[ -z "$row" ]] && { printf '0\t0\n'; return; }
  echo "$row"
}

# tf_worker_beta <worker> <engine> → "alpha beta" (Beta posterior, uniform prior)
tf_worker_beta() {
  local o wins total
  o="$(tf_worker_outcomes "$1" "$2")"
  wins="${o%%$'\t'*}"; total="${o##*$'\t'}"
  wins="${wins:-0}"; total="${total:-0}"
  echo "$((wins + 1)) $((total - wins + 1))"
}

# tf_worker_success_mean <worker> <engine> → mean P(success) in [0,1]
tf_worker_success_mean() {
  local b a
  b="$(tf_worker_beta "$1" "$2")"
  a="${b%% *}"; b="${b##* }"
  LC_ALL=C awk -v a="$a" -v b="$b" 'BEGIN{printf "%.4f", a/(a+b)}'
}

# tf_worker_success_lower <worker> <engine> → conservative lower credible bound.
# Closed-form Beta(α,β) 5% lower bound; falls back to α/(α+β) for α,β ≤ 1.
tf_worker_success_lower() {
  local b a
  b="$(tf_worker_beta "$1" "$2")"
  a="${b%% *}"; b="${b##* }"
  if [[ "$a" -gt 1 && "$b" -gt 1 ]]; then
    LC_ALL=C awk -v a="$a" -v b="$b" 'BEGIN{printf "%.4f", (a-1)/((a+b)-2)}'
  else
    LC_ALL=C awk -v a="$a" -v b="$b" 'BEGIN{printf "%.4f", a/(a+b)}'
  fi
}

# tf_worker_cost_weight <worker> → 0 for free/local, else relative USD/1M weight.
# Avoids bc entirely (uses a static price map). Free/local dominates routing.
tf_worker_cost_weight() {
  local ep prov model
  ep="$(tf_worker_field "$1" .endpoint 2>/dev/null || echo "")"
  [[ -z "$ep" ]] && ep="$(tf_worker_field "$1" .base_url 2>/dev/null || echo "")"
  # Free / self-hosted (loopback or private range) → zero marginal cost.
  [[ "$ep" =~ ^https?://(127\.|localhost|::1|10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.) ]] && { echo "0"; return; }
  prov="$(tf_worker_field "$1" .provider 2>/dev/null || echo unknown)"
  model="$(tf_worker_field "$1" .model 2>/dev/null || echo unknown)"
  # Relative weight ≈ USD per 1M tokens blended, normalized so ~$5/1M ≈ 1.0.
  case "$prov:$model" in
    *:gpt-4o)          echo "2.0" ;;
    *:gpt-4*)          echo "3.0" ;;
    *:claude-*opus*)   echo "3.0" ;;
    *:claude-sonnet*)  echo "1.2" ;;
    *:claude-*)        echo "1.5" ;;
    *:deepseek*)       echo "0.05" ;;
    *:mistral*)        echo "1.0" ;;
    *)                 echo "1.0" ;;
  esac
}

# tf_worker_expected_cost_to_done <worker> <engine> → expected relative cost.
# E[cost] = cost_weight * (1 / P_success). Lower is better. Free workers ≈ 0.
# Floor P(success) at 0.01 so an unknown worker is never ranked infinitely cheap
# (we still prefer known-good free workers via cost-aware restriction first).
tf_worker_expected_cost_to_done() {
  local p cw
  p="$(tf_worker_success_mean "$1" "$2")"
  cw="$(tf_worker_cost_weight "$1")"
  LC_ALL=C awk -v p="$p" -v cw="$cw" 'BEGIN{ if (p+0 <= 0) p=0.01; printf "%.4f", cw * (1/p) }' 2>/dev/null || echo "999"
}

# tf_task_success_prob_within <worker> <engine> <attempts_left> →
#   P(at least one success in remaining attempts) for a geometric success process
#   = 1 - (1 - P_success)^attempts_left. Used by the Markov early-stop guard to
#   avoid burning paid tokens on near-doomed tasks.
tf_task_success_prob_within() {
  local p
  p="$(tf_worker_success_mean "$1" "$2")"
  LC_ALL=C awk -v p="$p" -v r="$3" 'BEGIN{ if (r+0 < 0) r=0; printf "%.4f", 1 - (1-p)^r }' 2>/dev/null || echo "0"
}
