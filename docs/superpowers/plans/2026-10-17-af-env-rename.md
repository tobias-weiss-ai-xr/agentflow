# AF_* Env Var Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename agentflow's public env vars from the taskfleet-era `TF_*` prefix to `AF_*`, reading `AF_*` first and falling back to legacy `TF_*` (with a one-time stderr deprecation warning) so no user breaks.

**Architecture:** One resolver in `config.rs` (`af_var`) does AF-first/TF-fallback + warn-once; all readers route through it (`Settings::from_env`, `main.rs` REPOS_JSON, `run.rs` NO_REUSE, `execute.rs` passthrough via the injected lookup). `gate.rs` *exports* both `AF_GATE_REPLAY` and legacy `TF_GATE_REPLAY` so existing gate scripts keep working. Tests move to `AF_*`; dedicated tests pin the legacy fallback + warning.

**Tech Stack:** Rust 2021, cargo fmt/clippy/test; openspec specs; markdown docs.

---

## Variable inventory (runtime readers)

| AF_* (canonical) | legacy | read in |
|---|---|---|
| AF_STATE_DIR | TF_STATE_DIR | config.rs `Settings::from_env` |
| AF_REPO_DIR | TF_REPO_DIR | config.rs |
| AF_WORKTREE_ROOT | TF_WORKTREE_ROOT | config.rs |
| AF_MAX_PARALLEL | TF_MAX_PARALLEL | config.rs |
| AF_BRANCH_PREFIX | TF_BRANCH_PREFIX | config.rs |
| AF_POLL | TF_POLL | config.rs |
| AF_GATE_ENV | TF_GATE_ENV | config.rs |
| AF_TASKS_JSON | TF_TASKS_JSON | config.rs |
| AF_WORKERS_JSON | TF_WORKERS_JSON | config.rs |
| AF_AGENT_TIMEOUT_S | TF_AGENT_TIMEOUT_S | config.rs |
| AF_AGENT_STALL_S | TF_AGENT_STALL_S | config.rs |
| AF_AGENT_MAX_TURNS | TF_AGENT_MAX_TURNS | config.rs |
| AF_MAX_WALL_CLOCK_S | TF_MAX_WALL_CLOCK_S | config.rs |
| AF_SANDBOX_CMD | TF_SANDBOX_CMD | config.rs |
| AF_REPOS_JSON | TF_REPOS_JSON | main.rs |
| AF_NO_REUSE | TF_NO_REUSE | run.rs |
| AF_AGENT_ENV_PASSTHROUGH | TF_AGENT_ENV_PASSTHROUGH | execute.rs `agent_env` |
| AF_GATE_REPLAY (exported) | TF_GATE_REPLAY (exported) | gate.rs |

Not renamed: branch prefix default `"tf"`, `TF_LOCAL_*` scratch vars in `docs/moe-sovereign-ideas.md` (not af env vars), taskfleet references in `docs/arc42/02-constraints.md` prose (only the af var names change).

---

### Task 1: config.rs resolver + Settings::from_env

**Files:**
- Modify: `src/config.rs` (env helpers ~L580-618, module doc L1, doc comments L103/172/251/541/556/564/567/576, parse_gate_env doc L717)

- [ ] **Step 1: Add `af_var` / `env_fallback` / `warn_legacy_env` and switch helpers**

Replace `env_or`/`env_or_int` bodies and add:

```rust
/// Resolve an env name with the AF_* → legacy TF_* fallback rule.
///
/// `AF_<suffix>` wins when set; only when it is ABSENT is `TF_<suffix>`
/// consulted, and then a deprecation notice is printed to stderr at most
/// once per legacy name per process. A `name` without the `AF_` prefix has
/// no legacy twin and is looked up literally. `lookup` is the env accessor
/// (injected so `agent_env` can be exercised with a fake environment).
pub(crate) fn env_fallback(
    name: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(v) = lookup(name) {
        return Some(v);
    }
    let legacy = format!("TF_{}", name.strip_prefix("AF_")?);
    let v = lookup(&legacy)?;
    warn_legacy_env(&legacy, name);
    Some(v)
}

/// Read an `AF_*` variable from the process environment, with the legacy
/// `TF_*` fallback described on [`env_fallback`].
pub fn af_var(name: &str) -> Option<String> {
    env_fallback(name, &|k| std::env::var(k).ok())
}

/// One-shot deprecation notice for a legacy `TF_*` variable.
fn warn_legacy_env(legacy: &str, af: &str) {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut warned = warned.lock().unwrap_or_else(|p| p.into_inner());
    if warned.insert(legacy.to_string()) {
        eprintln!("warning: {legacy} is deprecated; use {af} instead");
    }
}

fn env_or(key: &str, default: &str) -> String {
    af_var(key).unwrap_or_else(|| default.to_string())
}

fn env_or_int(key: &str, default: u64) -> u64 {
    af_var(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}
```

Add `use std::sync::{Mutex, OnceLock};` at the top.

- [ ] **Step 2: `Settings::from_env` keys → AF_**

Every `env_or("TF_...", ...)` / `env_or_int("TF_...", ...)` becomes the `AF_` twin (see inventory table).

- [ ] **Step 3: Doc/comment sweep in config.rs** — `TF_*` → `AF_*` in the module doc, `Settings` field docs, `parse_gate_env` doc. Note legacy acceptance where useful.

- [ ] **Step 4: Inline unit tests**

Add to `mod tests`: `af_env_fallback_prefers_af_then_legacy` using `EnvVar`-style set/remove of `AF_CONTRACT_UNIT_X` / `TF_CONTRACT_UNIT_X` (verify AF wins, legacy used when AF absent, None when both absent) and `env_fallback_warns_via_injected_lookup` (covers `warn_legacy_env` with a fake lookup).

- [ ] **Step 5:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --lib`
- [ ] **Step 6: Commit** `refactor(config): read AF_* env vars with TF_* legacy fallback`

---

### Task 2: remaining readers + messages

**Files:** `src/main.rs`, `src/run.rs`, `src/execute.rs`, `src/gate.rs`, `src/harness.rs`

- `main.rs`: help `ENV:` block → `AF_*` (+ note legacy accepted); `std::env::var("TF_REPOS_JSON")` → `config::af_var("AF_REPOS_JSON")`; comment.
- `run.rs`: `reuse_disabled()` → `config::af_var("AF_NO_REUSE")` with the same `matches!` set; budget log message `TF_MAX_WALL_CLOCK_S` → `AF_MAX_WALL_CLOCK_S`; doc comments.
- `execute.rs`: `agent_env` → `crate::config::env_fallback("AF_AGENT_ENV_PASSTHROUGH", lookup)`; stall message `TF_AGENT_STALL_S` → `AF_AGENT_STALL_S`; test map key `TF_` → `AF_` and a legacy-fallback case.
- `gate.rs`: push BOTH `AF_GATE_REPLAY` and `TF_GATE_REPLAY` (legacy twin for existing gate scripts); comments.
- `harness.rs`: doc comment `TF_AGENT_MAX_TURNS` → `AF_AGENT_MAX_TURNS`.
- [ ] Verify: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --all-targets`
- [ ] **Commit** `refactor(env): AF_* names in main/run/execute/gate/harness; keep TF_GATE_REPLAY twin`

---

### Task 3: tests

- Rename `TF_`→`AF_` for af-read vars across: `tests/{budget,cli,contract_config,contract_env,contract_gate,e2e,gate_replay,journal,lock,scope_enforcement,token_cost,wakeup,watchdog,worker_args}.rs`. `cli.rs::af_env` helper and `("TF_NO_REUSE","1")` extras → AF_.
- `contract_gate.rs` / `gate_replay.rs`: assert PRIMARY `AF_GATE_REPLAY`, keep a legacy assertion that `TF_GATE_REPLAY` is still exported.
- New: `contract_config.rs::settings_env_af_prefers_af_over_legacy_tf` (both set → AF value; only TF set → TF value), and `cli.rs::legacy_tf_env_warns_once_on_stderr` (spawn `af status` with `AF_*` removed + `TF_STATE_DIR` set → exactly one `... is deprecated; use AF_STATE_DIR ...` line on stderr).
- [ ] Verify: `cargo test --all-targets`
- [ ] **Commit** `test(env): move to AF_*; pin legacy fallback + deprecation warning`

---

### Task 4: docs & specs

- README.md: env table/prose `TF_` → `AF_`; add one line: legacy `TF_*` still read but deprecated (warn once).
- `openspec/specs/{config,cli,lifecycle,sandbox,worktree}/spec.md`: rename to `AF_*`; config "Environment overrides" requirement states AF-first/TF-fallback + warning; lifecycle gate-replay requirement says `AF_GATE_REPLAY` (legacy `TF_GATE_REPLAY` still exported).
- `docs/arc42/*.md`, `docs/multi-repo-design.md`, `docs/af-usage-feedback.md`, `docs/pi-durable-ideas.rst`, real-var references in `docs/moe-sovereign-ideas.md`.
- Left as history (unchanged): `openspec/changes/archive/**`, `docs/superpowers/plans|specs/**`, `config/dogfood*.json`.
- [ ] Verify: `openspec validate --all` (if it accepts direct spec edits), grep shows no `TF_` in active docs except intentional legacy notes.
- [ ] **Commit** `docs: rename TF_* to AF_* across README, specs, arc42`

---

## Verification (final)

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets`
- Grep: only intended `TF_` remain (legacy fallback code/tests, legacy notes, history files, `TF_LOCAL_*` scratch vars).
