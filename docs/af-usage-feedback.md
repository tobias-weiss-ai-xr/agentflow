# agentflow — usage feedback & improvement proposals

Grounded in real campaign usage (World-Office parity + spec/contract/test
pyramid work, Oct 2026; workers = `opencode` via a shared `litellm` proxy on
`glm-4.7`, 1-3 declarative tasks per campaign, acceptance = exit-code gates).
Only *confirmed gaps* are listed; everything already implemented (isolated
worktrees, parallel dispatch, multi-repo, UCB1 routing, retry memory, exact
acceptance gates, honest success, recovery ledger + `af recover --attempt`,
dependency DAG, `--json` / `status --json`, `--poll`) is **not** re-reported.

---

## 1. Detached-run survival (ergonomics)

**Observed.** From a short-lived shell, `af run --once` does the dispatch under
the invoking subprocess tree and does not survive the shell exiting. I have to
wrap every campaign manually:

```sh
setsid env TF_REPO_DIR=… TF_TASKS_JSON=… TF_WORKERS_JSON=… TF_STATE_DIR=/tmp/af-x \
  TF_BASE_BRANCH=main af run </dev/null >/tmp/af-run.log 2>&1 &
```
then poll `af status` from another shell.

**Ask.** `af run --detach` (or `--daemon`) that detaches the run (setsid-style,
stdin/out/err to a log) and prints the state-dir + a `--pid`/`recover` line so a
caller can tail/join. Optional `af wait [--state-dir X]` that blocks until the
run reaches a terminal state (complements `--poll`, which already gives a
blocking loop today).

## 2. Concurrency / rate control toward shared stateful providers

**Observed.** `workers.json` has no cap on in-flight tasks per worker. Three
independent tasks dispatch **in parallel** and hammer one shared `litellm`
proxy → request bursts, occasional 429/timeouts that then show up as failed
attempts.

**Ask.** Per-worker `max_in_flight` (default = number of tasks), an optional
request/decoded-token budget, and short exponential backoff on provider
transport errors (429/5xx/connect). Tracked alongside the existing
`retry_pacing` timing work.

## 3. Classify non-agent (infra/env) failures separately

**Observed.** When the gate or the agent fails because the proxy is down
(connect error, 429, timeout) rather than because the work is wrong, the
attempt is recorded as *a failed attempt by that worker*. Over time this skews
the UCB1 trust data against an honest worker and muddies `af status`.

**Ask.** A distinct failure class for provider/environment errors (tag the
receipt `kind=infra` vs `kind=agent`), surfaced in `af status --json` as
`infra_fail`; do **not** feed infra failures into the worker's trust/UCB1 score,
and make them re-dispatchable without burning a worker's retry budget.

## 4. First-class "verify" tasks (no code merge)

**Observed.** In the spec/contract/test-pyramid workflow a slice is often
*"write/refresh a probe, it must pass, archive a receipt"* — there is no useful
code diff to land. The current model assumes a task always commits code before
the gate, so a verification-only slice is awkward (would need a no-op commit).

**Ask.** A task flag/type (`verify: true`) whose "success" is: run the `accept`
gate to 0 and write the receipt; merge is skipped or limited to a report
artifact. `af status --json` reports `verdict: verified`.

## 5. Per-contract acceptance granularity (spec/contract pyramids)

**Observed.** `accept` is a single exit-0 shell command. For a contract set
(`C-M1…C-Px`) the gate is binary: pass or fail — `af` can't tell *which*
contract broke, so a failed campaign restart retries the whole slice blindly.

**Ask.** Let `accept` optionally emit a machine-readable report
(`accept_report.json`, e.g. `{"C-M1":"pass","C-M3":"fail","C-P5":"skip"}`).
When present, surface per-contract verdicts in `af status --json` and (later) in
the retry prompt so the next attempt is seeded with the exact failing contract.

---

**Note on process:** agentflow is openspec-driven (`openspec/`), so the natural
next step is to fold the accepted items here into OpenSpec change proposals
(esp. §2, §3, §5) before implementation.