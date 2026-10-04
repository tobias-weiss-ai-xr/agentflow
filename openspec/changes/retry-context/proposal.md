# Proposal: retry-context (episodic memory for retries)

## Why

On retry, the agent starts blank: it does not know the previous attempt
already failed, or why. The MoE-sovereign doc's "episodic memory" and
"self-correction few-shot" both inject exactly this signal into the prompt.
The bash designs use TSV side-stores because they lack a receipt store —
af already persists one receipt per attempt (ADR-12). The lazy version:
**give receipts the error line, and render prior failures into retry
prompts.**

Skipped deliberately: cross-task episode recall and global "last N
episodes" injection. af tasks have no `type`, so similarity matching would
be fake; per-task failure context is the honest, useful subset
(ponytail: revisit when task types exist).

## What Changes

1. **`Receipt.error: Option<String>`** — first line of the failure reason,
   capped at 200 chars; `None` on merged attempts and for legacy receipts
   (serde default).
2. **Retry prompt injection.** For attempt ≥ 2, `render_prompt` appends a
   "Previous attempts" block built from this task's failed receipts
   (attempt number + error line). Attempt 1 renders unchanged.

## Capabilities

### Modified
- `state` — receipts carry the error line.
- `lifecycle` — retry prompts include prior-attempt failure context.

## Impact

- `src/state.rs`: field + test.
- `src/execute.rs`: error capture in the receipt wrapper; `failure_context`
  (pure, unit-tested); `render_prompt` gains the context param.
- E2E: final prompt render of a failed task contains prior failures.
