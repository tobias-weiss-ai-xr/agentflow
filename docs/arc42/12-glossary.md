# 12. Glossary

| Term | Definition |
|------|-----------|
| **acceptance gate** | Shell command (task `accept`) run in the worktree; exit 0 = pass. The runtime contract test of a task. |
| **agent CLI** | OpenAI-compatible CLI (`pi`, opencode, …) that `af` shells out to for the LLM step. External; never reimplemented. |
| **arc42** | Template for documenting software architecture in 12 chapters; this documentation's format (one file per chapter). |
| **ADR** | Architecture Decision Record — a numbered decision with context/consequence (chapter 9). |
| **atomic write** | temp-file + rename persistence; a crash never leaves a partially written file. |
| **contention** | Rule that tasks with overlapping `scope` globs are not dispatched concurrently (defer). |
| **contract test** | Executable test that a module's public function honors its contract (totality, error/exit codes, schema). Middle tier of the pyramid. |
| **DAG** | Directed acyclic graph of `deps` between tasks; built by the scheduler. |
| **deadlock** | State where every remaining task is blocked by failed deps/attempts → orchestrator exits cleanly. |
| **E2E test** | End-to-end test running the real pipeline (opt-in, real agent, scratch repo). |
| **episodic memory** | (v2) Wall-clock-anchored record of past runs. Excluded from v1 (ADR-6). |
| **fake agent** | Test double replacing the agent CLI in default test runs (deterministic exit codes / outputs). |
| **receipt** | Record of one task attempt: worker, model, wall-clock, tokens → `af cost` input. |
| **scope** | File globs a task is allowed to modify (advisory check + contention input). |
| **single writer** | Property that exactly one process (the orchestrator) mutates `state/` — no cross-process races. |
| **spec** | OpenSpec change: behavior + requirements + design defined before code (ADR-5). |
| **taskfleet** | The internal, private bash orchestrator (campaign runner). Source of proven semantics; not documented here. |
| **test pyramid** | unit ≫ integration ≫ E2E; the bottom tier of the spec → contract → test pyramid. |
| **worker** | One provider+model slot; at most one task at a time. |
| **worktree** | git worktree: isolated branch + checkout per task attempt. |
| **TF_\*** | Env vars controlling repo/state/config locations (kept for drop-in compat, chapter 7). |
