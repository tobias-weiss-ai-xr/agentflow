# Coverage ratchet

`agentflow` enforces a **line-coverage floor** in CI so coverage can only move
up. The measured baseline on the current tree is **94.19% of lines**; the floor
is set just below it at **94%** and may only be raised from there.

## What it enforces

`scripts/coverage-gate.sh` reads the minimum from `.coverage-min` (a single
number) and runs:

```sh
cargo llvm-cov --workspace --fail-under-lines "$MIN"
```

If total workspace line coverage falls below `$MIN`, `cargo-llvm-cov` exits
non-zero and the script prints a short explanation. The CI `coverage` job runs
`./scripts/coverage-gate.sh`, so a drop below the floor **fails the build**.

The threshold lives in exactly one place — `.coverage-min` — and is *not*
duplicated in `.github/workflows/ci.yml`.

| File | Role |
|------|------|
| `.coverage-min` | Single line with the minimum line-coverage percentage (e.g. `94`) |
| `scripts/coverage-gate.sh` | Reads the floor, runs `cargo llvm-cov --fail-under-lines`, fails loudly |
| `.github/workflows/ci.yml` (`coverage` job) | Invokes the gate on every push/PR |

## Run it locally

```sh
# cargo-llvm-cov must be installed (once):
cargo install cargo-llvm-cov
rustup component add llvm-tools-preview

# Enforce the floor:
./scripts/coverage-gate.sh

# Just see the report without enforcing anything:
cargo llvm-cov --workspace --show-missing-lines
```

> Windows toolchains can't produce the report locally — no
> `profiler_builtins`. Run the gate on Linux/macOS or in CI.

## The ratchet rule

The number in `.coverage-min` is a **ratchet**: it may only be **RAISED**,
never lowered to make a red build green. When a change lifts coverage, bump
`.coverage-min` to the new floor (rounded down to a whole percent with a small
margin, so the gate stays stable) in a reviewed commit.

Never lower `.coverage-min` silently. If you find yourself wanting to lower it,
that is a signal that untested code landed — fix the code, not the gate.

## When new untested code legitimately drops coverage

Prefer, in order:

1. **Add tests** for the new code paths and re-run `./scripts/coverage-gate.sh`.
   This is almost always the right answer and keeps the ratchet moving up.
2. **Raise the floor deliberately** only when coverage *increases* — record the
   new value in `.coverage-min` in a commit whose message says so.
3. **Lower the floor** only as an explicit, reviewed, last-resort trade-off
   (e.g. a large generated/excluded module). It must be visible in the diff and
   called out in the commit message; never sneak it in to silence CI.

If your run reports a number **below** the current floor, the code changed.
Investigate the new uncovered lines (`--show-missing-lines`) before touching
`.coverage-min`.

## Raising the floor

```sh
# After coverage improves, update the single source of truth:
printf '%s\n' 95 > .coverage-min
./scripts/coverage-gate.sh   # must pass at the new floor
```

Then commit `.coverage-min` together with the tests that made the increase real.
