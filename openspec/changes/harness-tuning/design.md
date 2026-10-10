# Design: harness tuning

## Write-time scope guard (builtin harness)

- **Where**: `harness::run_tool` dispatches both `write` and `edit` through a new
  `guard_path(wt, path, scope, readonly)` before the underlying
  `tool_write`/`tool_edit` run.
- **Matcher reuse**: `guard_path` calls the existing
  `execute::scope_violations(&[rel], scope)` (→ `scheduler::scope_overlap`), the
  exact matcher the end-of-attempt enforcement uses, so admission and
  enforcement can never disagree. `rel` is worktree-relative, `/`-normalized
  (matches `git diff --name-only` output on Windows).
- **Semantics**: readonly → reject always; scope empty → allow all; else reject
  when the path matches no scope entry. The error text names the path and the
  allowed scope and returns as the tool result (model can correct course).
- **Backstop kept**: the end-of-attempt check stays — `bash` can write anywhere
  and CLI workers bypass the harness entirely; they are still caught there.
- Tool functions themselves are unchanged (only dispatch gained the guard), so
  their existing tests stand.

## Per-task turn cap

`Task.max_turns: Option<u32>`; `harness::run` takes `task_max_turns` and
`effective_max_turns(worker, st, task)` resolves task > worker >
`TF_AGENT_MAX_TURNS` > 32. config validation rejects `Some(0)`.

## Readonly tasks

`Task.readonly: bool`. Two enforcement points:
- harness: `guard_path` rejects every write/edit when readonly.
- execute.rs: after the agent stops normally, a readonly attempt returns
  `Outcome::Merged` (success) directly — no gate, no merge, no "no change"
  failure — after the standard preserve/cleanup path. config validation
  rejects `readonly` with an `accept` gate.

## Router Laplace prior

`pick()` exploitation term becomes `(wins + 1)/(n + 2)`. `trust()` (cost
report) stays raw. Rationale: with N=1–2 the raw rate is 0-or-1 noise; the
smoothed mean cannot diverge once n grows (converges to raw). Existing
decision-level router tests all still hold; one new test pins the small-N
ordering change, one pins trust() staying raw.

## Gate/agent-tool shell on Windows (driven by the dogfood evidence)

`gate::shell()` now probes once (OnceLock) for a reachable POSIX `sh`
(git-bash) on Windows and returns `("sh", "-c")`; falls back to `cmd /C` only
when no `sh` exists. One shared function feeds both `run_accept` and the
harness's `tool_bash`, so a gate dialect that works for the runner works for
the agent. This is the root-cause fix for the r17b/r17c/r18 doc-consistency
gate failures ("'.' is not recognized" under cmd).
