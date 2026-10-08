# 2. Constraints

## Technical

| Constraint | Value |
|------------|-------|
| Language / edition | Rust 2021+ (toolchain: cargo/rustc 1.99) |
| Async runtime | **None** — std `thread::scope` per-attempt threads; no tokio (ADR-2, amended) |
| Deliverable | Single static binary `af` (plus crate `agentflow`) |
| LLM interaction | **Subprocess** to an OpenAI-compatible agent CLI (`pi --provider X --model Y -p @prompt`). No direct HTTP to LLM providers. *(ADR-1)* |
| Git interaction | `git` CLI as subprocess. No libgit2. |
| Config | JSON via `serde`; `tasks.json` / `workers.json` / optional `repos.json` schema-compatible with taskfleet's shipped examples |
| State | JSON files under `state/`, atomic writes (temp + fsync + rename), single writer process, lock file *(ADR-3)* |
| Receipts | JSON files under `state/receipts/`, one per attempt, atomic write (unique temp filename + fsync + rename) |
| Test suite | 340 tests (unit + integration + E2E + contract); line-coverage ratchet at 94% (`.coverage-min`, `scripts/coverage-gate.sh`) |
| Dependencies | `serde`, `serde_json` only — zero runtime deps beyond std |

## Organizational / Process

| Constraint | Value |
|-----------|-------|
| Repo visibility | agentflow = **public** (GitHub, MIT); taskfleet = **private** internal *(ADR-8)* |
| Development workflow | **Spec → Contract → Test pyramid** for every change *(ADR-5)*: OpenSpec spec first, executable contract tests, then unit/integration/E2E pyramid |
| Documentation | arc42, one file per chapter in `docs/arc42/` *(ADR-7)*, maintained in the same change as the code |
| Dogfooding | agentflow is used to improve agentflow (14 rounds, 167 commits); every feature is spec'd, tested, and probed before/after against a pre-change binary |
| No secrets | Public repo must never contain credentials, hosts, or internal campaign configs |

## Runtime Environment

- Requires on `PATH`: `git`, an OpenAI-compatible agent CLI (e.g. `pi`).
- Environment variables (see glossary for full list):
  - `TF_REPO_DIR` — the git repo to operate on
  - `TF_STATE_DIR` — state directory (status, receipts, logs, worktrees)
  - `TF_MAX_PARALLEL` — max concurrent workers (capped by enabled-worker count)
  - `TF_BRANCH_PREFIX` — branch prefix (default `tf`)
  - `TF_POLL` — poll interval seconds (default 15; wake-on-completion makes this an upper bound)
  - `TF_GATE_ENV` — comma-separated env vars to pass to the gate
  - `TF_TASKS_JSON` / `TF_WORKERS_JSON` / `TF_REPOS_JSON` — config file paths
  - `TF_AGENT_TIMEOUT_S` — total agent timeout (default 3600)
  - `TF_AGENT_STALL_S` — stall watchdog window (default 0 = disabled)
  - `TF_MAX_WALL_CLOCK_S` — campaign wall-clock budget (default 0 = unlimited)
  - `TF_SANDBOX_CMD` — wrapper command prepended to the agent argv
  - `TF_AGENT_ENV_PASSTHROUGH` — comma-separated extra env vars for the agent child
  - `TF_NO_REUSE` — disable auto-recovery of archived branches (default unset)
- Runs on Linux, macOS, and Windows (WSL); CI uses GitHub Actions with Ubuntu.

## Non-negotiable Boundaries

- `af` orchestrates; it does not originate: no agent loop, no tool calling, no embedded git server.
- v1 does **not** include: Bayesian routing, episodic memory, trust/constitution, corrections, transparency, openspec bridge, vLLM worker. These are v2 candidates (referenced in 04).
- The agent child is **untrusted**: it receives a scoped env (ADR-10), never the orchestrator's full environment.
- A `Merged` result is never inferred from an exit code — the work must be in the base.
- A failed attempt's committed work is never destroyed — it is archived and recoverable.
