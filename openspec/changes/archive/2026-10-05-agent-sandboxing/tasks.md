# Tasks: agent-sandboxing

## 1. Subprocess env policy
- [x] 1.1 `EnvMode { Inherit, Allowlist }` in `subprocess.rs`; `run()` applies it
- [x] 1.2 Update trusted callers (git, gate) to `Inherit`

## 2. Sandbox policy builder
- [x] 2.1 `Worker.api_key_env` field (serde default)
- [x] 2.2 `execute::agent_env(worker, lookup)` → (pairs, allowlist); system basics, api key, passthrough, git hygiene pairs
- [x] 2.3 `Settings.sandbox_cmd` from `TF_SANDBOX_CMD`; argv prefix in `execute_task`
- [x] 2.4 Unit tests: policy contents (pure fn with injected lookup), wrapper argv

## 3. Verification
- [x] 3.1 `example_agent` `FAKE_AGENT_ENV` probe
- [x] 3.2 E2E: foreign secret invisible, api key + passthrough + PATH visible
- [x] 3.3 Full suite green, corpus re-check, zero warnings

## 4. Docs
- [x] 4.1 ADR-10 (env allowlist + wrapper seam, no embedded OS sandbox)
- [x] 4.2 arc42 ch. 8 sandboxing concept (threat model, recipes, limits)
- [x] 4.3 README security note + env var reference
