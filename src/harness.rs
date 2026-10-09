//! Native agent harness (`cli: "builtin"`): af owns the LLM loop.
//! ADR-11 amends ADR-1 narrowly: the orchestrator may originate an agent
//! loop, but only inside this worker mode. CLI workers keep the ADR-1
//! subprocess contract unchanged.

//! Implemented in Tasks 2-3 of docs/superpowers/plans/2026-10-09-native-rust-harness.md.
