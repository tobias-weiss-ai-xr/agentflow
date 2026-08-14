# taskfleet Roadmap

*Last updated: 2026-08-14*

Planned features ranked by impact and dependency order. Each item links to the
competitive analysis that motivated it.

---

## In Progress (Current Sprint)

### A1. Scope Contention Detection — ✅ done
**Problem:** 7 tasks touch `lib.rs`, 7 touch `layout.rs`. When they run in parallel, merge conflicts are near-guaranteed. The scheduler dispatches blindly.

**Scope:**
- New lib: `lib/schedule.sh` — all scheduling intelligence lives here
- `tf_scope_conflicts <task_id>` → list of running tasks that share any scope file
- `tf_has_scope_conflict <task_id>` → 0/1 boolean for dispatch gating
- `tf_ready_task_ids()` → filter out tasks with active scope contention
- Optional: `TF_CONTENTION_POLICY=defer|allow` (default: defer)

**Acceptance gate:** Two tasks with overlapping scope are never dispatched simultaneously (unless `TF_CONTENTION_POLICY=allow`).

### A2. Critical-Path Priority Scheduling — ✅ done
**Problem:** `tf_ready_task_ids()` returns tasks in config-file order. If a critical-path task and a leaf task are both ready, the leaf grabs the best worker first, increasing wall-clock time.

**Scope:**
- `tf_task_depth <task_id>` → BFS from leaves to find each task's depth in the DAG (precomputed at init, cached in `$STATUS_JSON`)
- `tf_ready_task_ids()` → sort by depth descending (deeper = more critical)
- `tf_status_init()` → compute depths once and store them

**Acceptance gate:** Tasks deeper in the dependency DAG are dispatched before shallow tasks when both are ready.

### A3. Error-Category-Aware Retry Strategies — ✅ done
**Problem:** Every failure gets the same treatment: cooldown → fresh branch → full re-dispatch. But `no_op` (LLM did nothing) will likely repeat, and `timeout` needs a different approach.

**Scope:**
- `no_op` → inject "You MUST modify the following files:" with explicit file list; if second no_op, permanently fail
- `timeout` → increase timeout by 50% and narrow scope to only the primary file
- `compile_error` / `test_failure` → keep current behavior (error feedback already works)
- `merge_conflict` → keep current behavior (branch preserved, works well)
- Config-driven: `retry_policies` map in `workers.json` defaults

**Acceptance gate:** A `no_op` retry injects explicit file-modification instructions; a `timeout` retry increases the timeout.

---

## Upcoming (Next Sprint)

### 4. Worker–Task Affinity (win-rate routing) — ✅ done
**Motivated by:** [amux budget-aware routing](./research/docs/ranked-learnings.md#a1-groundcrew-jiralinear-integration-task-sources)

**Problem:** Workers are picked round-robin / in config order, ignoring task type.
A worker that's a Rust expert (high win-rate on `engine`=rust tasks) gets the same
odds as one that isn't. The receipt ledger now records per-task, per-worker results,
so we can learn which worker is best for which task characteristics.

**Scope:**
- New lib: `lib/affinity.sh` — win-rate routing
- `tf_affinity_score <worker> <task_id>` → historical success rate weighted by recency
- `tf_ready_task_ids()` / dispatch loop → prefer highest-affinity free worker
- Read receipts from `$TF_RECEIPT_DIR` to build per-worker, per-category win rates
- Fall back to config-order when no history

**Acceptance gate:** Given receipts showing worker `git-bot` winning 8/10 rust tasks
and worker `claude-sonnet` winning 2/10, dispatch prefers `git-bot` for a rust task
but respects `TF_TASK_FILTER` / explicit worker overrides.

---

### 5. Cross-Model Routing (booster/fast/standard/deep tiers) — ✅ done
**Motivated by:** [6-patterns doc](./research/docs/ranked-learnings.md#a3-evgeniy-mikhailove-six-production-patterns-documentation)

**Problem:** Every task runs on the default worker set, regardless of complexity.
A trivial file-touch task pays for the premium model; a deep refactor is sent to a
cheap flash model. No way to express "this is a cheap task" or "this needs a deep
thinker".

**Scope:**
- Task field `model_tier`: `booster|fast|standard|deep` (default `standard`)
- Worker field `tiers`: which tiers each worker can handle (default: all)
- `tf_task_tier <task>` / `tf_worker_tiers <worker>` / `tf_worker_can_tier <worker> <tier>`
- Dispatch filters free healthy workers to those capable of the task's tier,
  then applies affinity ranking among them
- `TF_ROUTING=0` disables (ignore worker tiers)

**Acceptance gate:** A `booster` task never dispatches to a `standard|deep`-only
worker; a `deep` task never dispatches to a `booster|fast`-only worker. Workers
without a `tiers` field can run anything.

---

### 6. External Task Sources (GitHub Issues) — ✅ done
**Motivated by:** [groundcrew](./research/docs/ranked-learnings.md#a1-groundcrew-jiralinear-integration-task-sources)

**Problem:** Tasks only come from hand-authored `config/tasks.json`. Work filed in
real issue trackers has to be copied by hand, drifting out of sync.

**Scope:**
- New lib: `lib/sources.sh` — external task adapters feeding `tasks.json`
- `taskfleet import github <owner/repo> [--label X] [--state open] [--dry-run]`
- Fetches open issues via GitHub REST API, converts to taskfleet task schema
- Dedup by stable `GH-<number>` id (idempotent across runs)
- `TF_GITHUB_TOKEN` env for authenticated rate-limit-free access

**Acceptance gate:** Running `taskfleet import github` twice imports each issue
once; imported tasks carry `GH-N` ids and a `source` tag linking back to the
issue; `--dry-run` never mutates `tasks.json`.

---

### 7. Per-File Merge Locks (non-overlapping parallel merge) — ✅ done
**Motivated by:** Merge bottleneck analysis (global merge lock serializes all merges)

**Problem:** In global lock mode, all task merges serialize — even tasks with
completely disjoint scope files. A task touching `model.rs` blocks a task
touching `layout.rs`, costing wall-clock time.

**Scope:**
- New env var: `TF_MERGE_LOCK_MODE=global` (default, backward compatible) or `per-file`
- `lib/worktree.sh`: `_tf_merge_acquire_per_file_locks`, `_tf_merge_release_per_file_locks`, `_tf_merge_lock_dir`
- Per-file locks are fs-safe encoded directory locks under `$TF_STATE_DIR/merge-locks/`
- `tf_worktree_merge` retrieves per-file locks before merging, unlocks on completion / error
- Contention: retries up to 3 times (1s delay between attempts) before failing
- Tasks with disjoint scope merge in parallel; tasks sharing files serialize

**Acceptance gate:** With `TF_MERGE_LOCK_MODE=per-file`, two tasks with disjoint
scope (e.g., `lib.rs` and `model.rs`) can merge concurrently. Tasks with
overlapping scope cannot. Switching `TF_MERGE_LOCK_MODE=global` restores backward
compatible behavior.

---

### 8. Speculative Dispatch (branch-from-dep) — ✅ done
**Motivated by:** Wall-clock reduction analysis

**Problem:** Task B depends on task A. B waits idle while A runs, and only
starts after A merges. This costs wall-clock time when tasks finish at
similar times across the DAG.

**Scope:**
- New functions in `lib/status.sh`: `tf_is_speculatively_ready`, `tf_speculative_base_dep`
- Modified `tf_smart_ready_task_ids` in `lib/schedule.sh` to return both regular and speculative ready tasks
- Modified `tf_worktree_create` call in `lib/dispatch.sh` to pass the running
  dependency's branch as the base when doing speculative dispatch
- Enabled via `TF_SPECULATIVE_DISPATCH_ENABLED=1` (disabled by default for backward compat)
- A task is speculatively ready when: status==ready AND all deps except exactly
  ONE are done, that ONE is running (not blocked/failed), and scope contention
  policy allows dispatch

**Acceptance gate:** With `TF_SPECULATIVE_DISPATCH_ENABLED=1`, when task A is
running and task B depends only on A, B appears in `tf_smart_ready_task_ids`
and is dispatched with a worktree branched from A's current branch. When A
merges to main, B's merge proceeds as a fast-forward onto main.

---

### 9. Multi-Repo Task Support — 🟢 done
**Motivated by:** [agent-dispatch](./research/docs/ranked-learnings.md#a4-agent-dispatch-mcp-based-cross-repo-delegation)

**Problem:** Taskfleet only targets a single git repository. Cross-repo refactors
or coordinated changes across multiple repositories require manual orchestration.

**Design:** See [docs/multi-repo-design.md](./docs/multi-repo-design.md) for complete
architecture, configuration format, and implementation plan.

**Implemented:**
- `REPOS_JSON` config path (`$TF_CONFIG_DIR/repos.json`) for multi-repo configuration
- `tf_task_repo <task_id>` — returns repo name from task's `repo` field (defaults to "")
- `tf_repo_dir <repo_name>` — resolves repo name to absolute path (with fallbacks)
- `lib/worktree.sh` functions (`tf_worktree_create`, `tf_worktree_merge`, `tf_worktree_remove`, `tf_worktree_delete_branch`, `tf_worktree_ensure_gitignore`) modified to use task-specific repo directories
- All git operations now use resolved repo directory instead of `$TF_REPO_DIR`
- Fully backwards compatible: defaults to `$TF_REPO_DIR` when task has no `repo` field

**Configuration example:**
```json
{
  "repos": {
    "main": "..",
    "docs": "../docs-site",
    "infra": "../infra-repo"
  }
}
```

**Task schema:**
```json
{
  "id": "update-docs-readme",
  "engine": "markdown",
  "repo": "docs",
  "scope": ["index.md"],
  "accept": "git diff --stat"
}
```

**Acceptance gate:** ✅ A task with `repo:"docs"` creates its worktree and branch in
the docs repository, and its merge operates on the docs repo's main branch.

---



*See [improvement-ideas.md](./research/docs/improvement-ideas.md) for the full backlog of 18 ideas.*
