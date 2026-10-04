# Rust vs Go: the af core loop, side by side

*2026-10-05 — the Go port (`go/`) was written by an LLM agent (glm-4.6) dispatched
through af itself, gated by `go build && go vet && go test` plus an e2e test
mirroring `tests/e2e.rs`. First attempt. This report compares the result.*

## Scope of the comparison

The port covers af's **campaign core**: tasks/workers JSON config, per-task git
worktrees, subprocess agent dispatch with prompt rendering, acceptance gates,
retry with attempts, atomic run-state, ff-merge on success, `run`/`status`/
`attach` CLI, deadlock exit. Not ported (and therefore not compared): af's
auxiliary commands beyond the campaign loop.

## Numbers

| Metric | Rust (af) | Go port (`go/`) |
|---|---|---|
| Source LOC (src only) | 3500 | 1096 |
| LOC incl. own tests | 4278 | 1096 (tests inside) |
| Third-party deps | 2 (serde, serde_json) | 0 (stdlib only) |
| Release binary | 2.0 MB | 3.7 MB |
| Clean build + test | 48.8 s | 22.7 s |
| E2E suite | green (44 tests total) | green (e2e 9.9 s) |

## What the port proved

1. **The loop is language-trivial.** ~1100 lines of Go reproduce the part of af
   that runs campaigns. The Rust surplus (3x) is mostly auxiliary commands,
   richer state/board printing, and test volume — not the core loop.
2. **Zero-dep is easy in Go, mild in Rust.** The port needed nothing beyond
   stdlib (`os/exec`, `encoding/json`, `flag`). af's only deps are serde/serde_json;
   dropping them would cost manual JSON ergonomics that serde earns.
3. **Toolchain speed flips.** Go: 22.7 s cold build+test vs Rust 48.8 s; the
   edit-check loop during the port was visibly faster. Rust pays back with a
   2 MB binary (Go 3.7 MB) and compile-time guarantees the agent's first
   attempt benefits from — af's 70 Rust tests plus the compiler have caught
   everything since the campaign bugs were fixed; Go leans fully on tests.
4. **Windows/unix portability cost is real in both.** The gate runner (`cmd /C`
   on Windows) and path handling needed build-tag care in Go just like Rust.
   Both run the same e2e scenario green on Windows.

## Verdict

For af's actual job — orchestrate, gate, merge, get out of the way — the Go
port is sufficient and cheaper to read (1/3 the lines, no lifetimes, no
traits). Rust stays the right choice for the shipped tool: smaller binary,
stronger refactoring safety on a codebase that keeps growing features, and the
existing investment. The port remains on the `go` branch as the working
counterexample.

## Reproduce

```sh
git checkout go
cd go && go build ./... && go test ./...   # Go side
cargo test                                 # Rust side, on main
```
