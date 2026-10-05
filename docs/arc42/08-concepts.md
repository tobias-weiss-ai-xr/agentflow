# 8. Cross-cutting Concepts

## 8.1 Task state machine

| State | Meaning | Guarded transitions |
|-------|---------|---------------------|
| `ready` | DAG-eligible, queued | → `running` on free worker + no contention |
| `running` | attempt in flight | → `done` (gate+merge ok) \| `failed` (gate/agent fail, attempts exhausted) \| `ready` (retry with fresh branch) |
| `done` | merged to base branch | terminal |
| `failed` | attempts exhausted, or dep failed | terminal; also trigger of deadlock exit |

- Persisted **before** every visible transition; idempotent re-run resumes from
  deepest `done`.

## 8.2 Concurrency & serialization

- One process, one writer; atomic JSON (temp + rename) for all `state/` files.
- Merges are serialized per repository (single merge mutex in-process).
- Contention: tasks whose `scope` globs overlap are not dispatched
  concurrently (default `defer`).

## 8.3 Subprocess lifecycle (shared helper)

- spawn → optional streaming tail to log → wait with timeout → on timeout or
  kill: kill entire process tree → classify by exit code (0 / non-zero /
  killed-by-us / missing binary). This is the *one* path that `git`, agent, and
  gate all use, so exit-contract behavior is uniform and contract-tested once.

## 8.4 Acceptance gate

- `accept` is a shell string run in the worktree, subprocess helper semantics,
  bounded by `accept_timeout_s`, env scoped via `TF_GATE_ENV`. exit 0 = pass;
  output captured for `af api results`.

## 8.5 Scope checking (advisory)

- After an attempt, diff against base; files outside declared `scope` globs are
  reported (advisory — does not fail the gate by default). Used by contention
  scheduling regardless.

## 8.6 Receipts & cost

- Every attempt appends a receipt (task, attempt, worker, model, wall-clock,
  tokens if reported by the agent CLI) → `state/receipts/`; `af cost` aggregates.
  Wall-clock, not token-count, is the source of truth for "elapsed".

## 8.7 Logging & transparency

- Per-task logs → `state/logs/<task>.log`; `af attach <task>` tails live.
- Status board is human (`af status`) and machine (`af status --json` / `af api status --json`) readable.
- `af validate` pre-flights config (cycles, duplicate ids, unknown workers) without dispatching; `af clean [--dry-run]` sweeps orphaned worktrees/branches from crashed runs.

## 8.8 Sandboxing the agent child (ADR-10)

The agent CLI executes LLM-directed tool calls — treat it as **untrusted**.
Three portable layers, in enforcement order:

1. **Env allowlist (default-on).** The child starts empty and receives only:
   system basics (PATH, HOME/USERPROFILE, temp dirs, Windows loader keys), git
   commit identity (`GIT_AUTHOR_*`/`GIT_COMMITTER_*`), the **dispatched**
   worker's `api_key_env`, and `TF_AGENT_ENV_PASSTHROUGH` (comma-separated
   extras). Other workers' keys and orchestrator secrets are withheld.
   Gates and af's own git calls inherit the full env (trusted code).
2. **Git hygiene (default-on).** Children get `GIT_TERMINAL_PROMPT=0` and an
   empty `credential.helper` (via `GIT_CONFIG_*` env): no credential popups,
   no hangs on auth prompts.
3. **Wrapper seam (opt-in).** `TF_SANDBOX_CMD` is whitespace-split and
   prepended to the agent argv.

**Limits (be honest):** env hygiene is not containment. The worktree inherits
the repo's remotes; a malicious agent with network + stored credentials could
still attempt `git push` or exfiltrate worktree contents. Enforce with the
wrapper:

| OS | Recipe |
|---|---|
| Linux | `TF_SANDBOX_CMD="bwrap --unshare-net --ro-bind / / --bind <wt> <wt> --dev /dev --proc /proc"` or `firejail --net=none --private=...` |
| macOS | `sandbox-exec -f <profile>` (Seatbelt profile denying network/write-outside) |
| Windows | run af inside a restricted Job/AppContainer or a container; no in-process equivalent |

## 8.9 Error contracts

- All module public fns return typed errors (`Result`) with a stable message;
  no silent partial writes; a crash mid-write never corrupts state (8.2/6.2).
