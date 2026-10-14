# Proposal: feedback-6-hardening

## Why

The openEduSuite campaign (docs/af-usage-feedback.md §6) burned operator time on
three af gaps: a mis-declared scope kept being replayed from an in-memory config
until every attempt was spent (§6.2), killing the orchestrator orphaned agents
with no crash trace (§6.3/§6.4), and `af status` output never names the state
directory it read — the reported "status ignores TF_STATE_DIR" repro (§6.1)
turned out to be the retired Go port (`/usr/local/bin/af`); the Rust binary
resolves TF_STATE_DIR correctly, but the silent output invited the confusion.

## What Changes

- **Config hot-reload between dispatch rounds** (§6.2): `af run` re-reads
  tasks.json/workers.json each poll round (mtime-gated); a scope/accept fix
  reaches the next retry without a process restart. A broken edited file keeps
  the last good config and warns once — a campaign must not die to a typo.
- **Graceful stop** (§6.4): SIGTERM/SIGINT stops NEW dispatches, lets running
  attempts finish (their normal reap path runs), exits non-zero with a
  "stopped" board; a second signal exits immediately. `af run` writes a pid
  file into the state dir; new `af stop` signals it.
- **Crash file** (§6.3): a panic in the run loop appends to `<state dir>/crash.log`
  (message + location) so an unattended death is diagnosable afterwards.
- **Status names its state dir** (§6.1): the human `af status` board prints
  `state dir: <path>` as its header; `--json` output is unchanged (machine contract).
- **Priority semantics documented** (§6.5): README states the rule explicitly —
  higher number/CRITICAL sorts FIRST; the "P1 = highest" convention is the
  opposite. No behavior change (corpus-compat, d29bf66).

## Capabilities

- **New Capabilities**: (none)
- **Modified Capabilities**:
  - `openspec/specs/config/spec.md` — status output names the consulted state dir.
  - `openspec/specs/lifecycle/spec.md` — hot-reload between rounds, graceful
    stop via signal + `af stop`, crash log.

## Impact

- `src/run.rs` (run_loop reload + signal flag + status board header),
  `src/main.rs` (`stop` subcommand), `src/state.rs` or `src/run.rs` (crash hook),
  `README.md` (priority semantics, stop/resume workflow).
- No new dependencies (plain libc signal handling via existing `unsafe`-free
  approach: signal handler only sets an atomic flag; libc is already an
  indirect dep? — if not, use `nix`-free raw syscall wrapper already present
  in the codebase; fall back to a tiny `unsafe libc` block behind the existing
  unsafe usage pattern).
- Tests: scheduler/run unit tests + one e2e for reload; traceability markers
  for every new requirement/scenario.
