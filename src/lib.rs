//! agentflow — parallel LLM task execution on isolated git worktrees.
//!
//! The orchestrator stays thin: it drives an OpenAI-compatible *agent CLI*
//! (`pi` or any `--provider/--model/-p @file` CLI) and the `git` CLI as
//! subprocesses. It never talks to LLM providers directly (ADR-1).
//!
//! See `docs/arc42/` for the architecture documentation.

pub mod config;
pub mod execute;
pub mod gate;
pub mod router;
pub mod run;
pub mod scheduler;
pub mod state;
pub mod subprocess;
pub mod worktree;

pub use config::{Config, Settings, Task, TaskState, Worker, WorkerDefaults};
pub use state::{Receipt, StateStore, Store, TaskStatus};
