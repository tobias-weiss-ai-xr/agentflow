# Tasks

1. **config.rs**: add `max_turns: Option<u32>` and `readonly: bool` to `Task`
   (both `#[serde(default)]`, `readonly` defaulting false). Validation:
   `max_turns == Some(0)` hard error; `readonly && accept.is_some()` hard error.
2. **harness.rs**: 
   - `run()` gains `task_max_turns: Option<u32>`, `task_scope: &[String]`,
     `readonly: bool`; `effective_max_turns` resolves task > worker > env > 32.
   - Tool guard: `write`/`edit` resolve the path, reject when `readonly` or
     when scope is non-empty and `execute::scope_violations(&[rel], scope)` is
     non-empty, returning a message naming path + allowed scope. Reuse
     `execute::scope_violations`.
   - Contract tests: out-of-scope write/edit rejected; in-scope allowed;
     readonly rejects; task max_turns overrides worker.
3. **execute.rs**: pass task fields into `harness::run`; readonly completion
   path — normal agent stop reports done, skip gate + merge (both harness and
   CLI workers); uncommitted-work preservation unchanged.
4. **router.rs**: `pick()` exploitation mean = `(wins + 1)/(n + 2)`;
   `trust()` stays raw. Update UCB1 doc comments.
5. **Tests**: config validation (max_turns 0, readonly+accept); harness tool
   guard + turns precedence; router prior scenarios; execute readonly flow.
   Update any router/trust tests pinned to the raw mean.
6. **Docs**: README task schema rows (`max_turns`, `readonly`) + trust-routing
   paragraph (Laplace prior, `af cost` shows raw); arc42 §8 routing concept
   mention.
7. Full suite green (`cargo test --lib`, contract tests); commit + push.
8. Dogfood round 18: probe the new knobs with real tasks (readonly audit task,
   per-task max_turns, out-of-scope write fast-fail) using 3 workers.
