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

**Implementation reality discovered while writing this (important — ask).**
The binary installed on the host (`/usr/local/bin/af`) is **a Go port** whose CLI
is `af run --once | status | attach ID` and whose env already includes
`TF_MAX_PARALLEL` and `TF_POLL`. The repo this file lives in describes a
**Rust** orchestrator. My day-to-day campaigns run the Go binary, so the gaps
above were observed against Go; two are already partly answered there:
`attach` (re: §1 detached-join) and `TF_MAX_PARALLEL` (re: §2 concurrency).
**Ask upstream:** which implementation is canonical, and should the Rust repo
track the Go features (`attach`, `TF_MAX_PARALLEL`, `TF_POLL`) so they are not
re-implemented? If Go is canonical, move these items into the Go issue
tracker and keep the Rust repo's docs honest about being a parallel port.

---

## Rust-port campaign learnings (first real run, 2026-10-08)

A 2-task campaign on the Rust `af` (build from this repo) against the
World-Office honing slices shipped 1 merged + 1 recovered. Grounded findings:

1. **The Rust port silently ignores a legacy worker `command`.** A
   `workers.json` written for the old convention
   (`command:"opencode run -m …"$(cat {prompt})""`) makes the agent exit 1 in
   ~4s because `spawn_argv` builds `cli --provider P --model M -p @prompt`
   and never touches `command`. `af validate` said *config OK* with no hint.
   **Improvement:** `af validate` should warn when a worker carries a stale
   `command` / an unknown key and print the current CLI arg contract
   (`--provider/--model/-p @file`), and `README`+`docs` should document the
   adapter requirement for CLIs that don't accept that shape.
2. **Non-opencode CLIs need an adapter.** opencode's real shape is
   `opencode run -m M "<prompt>"` — incompatible. A 30-line shell adapter
   (`scripts/af-opencode.sh`) that swallows `--provider/--model/-p` and execs
   opencode unblocked the campaign. Worth a `docs/worker-cli-contract.md`.
3. **`af recover` recovered a task whose only failure was a bad acceptance
   gate — without re-running the agent.** First attempt of the gate was a
   shell bug (`[: : integer expected`, exit 2); fixing the gate in
   `tasks.json` and calling `af recover --task ID` re-validated it against
   the kept work and marked it done instantly. Excellent behavior.
4. **Gate failure detail is lost in status.** `af status` showed only
   `acceptance gate failed (exit 2)`; the gate's `stderr` (`[: : integer
   expected`) required reading the run log. **Improvement:** carry the gate's
   first error line into the receipt `error` / `status --json`.
5. **`af status` unexpectedly needs a tasks file.** Running `af status` from
   a CWD where `config/tasks.json` defaulted elsewhere reported an unrelated
   stale campaign (`WO-APPLYOP-SMOKE`) until `TF_TASKS_JSON`+`TF_STATE_DIR`
   were supplied. **Improvement:** `status` should render from the state dir
   and default to it without requiring `--tasks` (a registry of the active
   run, cf. §1 `af wait`).

**Note on process:** agentflow is openspec-driven (`openspec/`), so the natural
next step is to fold the accepted items here into OpenSpec change proposals
(esp. §2, §3, §5) before implementation.