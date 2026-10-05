# Tasks: retry-context

## 1. Receipts carry the error line
- [x] 1.1 `Receipt.error: Option<String>` (serde default); capture first line (≤200 chars) in the execute_task wrapper
- [x] 1.2 State test: legacy receipt → error None

## 2. Retry prompt injection
- [x] 2.1 `failure_context(receipts, task_id, attempt)` pure fn + unit tests (first attempt: none; retry: lists earlier failures; merged receipts excluded)
- [x] 2.2 `render_prompt` renders the block for attempt ≥ 2
- [x] 2.3 E2E: after a failing run, final prompt render lists prior attempts; happy-path prompt has no block

## 3. Verification & docs
- [x] 3.1 Full suite green, corpus 64/64, zero warnings
- [x] 3.2 ADR-13 (receipts as episodic memory; cross-task recall skipped — no task types), README note
