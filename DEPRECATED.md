# ⚠️ DEPRECATED — do not use

This branch is the **retired Go port** of agentflow (`go/` directory).

As of 2026-10-09 the **Rust af on `main` is the only maintained version** and
runs fleet-wide. The port's one unique feature — the `command` worker shell
template (GOWORKER) — has been ported back to Rust (commit `5a8d40f` on
`main`), so `main` has full feature parity and more (contract tests, spec
traceability, coverage ratchet, `af validate`/`recover`).

This branch is kept for reference and history only. Do not build, deploy, or
campaign from it.
