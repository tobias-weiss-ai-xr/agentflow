# Tasks: feedback-6-hardening

## 1. Hot-reload (§6.2)

- [x] 1.1 `run_loop`: local `mut cfg` clone; mtime-gated reload of tasks/workers per poll round; refresh `ctx.cfg`; warn-once on reload failure; unit tests (scope fix picked up, broken file keeps last good + warns once)
- [x] 1.2 e2e: edit tasks file between rounds → next attempt uses the new scope (native-harness stub)

## 2. Cooperative stop (§6.4) + crash log (§6.3)

- [x] 2.1 `af.pid` write/remove in `run_loop`; stop-flag check at poll head → drain mode, exit non-zero with stopped marker; flag file consumed
- [x] 2.2 `af stop` subcommand: read pid file, kill -TERM (best effort), create flag file, report stale pid
- [x] 2.3 Panic hook anchored to state dir appends to `crash.log` (default output preserved)
- [x] 2.4 Tests: drain e2e (flag halts dispatch, in-flight reaps, exit non-zero), crash-log unit test, pid-file unit test

## 3. Status header (§6.1) + docs (§6.5)

- [x] 3.1 `status_board` prints `state dir: <path>` first; `--json` unchanged; unit test
- [x] 3.2 README: priority semantics (higher number first; P1-first convention inverts), `af stop` workflow
- [x] 3.3 Feedback doc §6 answered with repro results (Go-port finding)

## 4. Spec hygiene + ship

- [x] 4.1 Traceability markers on every new requirement/scenario; `cargo test --test spec_traceability` green
- [x] 4.2 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, full `cargo test` green
- [x] 4.3 `openspec validate --all --strict` green; archive change; commit + push
