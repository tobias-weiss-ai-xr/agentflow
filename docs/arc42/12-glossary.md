# 12. Glossary

| Term | Definition / Notes |
|------|-------------------|
| **Agent** | External OpenAI-compatible CLI process that agentflow spawns to perform a task (e.g. `pi`, `opencode`). The agent is **untrusted** — it receives a scoped env, runs in an isolated worktree, and its subtask is sandboxes. |
| **Native harness** | In-process agent loop (`cli: "builtin"`, ADR-14): af calls the OpenAI-compatible chat endpoint directly and executes `bash`/`write`/`edit` tool calls in the worktree under the standard env allowlist. No external agent CLI needed. |
| **AgentDone** | `AttemptPhase::AgentDone` — the agent exited 0 and its changes have been committed to the attempt branch.久久 | 
| **Archive** | A git branch created by `archive_branch` to preserve an attempt's committed work when the attempt fails. Name format: `<branch>-rejected-[<attempt>-]<unix-ts>[-<n>]`. Legacy format: `<branch>-rejected-<unix-ts>[-<n>]`. |
| **Attempt** | One invocation of an agent on a task. Incremented on every retry. An attempt is NOT the same as an archived branch; one attempt produces exactly one `Receipt`. |
| **AttemptPhase** | Journal checkpoint in the effect sandwich: `Spawned` (worktree created, agent about to run), `AgentDone` (agent work committed), `GatePassed` (gate passed). Persisted in `TaskStatus.phase`. |
| **auto-reuse** | `af run` pre-dispatch feature: if an archived branch's changes satisfy the task's current scope, re-run only the gate and merge without re-invoking the agent. Default ON; disabled by `TF_NO_REUSE=1`. |
| **Basis** | How a worker's expense is measured: `Priced(f64)` = $/Mtok, `Sized(f64)` = parameter count in billions. See `src/cost.rs`. |
| **Branch prefix** | `TF_BRANCH_PREFIX` — prefix for worktree branches (default `tf`). Example: task `foo` with prefix `af` → branch `af/foo`. |
| **Changed paths** | `git diff --name-only <base>...<branch>` — files changed on the attempt branch relative to the merge base. Always operates on committed state. |
| **Clean** | `af clean [--dry-run]` — removes archived rejected branches and stale worktree directories. Never removes running worktrees or base branches. |
| **Config** | JSON files: `tasks.json` (task definitions), `workers.json` (LLM provider+model slots), optional `repos.json` (multi-repo mapping). |
| **Cost basis** | See `Basis`. A declared basis enables expense comparison and estimation; no declared basis → neutral (excluded from cost tie-breaks). |
| **Count as verdict** | A receipt whose `outcome` is `"merged"` or `"failed"`. `"interrupted"` and `"recovered"` are **non-verdicts** — excluded from trust calculations, WINS/TOTAL, and MEAN_S. |
| **Critical path** | Tasks not blocked by any dependency. ties are broken | |
| **DAG** | Directed Acyclic Graph of task dependencies. Cycles are detected at config time. |
| **dry-run** | `af run --dry-run` — simulate the campaign without invoking agents; reports what would be dispatched, in which order. Exit 0 always. |
| **Effect sandwich** | pi-durable pattern for resuming crashed processes: commit intent → perform effect → commit outcome. Makes workflows idempotently resumable. |
| **Enabled worker** | A worker marked `"enabled": true` in `workers.json`. Only enabled workers are dispatched. The count of enabled workers caps `max_parallel`. |
| **EnvMode** | Sandbox layer for subprocess env: `Inherit` (trusted callers like gate) or `Sandbox` (agent child — empty env + allowlist). |
| **Gate** | Acceptance gate — shell command that must exit 0 for the attempt to pass. Runs in the task's worktree with scoped env. Re-run on recovery. |
| **GatePassed** | `AttemptPhase::GatePassed` — the gate passed. |
| **Gate replay** | `task.gate_replay: bool` — when true, sets `TF_GATE_REPLAY=1` in the gate's env so gate scripts can assert replay-safety (e.g. idempotent checks). |
| **Heal** | `worktree::heal` — at startup, removes stale worktrees from dead attempts and archives their branches to preserve work. |
| **Honest success** | A `Merged` outcome **only** when the work is provably in the base branch: dirty worktree committed, commits_ahead > 0, merge verified via `branch_merged_into_head`. |
| **INTERRUPTED** | `OUTCOME_INTERRUPTED` — receipt outcome for attempts lost when the orchestrator itself died. Wall-clock recorded as 0.0 (unknown). Non-verdict. |
| **Ledger** | `af cost` report. Shows per-task receipts with WALL_S, TOKENS, COST; per-worker WINS/TOTAL, TRUST, MEAN_S, COST; WASTED/RECOVERED/INTERRUPTED lines; WASTED BY REASON grouping. |
| **Lock file** | `<state_dir>/.lock` — records the pid of the single writer `af run` process. Second `af run` gets exit 2. Dead pid → lock reclaimed. |
| **Manual task** | `task.manual: true` — a task with no gate; always reported as Done after its scope is verified. |
| **Max parallel** | `TF_MAX_PARALLEL` — maximum concurrent dispatches. Effective cap = min(max_parallel, enabled_worker_count). Default 1. |
| **Measured cost** | Provider-reported cost via `cost_micros` in the receipt. Outranks every declared basis. Reported as `$X.XXXX` in the ledger. |
| **Merge** | `worktree::merge` — merges the attempt branch into the base branch with `git merge --no-ff`. Serialized per repository via `MergeLocks`. |
| **Outcome** | Receipt field recording the final disposition: `"merged"`, `"failed"`, `"interrupted"`, `"recovered"`. Only `"merged"` and `"failed"` count as verdicts. |
| **Poll** | `TF_POLL` — poll interval in seconds (default 15). Wake-on-completion reduces effective latency. |
| **Prompt template** | `prompts/worker.md` — render template for the agent prompt. Injected placeholders: `{{TASK_ID}}`, `{{TASK_TITLE}}`, `{{SCOPE}}`, `{{ACCEPTANCE}}`, `{{ACCEPT_CMD}}`. |
| **Ready** | A task whose deps are all Done, is not Running, has attempts < max_attempts, and does not contend on scope with a Running task. |
| **Recovery** | `af recover --task ID [--attempt N] [--dry-run]` — re-validate and merge an archived rejected branch without re-invoking the agent. |
| **RECOVERED** | `OUTCOME_RECOVERED` — receipt outcome written when `af recover` or auto-reuse merges an archived branch. `wall_clock_s: 0.0`. Non-verdict. Pairs with the original failed receipt it reclaims. |
| **Receipt** | Record of one attempt: task, attempt, worker, model, wall_clock_s, tokens, ts, outcome, error, cost_micros. Written atomically. |
| **Resume action** | Derived from `AttemptPhase`: `RerunAgent` (None / Spawned), `RerunGate` (AgentDone), `MergeOnly` (GatePassed). |
| **Router** | `router.rs` — UCB1 worker selection from receipts: `score = mean + explore_term`. Tie-breaks: declared cost → mean duration → config order. |
| **Running** | `TaskState::Running` — a task currently being attempted. |
| **Scope** | `task.scope: Vec<String>` — file globs that the task is allowed to modify. Empty = any file. |
| **Spawned** | `AttemptPhase::Spawned` — worktree created, agent about to spawn. |
| **Spec → Contract → Test Pyramid** | ADR-5 workflow: OpenSpec change first, contract tests at module boundaries, test pyramid underneath. |
| **Stall watchdog** | `TF_AGENT_STALL_S` — kills an agent that produces no output for N seconds. Distinct from timeout. |
| **State dir** | `TF_STATE_DIR` — directory containing `run-state.json`, `state.lock`, `receipts/`, `logs/`, `worktrees/`. |
| **Status** | `TaskState` — per-task state: `Ready`, `Running`, `Done`, `Blocked` |
| **Task** | Declarative unit: id, title, deps, scope, touch, accept, accept_timeout_s, manual, repo, priority, gate_replay, retry_delay_s. |
| **TaskStatus** | Persisted per-task state: state, attempts, last_error, phase, attempt_started_ts, attempt_worker. |
| **Token** | LLM input/output token count. Captured from JSON-mode transcript when available; measured via `Usage.total_tokens` or provider's own report. |
| **Total** | `TOTAL` line in `af cost` — sum of all selected receipt wall_clock_s and count. |
| **Touch** | `task.touch: Vec<String>` — files the task intends to modify (advisory). `af validate` rejects any `touch` entry not covered by `scope`. |
| **Trust** | `wins / total` from receipt **verdicts** for a worker. Excludes interrupted and recovered. Reported in `%` in `af cost`. |
| **UCB1** | Upper Confidence Bound 1 — bandit algorithm: `score = mean_reward + √(2 * ln(N+1) / (n+1))`. Balances exploitation vs exploration. |
| **Wall-clock** | Measured elapsed time per attempt. Receipt field `wall_clock_s: f64`. Primary duration metric. |
| **Work tree** | Git worktree — isolated filesystem tree for one attempt. Created per attempt, removed on cleanup. |
| **Worker** | One provider+model slot: name, provider, model, api_base, api_key_env, enabled, cli, output, args, params_b, price_per_mtok_usd. |
| **Worker basis** | The `params_b` / `price_per_mtok_usd` pair used to compute `Basis`. Must be finite and > 0 to be usable; validated at load time. |
