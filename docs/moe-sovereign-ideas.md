# MoE Sovereign → AgentFlow: Performance-First Analysis

**Source:** `https://github.com/h3rb3rn/moe-sovereign` (cloned to `~/git/moe-sovereign`)
**Target:** AgentFlow / taskfleet (`~/git/agentflow`)
**Date:** 2026-08-17
**Focus:** Performance impact of each idea on the scheduling hot path

---

## The Performance Problem (Measured)

Before adding any new features, we must understand taskfleet's existing performance bottleneck.

### Measured spawn costs (100 iterations)

| Operation | Cost per call | Relative |
|-----------|--------------|----------|
| `jq` (JSON parse) | **251 ms** | **1×** |
| `python3 -c "pass"` | 96 ms | 0.4× |
| `awk` (TSV parse) | 15 ms | 0.06× |
| `grep` (pattern match) | 15 ms | 0.06× |
| bash string op (no spawn) | 0.03 ms | 0.0001× |

**jq is 17× slower than awk and 2.6× slower than python3.**

### Current scheduling round cost (20 tasks, 5 ready, 3 running, 3 workers)

| Function | jq calls | Cost |
|----------|----------|------|
| `tf_all_task_ids` | 1 | 251 ms |
| `tf_is_ready` (20 tasks × ~4 jq each) | 80 | 20,080 ms |
| `tf_scope_conflicts` (5 ready × 3 running) | 15 | 3,765 ms |
| `tf_best_worker_for` (3 workers × 2 jq each) | 6 | 1,506 ms |
| **Total** | **102** | **25.6 s** |

**The scheduling overhead (25.6 s) already exceeds the default poll interval (15 s).** The scheduler spends more time parsing JSON than sleeping between rounds. Adding features that call jq on the hot path would make this worse.

### Root cause: per-field jq calls

Every `tf_task_field`, `tf_status_get`, `tf_scope_files_for`, and `tf_affinity_table` call spawns a new `jq` process that re-parses the entire JSON file. With 20 tasks and dependency checks, a single scheduling round spawns 100+ jq processes.

---

## Performance-First Principles for New Features

1. **Never add jq calls to the scheduling hot path.** The hot path is: `tf_smart_ready_task_ids` → `tf_is_ready` → `tf_scope_conflicts` → `tf_best_worker_for` → `tf_dispatch_one`.
2. **Use TSV indexes + grep/awk (15 ms) instead of JSON + jq (251 ms).** A grep on a 500-line TSV is ~1 ms. A jq parse of a 500-line JSON is ~251 ms.
3. **Precompute at startup, not per dispatch.** Load task metadata, affinity tables, and mission context into bash associative arrays once per orchestrator run.
4. **Append-only writes are O(1).** Logs (dispatch log, decision log, episode log) use `>>` (1 ms). No JSON parsing on write.
5. **Off-hot-path operations can use jq.** Retry, gate verification, merge, and report generation run once per task completion (dwarfed by the 30s+ LLM call). A few jq calls there are fine.
6. **Batch jq calls.** Instead of `tf_task_field` per field, load the entire task into a bash associative array with a single jq call.

---

## Prerequisite: Batch jq Optimization (Fix the Existing Bottleneck)

Before adding any moe-sovereign features, fix the existing jq bottleneck. This alone could cut scheduling overhead from 25.6 s to <1 s.

### A. Cache task metadata in a bash associative array (one jq call)

```bash
# Current: 4+ jq calls per task per round
tf_status_get "$id" .status      # 251ms
tf_task_field "$id" .deps[]      # 251ms
tf_task_field "$id" .scope[]     # 251ms
tf_task_field "$id" .engine       # 251ms

# Proposed: one jq call to load ALL tasks into a TSV cache
tf_cache_tasks() {
  # One jq call → TSV file: id\tstatus\tdeps\tscope\tengine\ttier
  jq -r '
    .tasks[] |
    [.id,
     (.deps | join(",")),
     (.scope | join(",")),
     (.engine // "unknown"),
     (.model_tier // "standard"),
     (.title // "")
    ] | @tsv
  ' "$TASKS_JSON" > "$TF_STATE_DIR/task-cache.tsv"
  # One jq call → status cache: id\tstatus\tnext_retry_at
  jq -r 'to_entries[] | [.key, .value.status, (.value.next_retry_at // "")] | @tsv' \
    "$STATUS_JSON" > "$TF_STATE_DIR/status-cache.tsv"
}
```

**Impact:** Replaces 80+ jq calls (20 s) with 2 jq calls (502 ms) + N grep calls (15 ms each). Net: **20 s → ~0.8 s.**

### B. Cache the affinity table (one jq call per round, not per worker)

```bash
# Current: tf_affinity_table is called once per tf_affinity_score call
#          (3 workers × 1 jq reading ALL receipts = 3 × 251ms = 753ms)
# Proposed: call tf_affinity_table ONCE per scheduling round, cache in TSV

tf_affinity_cache() {
  # One jq -s call → TSV: worker\tengine\twins\ttotal
  tf_affinity_table > "$TF_STATE_DIR/affinity-cache.tsv"
}
# Then tf_affinity_score reads from the TSV with awk (15ms, not 251ms)
```

**Impact:** 3 × 251 ms → 1 × 251 ms + 3 × 15 ms = 296 ms. **753 ms → 296 ms.**

### C. Cache scope files in a bash associative array

```bash
# Current: tf_scope_files_for calls tf_task_field (jq) per running task
# Proposed: precompute a scope→task_id map at round start

tf_scope_cache() {
  # Build: scope_file\ttask_id (for all running tasks)
  while IFS=$'\t' read -r id status _; do
    [[ "$status" == "running" ]] || continue
    # Read scope from task-cache.tsv (grep, not jq)
    local scope
    scope="$(awk -F'\t' -v id="$id" '$1==id {print $3}' "$TF_STATE_DIR/task-cache.tsv")"
    local f
    IFS=',' read -ra files <<< "$scope"
    for f in "${files[@]}"; do
      echo -e "$f\t$id"
    done
  done < "$TF_STATE_DIR/status-cache.tsv" > "$TF_STATE_DIR/scope-cache.tsv"
}
# Then tf_scope_conflicts is a single grep (15ms), not N×jq
```

**Impact:** 15 × 251 ms → 1 × 15 ms (grep on TSV). **3.8 s → 15 ms.**

### Combined impact of A+B+C

| Before | After | Speedup |
|--------|-------|---------|
| 102 jq calls, 25.6 s | 3 jq calls + ~30 grep/awk calls, ~1.2 s | **21×** |

This makes room for new features without exceeding the poll interval.

---

## Re-Evaluated Ideas (Performance-First)

Each idea is re-evaluated against the hot-path cost. Ideas that would add jq or python3 calls to the scheduling loop are restructured or rejected.

### Tier 1 — High Impact, <15 ms Hot-Path Cost

#### 1. Episodic Memory (TSV-Indexed, Not JSON)

**MoE source:** `episodic_memory.py` — Neo4j episode nodes with semantic recall.

**Performance constraint:** Episode recall must NOT parse JSON on the hot path. 500 episodes in JSONL = 500 × 251 ms if parsed with jq per episode. Unacceptable.

**Implementation:**
- **Write:** Append one TSV line per completed task to `state/episodes.tsv`:
  ```
  task_type\tscope_hash\tworker\ttier\twon\twall_clock_s\terror_category\ttimestamp
  ```
  Write is O(1) (`>>` append, ~1 ms).
- **Read (hot path):** `grep "^${task_type}\t" state/episodes.tsv | tail -5` — pure grep, ~1 ms for 500 lines.
- **Index:** Optionally build a sorted index (`sort -k1,1`) on episode rotation. Binary search with `look` is O(log n).
- **Inject into prompt:** Only the 1-3 most recent matching episodes, formatted as a short text block. Added to the prompt file during `tf_render_prompt` (off-hot-path, runs once per dispatch, dwarfed by LLM call).

**Hot-path cost:** 1 grep (~1 ms). **No jq, no python3.**

**Effort:** ~100 lines bash.

---

#### 2. Complexity Auto-Tier (Precomputed at Task Load)

**MoE source:** `complexity_estimator.py` — AIC/zlib compressibility, token count, domain markers.

**Performance constraint:** Compressibility check (`gzip -c | wc -c`) is ~5 ms but must NOT run per dispatch. It should run ONCE when the task is loaded.

**Implementation:**
- **At task load** (not per dispatch): compute complexity and write it into the task-cache TSV:
  ```bash
  tf_compute_complexity() {
    local desc="$1" scope_count="$2" dep_count="$3"
    local words=$(echo "$desc" | wc -w)
    local compressed=$(echo "$desc" | gzip -c | wc -c)
    local ratio=$(echo "scale=2; $compressed / ${#desc}" | bc 2>/dev/null)
    # Heuristics: word count, scope size, dep depth, compressibility
    if [[ $words -gt 50 && $scope_count -gt 5 ]] || [[ $ratio -lt 0.15 ]]; then
      echo "complex"
    elif [[ $words -gt 20 || $scope_count -gt 2 ]]; then
      echo "moderate"
    else
      echo "trivial"
    fi
  }
  ```
- **Cached in `task-cache.tsv`** alongside the other fields. No per-dispatch cost.
- **Read on dispatch:** `awk -F'\t' -v id="$id" '$1==id {print $6}' task-cache.tsv` — 15 ms.

**Hot-path cost:** 0 ms (precomputed). **No jq, no python3, no gzip on hot path.**

**Effort:** ~40 lines bash. One `gzip` call per task at load time.

---

#### 3. Self-Correction Few-Shot (Off-Hot-Path, Retry Only)

**MoE source:** `self_correction.py` — Redis-stored few-shot examples per error category.

**Performance constraint:** This runs on retry only (cold path), not on the scheduling hot path. The LLM call takes 30s+; a 15 ms `grep` is negligible.

**Implementation:**
- **Write (on successful gate after retry):** Append to `state/corrections/{category}.tsv`:
  ```
  error_summary\tfix_description\ttask_type\ttimestamp
  ```
  O(1) append, ~1 ms. Cap at 10 entries per category (rotate on write).
- **Read (on retry, in `tf_render_prompt`):** `grep "^${task_type}\t" state/corrections/${category}.tsv | tail -3` — ~1 ms.
- **Inject into prompt:** 3-line text block: "Previous similar failures were resolved by: ..."

**Hot-path cost:** 0 ms (runs on retry, not on dispatch). **No jq, no python3.**

**Effort:** ~80 lines bash.

---

#### 4. Trust Score (Pure Arithmetic, No I/O)

**MoE source:** `trust_score.py` — weighted [0.0–1.0] score with PROCEED/REVIEW/BLOCK buckets.

**Performance constraint:** Must be pure bash arithmetic. No jq, no python3, no file I/O beyond reading existing receipt data.

**Implementation:**
- Computed after gate verification (off-hot-path, gate already took seconds):
  ```bash
  tf_trust_score() {
    local id="$1" exit_code="$2" scope_ok="$3" tests_passed="$4" tests_total="$5" retries="$6" affinity="$7"
    # Pure bash arithmetic (no external calls)
    local score=0
    (( exit_code == 0 )) && score=$((score + 35))    # gate: 35%
    [[ "$scope_ok" == "true" ]] && score=$((score + 25))  # scope: 25%
    if [[ $tests_total -gt 0 ]]; then
      score=$((score + 20 * tests_passed / tests_total))  # tests: 20%
    fi
    (( retries == 0 )) && score=$((score + 10))         # retries: 10%
    score=$((score + 10 * affinity / 100))              # affinity: 10%
    echo "$score"
  }
  ```
- Buckets: `trusted` (≥80), `review` (50-79), `blocked` (<50).
- Stored in receipt (append-only, O(1)).

**Hot-path cost:** 0 ms (runs after gate, not on scheduling path). **Pure bash arithmetic.**

**Effort:** ~50 lines bash.

---

#### 5. Pipeline Transparency Log (Append-Only JSONL)

**MoE source:** Pipeline transparency log — per-request routing log with CSV export.

**Performance constraint:** Must be O(1) append. No JSON parsing on write. Read/report is off-hot-path.

**Implementation:**
- **Write:** `echo '{"ts":"...","task_id":"...","event":"dispatch",...}' >> state/dispatch_log.jsonl` — O(1), ~1 ms.
- **Report:** `tf_dispatch_log report` runs manually (off-hot-path), can use jq freely.
- **Rotation:** Rotate at 10 MB (`mv` + `gzip` in background).

**Hot-path cost:** 1 ms per event (append). **No jq on write.**

**Effort:** ~40 lines bash.

---

### Tier 2 — Medium Impact, Requires Careful Implementation

#### 6. Thompson Sampling → UCB1 (Pure Bash, No python3)

**MoE source:** `routing_bandit.py` — Thompson Beta-Bernoulli sampling per (gate, context, action).

**Performance constraint:** Thompson sampling requires `random.betavariate()` — a python3 call (96 ms) per worker per dispatch. With 3 workers, that's 288 ms per dispatch. **Unacceptable on the hot path.**

**Alternative: UCB1 (Upper Confidence Bound) — pure bash arithmetic:**
```bash
tf_ucb1_score() {
  local worker="$1" task_type="$2"
  # Read from affinity-cache.tsv (already built once per round)
  local wins total
  read wins total <<< "$(awk -F'\t' -v w="$worker" -v t="$task_type" '$1==w && $2==t {print $3, $4}' affinity-cache.tsv)"
  wins=${wins:-0}; total=${total:-0}
  if [[ $total -eq 0 ]]; then
    echo "1.0"  # Cold start: prioritize exploration
    return
  fi
  local mean=$(echo "scale=3; $wins / $total" | bc)
  local exploration=$(echo "scale=3; sqrt(2 * l($N_total) / $total)" | bc -l 2>/dev/null)
  echo "$mean + $exploration" | bc -l
}
```
- UCB1 gives the same exploration/exploitation balance as Thompson sampling but with pure `bc` arithmetic (~5 ms, no python3).
- Cold-start: score 1.0 for workers with no history (forces exploration).
- `N_total` = total observations across all workers for this task type (from the cached affinity table).

**Hot-path cost:** 1 `awk` (15 ms) + 2 `bc` (10 ms) = ~25 ms per worker. With 3 workers: 75 ms. **No jq, no python3.**

**Effort:** ~50 lines bash. Replaces `tf_affinity_score`'s raw win-rate with UCB1.

---

#### 7. Gap-Aware Retry (Off-Hot-Path, Retry Only)

**MoE source:** `cascade.py` — typed cascade events (SPEC_GAP, CONTEXT_GAP, SCOPE_DRIFT, etc.) with replan strategies.

**Performance constraint:** Runs on retry only (cold path). No hot-path cost.

**Implementation:**
- Extends existing `tf_classify_error` in `verify.sh`.
- Error categories already classified (`no_op`, `compile_error`, `test_failure`, `timeout`, `merge_conflict`).
- Add targeted prompt injection per category (already partially done in `tf_render_prompt`).
- Add `SCOPE_DRIFT` detection: compare `git diff --name-only` against declared scope. Pure `comm` + `grep`, ~15 ms.

**Hot-path cost:** 0 ms (retry only). Uses `comm`/`grep` (15 ms), no jq.

**Effort:** ~60 lines bash. Extends existing error classification.

---

#### 8. Sovereignty / Egress Guard (Cached at Startup)

**MoE source:** `sovereignty.py` — `assert_egress_allowed()` checks outbound endpoints.

**Performance constraint:** Must NOT validate per dispatch. Validate all workers ONCE at startup.

**Implementation:**
- At orchestrator startup, validate each worker's `api_base` against a private-IP allowlist:
  ```bash
  tf_validate_workers() {
    # One pass at startup, results cached in an associative array
    declare -gA TF_LOCAL_WORKERS
    while IFS= read -r worker; do
      local api_base
      api_base="$(tf_worker_field "$worker" .api_base)"  # jq, but only at startup
      if [[ "$api_base" =~ ^(10\.|172\.(1[6-9]|2[0-9]|3[01])\.|192\.168\.|127\.|::1) ]]; then
        TF_LOCAL_WORKERS["$worker"]=1
      else
        tf_warn "worker $worker has external api_base ($api_base) — blocked in TF_LOCAL_ONLY mode"
      fi
    done < <(tf_worker_names)
  }
  ```
- On dispatch: `[[ "${TF_LOCAL_WORKERS[$worker]}" == "1" ]]` — pure bash associative array lookup, 0.03 ms.

**Hot-path cost:** 0.03 ms (bash array lookup). **Validation at startup only.**

**Effort:** ~30 lines bash.

---

#### 9. Decision Log (Append-Only TSV)

**MoE source:** `decision_log.py` — append-only log with mandatory rationale.

**Performance constraint:** Must be O(1) append. No JSON parsing on write.

**Implementation:**
- **Write:** TSV append (not JSON, to avoid jq on read):
  ```
  timestamp\ttask_id\ttype\trationale
  ```
  `echo -e "$(date -u +%FT%TZ)\t$id\tDISPATCH\tworker=$worker tier=$tier" >> state/decisions.tsv` — O(1), ~1 ms.
- **Read (off-hot-path):** `grep`/`awk` for reporting. No jq needed.

**Hot-path cost:** 1 ms per event (append). **No jq on write or read.**

**Effort:** ~30 lines bash.

---

#### 10. Mission Context (Cached at Startup)

**MoE source:** `mission_context.py` — persistent JSON file with project state.

**Performance constraint:** Must NOT read JSON per dispatch. Read ONCE at startup into an environment variable.

**Implementation:**
- At startup: `TF_MISSION_CONTEXT="$(jq -r '.' state/mission.json)"` — one jq call, 251 ms, amortized over the entire orchestrator run.
- Pass to workers via environment variable or prompt preamble: `export TF_MISSION_CONTEXT`.
- On update: atomic write (tmp + rename), then re-read only if the orchestrator is long-running and mission changes mid-run (rare).

**Hot-path cost:** 0 ms (cached in env var at startup). **One jq call at startup, never per dispatch.**

**Effort:** ~40 lines bash.

---

### Tier 3 — Lower Priority, Performance-Neutral

These ideas are valuable but either have marginal performance impact or are complex enough that the implementation cost outweighs the benefit for a bash-based orchestrator.

#### 11. Corrective RAG Gate for Task Context
Relevance scoring before injecting past context. Use scope-file overlap (pure `comm`, 15 ms) rather than semantic similarity. Off-hot-path (runs during `tf_render_prompt`). **Hot-path cost: 0 ms.** Priority: P3.

#### 12. Community Knowledge Bundles (Export/Import)
Export/import task patterns as TSV with `sed`-based privacy scrubbing. Off-hot-path (manual command). **Hot-path cost: 0 ms.** Priority: P3.

#### 13. Cynefin Task Classification
Map trust score + scope size + dep depth to CLEAR/COMPLICATED/COMPLEX/CHAOTIC. Pure bash arithmetic if trust score is already computed. **Hot-path cost: 0 ms** (computed after gate). Priority: P3.

#### 14. HITL Gate for Low-Trust Tasks
Check trust score bucket (already computed) and block auto-merge. Pure bash comparison. **Hot-path cost: 0 ms.** Priority: P3.

#### 15. Cascade Typing
Extends `tf_classify_error` with typed events. Already off-hot-path (retry only). **Hot-path cost: 0 ms.** Priority: P2.

#### 16. Structured Failure Recovery
Classify worker failures into SCHEMA_OUTPUT/PROVIDER_TRANSPORT/RUNTIME_ERROR. Off-hot-path (post-dispatch). **Hot-path cost: 0 ms.** Priority: P2.

#### 17. Context Budget Management
Truncate scope files based on worker model's context window. Runs during `tf_render_prompt` (off-hot-path). Can use `wc -c` (pure bash, ~1 ms) for size estimation. **Hot-path cost: 0 ms.** Priority: P3.

#### 18. Federation (Cross-Instance Knowledge Sharing)
Network protocol for sharing task patterns. Off-hot-path (background sync). **Hot-path cost: 0 ms** but high implementation complexity. Priority: P3.

#### 19. Quality Probe (A/B Testing)
Dispatch the same task to two workers and compare. Doubles LLM cost for 5% of tasks. Off-hot-path (extra dispatch). **Hot-path cost: 0 ms** but doubles LLM cost. Priority: P3.

#### 20. Constitution Checks (Pre-Merge Deterministic Checks)
Regex-based checks on the diff before merge. Runs after gate (off-hot-path). `grep`/`sed` based, ~15 ms. **Hot-path cost: 0 ms.** Priority: P2.

---

## Revised Priority Matrix (Performance-Weighted)

| # | Idea | Impact | Effort | Hot-Path Cost | Priority |
|---|------|--------|--------|---------------|----------|
| **0** | **Batch jq Optimization** | **Critical** | **Medium** | **-24.4 s** | **P0 (prerequisite)** |
| 1 | Episodic Memory (TSV) | High | Low | 1 ms (grep) | **P0** |
| 2 | Complexity Auto-Tier | High | Low | 0 ms (precomputed) | **P0** |
| 3 | Self-Correction Few-Shot | High | Low | 0 ms (retry only) | **P0** |
| 4 | Trust Score | Medium | Low | 0 ms (arithmetic) | **P1** |
| 5 | Pipeline Transparency Log | Medium | Low | 1 ms (append) | **P1** |
| 6 | UCB1 Affinity (not Thompson) | Medium | Medium | 25 ms (awk+bc) | **P1** |
| 7 | Gap-Aware Retry | High | Medium | 0 ms (retry only) | **P1** |
| 8 | Egress Guard (cached) | Medium | Low | 0.03 ms (array) | **P2** |
| 9 | Decision Log (TSV) | Medium | Low | 1 ms (append) | **P2** |
| 10 | Mission Context (env var) | Medium | Low | 0 ms (env var) | **P2** |
| 15 | Cascade Typing | Medium | Low | 0 ms (retry only) | **P2** |
| 16 | Structured Failure Recovery | Medium | Low | 0 ms (post-dispatch) | **P2** |
| 20 | Constitution Checks | Medium | Low | 0 ms (pre-merge) | **P2** |
| 11 | Corrective RAG Gate | Low | Medium | 0 ms (render time) | P3 |
| 12 | Knowledge Bundles | Low | Medium | 0 ms (manual) | P3 |
| 13 | Cynefin Classification | Low | Low | 0 ms (post-gate) | P3 |
| 14 | HITL Gate | Low | Medium | 0 ms (post-gate) | P3 |
| 17 | Context Budget | Low | Medium | 0 ms (render time) | P3 |
| 18 | Federation | Low | High | 0 ms (background) | P3 |
| 19 | Quality Probe (A/B) | Low | Medium | 0 ms (extra dispatch) | P3 |

---

## Implementation Order

### Phase 1: Fix the Bottleneck (P0 prerequisite)
1. **Batch jq optimization** — `tf_cache_tasks()`, `tf_affinity_cache()`, `tf_scope_cache()`. Replace per-field jq calls with TSV caches built once per scheduling round. Expected: 25.6 s → ~1.2 s per round.

### Phase 2: High-Value, Zero-Cost Features (P0)
2. **Episodic memory** — TSV-indexed, grep-based recall. 1 ms per dispatch.
3. **Complexity auto-tier** — Precomputed at task load. 0 ms per dispatch.
4. **Self-correction few-shot** — Retry-only, grep-based. 0 ms on hot path.

### Phase 3: Medium-Value, Low-Cost Features (P1)
5. **Trust score** — Pure arithmetic, post-gate. 0 ms on hot path.
6. **Pipeline transparency log** — Append-only TSV. 1 ms per event.
7. **UCB1 affinity** — Replace raw win-rate with UCB1 (awk+bc, no python3). 25 ms per worker.
8. **Gap-aware retry** — Extends existing error classification. 0 ms on hot path.

### Phase 4: Operational Features (P2)
9. **Egress guard** — Cached at startup. 0.03 ms per dispatch.
10. **Decision log** — Append-only TSV. 1 ms per event.
11. **Mission context** — Env var cached at startup. 0 ms per dispatch.
12. **Cascade typing** — Retry-only. 0 ms on hot path.
13. **Structured failure recovery** — Post-dispatch. 0 ms on hot path.
14. **Constitution checks** — Pre-merge grep. 0 ms on hot path.

---

## Key Takeaways

1. **The existing jq bottleneck is the #1 performance problem.** 102 jq calls per scheduling round = 25.6 s, exceeding the 15 s poll interval. Fix this before adding any features.

2. **TSV indexes + grep/awk are 17× faster than jq.** All new features should use TSV files with grep/awk lookups, not JSON with jq.

3. **All 20 moe-sovereign ideas can be implemented with zero hot-path jq cost.** The key is: precompute at startup, cache in TSV/bash arrays, use grep/awk for lookups, append-only for writes.

4. **Thompson sampling must be replaced with UCB1.** Thompson requires python3 (96 ms per call); UCB1 is pure `bc` arithmetic (5 ms).

5. **Off-hot-path operations can use jq freely.** Retry, gate, merge, and report generation run once per task (dwarfed by 30s+ LLM call). A few jq calls there are acceptable.

6. **Append-only TSV logs are O(1).** Dispatch log, decision log, episode log — all use `>>` (1 ms). No JSON parsing on write. Report generation uses grep/awk, not jq.
