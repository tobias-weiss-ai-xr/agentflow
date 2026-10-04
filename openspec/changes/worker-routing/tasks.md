# Tasks: worker-routing

## 1. Receipts carry outcomes
- [x] 1.1 `Receipt.outcome: String` (serde default `"merged"`); receipt append wrapped around every attempt (merged + failed)
- [x] 1.2 State tests: legacy receipt parses; failed+merged sequence aggregates

## 2. Router (UCB1)
- [x] 2.1 `src/router.rs`: from_receipts / record / pick / trust; tie-break config order
- [x] 2.2 Unit tests: fresh-state order, exploration, exploitation, trust math
- [x] 2.3 `run.rs` wiring: replay at startup, pick among free eligible, record on Msg::Done

## 3. Surface
- [x] 3.1 `af cost` per-worker trust section
- [x] 3.2 E2E: receipts outcomes after a run (merged + failed paths)

## 4. Verification & docs
- [x] 4.1 Full suite green (back-compat: fresh state = old behavior), corpus 64/64
- [x] 4.2 ADR-12 (measured routing over first-free; cumulative history), README note
