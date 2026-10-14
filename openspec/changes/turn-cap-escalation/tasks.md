# Tasks: turn-cap-escalation

## 1. Escalation

- [x] 1.1 Add `TURN_CAP_CEILING` const + pure fn `escalate_turns(receipts,
      task_id, attempt, base) -> Option<u32>` in `src/execute.rs` (parse
      `harness: turn cap N reached` from prior failed receipts, `saturating_mul(2)`,
      clamp 128; None → pass base through)
- [x] 1.2 Unit tests: no prior cap failure → base unchanged; single cap death
      12 → 24; caps 64+128 → 128 (ceiling); unrelated failure → unchanged;
      survives synthetic receipt ordering
- [x] 1.3 Wire into the builtin-harness call site: replace `task.max_turns`
      argument with `escalate_turns(&ctx.store.load_receipts(), id, attempt,
      task.max_turns)`

## 2. Validate warning

- [x] 2.1 `config.rs::validate`: non-fatal warning when `accept` contains
      `--workspace` while scope is non-empty and lacks `**`-prefixed entries
- [x] 2.2 Unit test: narrow scope + workspace gate → warning emitted; empty
      scope → no warning

## 3. Docs + validation

- [x] 3.1 README task-schema rows: turn-cap escalation behavior + the
      gate/scope mismatch warning
- [x] 3.2 `openspec validate --changes --strict` passes; `cargo clippy
      --workspace --all-targets` clean; `cargo test` green
- [x] 3.3 Dogfood: one-shot config with a deliberately tight cap on a
      task that needs more turns → attempt 2 escalates (log line shows the
      raised cap)
