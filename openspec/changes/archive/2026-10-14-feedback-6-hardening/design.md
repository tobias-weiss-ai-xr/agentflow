# Design: feedback-6-hardening

## D1 — Hot-reload is mtime-gated full-config reload, not a patch

Each poll round of `run_loop` compares `(tasks_file, workers_file)` mtimes
against the last-seen pair; on change it calls `config::load` again. On parse
or validation failure it keeps the last good config and prints ONE warning
per generation (a broken file must not spam every 15 s poll, and must never
kill a running campaign — the operator fixes the typo and the next mtime
change picks it up). `cfg` becomes a local `mut` clone; `ctx.cfg` is refreshed
so dispatch threads see the new definition. No signature changes, no test
churn.

## D2 — Cooperative stop via flag file, not signal handlers

`af run` checks `<state dir>/stop` each poll round. Present → drain mode: no
new dispatches; when `running` is empty the loop exits 3 with the board.
The flag file is consumed (deleted) by `af stop`. Rationale: zero new deps,
no unsafe, works over shared filesystems, and a hard kill stays SAFE by the
existing effect-sandwich heal (running/spawned attempts get an interrupted
receipt and a clean resume). `af run` also writes `af.pid` into the state dir
(removed on exit); `af stop` reads it and shells out to `kill -TERM` — best
effort: killing the orchestrator early just hands control to the heal path.
Liveness check is `/proc/<pid>` existence (Linux dev/CI hosts; a stale pid
file is reported as such, never fatal).

## D3 — Crash log via panic hook anchored to the state dir

`main` installs `panic::set_hook` after `load_cfg` resolved the state dir
(stored in a `OnceLock<PathBuf>`). The hook chains to the default hook
(stderr message preserved) and appends one line to `<state dir>/crash.log`:
timestamp, thread, panic message, location. Diagnosable unattended deaths
without changing any control flow (panics keep their semantics).

## D4 — Status header, not a JSON contract change

The human board prints `state dir: <path>` first. `--json` stays untouched:
machines parse it, and the reporter's §6.1 confusion came from reading the
human output. The §6.1 bug itself is not reproducible on the Rust binary
(reproed locally: TF_STATE_DIR is honored) — it was the retired Go port at
`/usr/local/bin/af`.

## D5 — Priority semantics: document, don't flip

`Priority::Num(n)` sorts higher-first (CRITICAL=20 … LOW=-10). The corpus
convention "P1 = highest" inverts that, which produced the §6.5 surprise.
Flipping would break corpus compat (d29bf66) and the numeric scale's
consistency with the string levels. README gets the explicit rule plus the
inversion warning; no code change.

## Non-goals

- No signal handlers (D2), no PID liveness beyond /proc existence, no remote
  stop propagation beyond the shared state dir, no per-field config diffing
  (D1 reloads wholesale).
