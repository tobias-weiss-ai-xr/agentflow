# Proposal: agent-sandboxing

## Why

`af` spawns an untrusted process (the agent CLI executes LLM-directed tool
calls). Today that child inherits:

- the orchestrator's **full environment** — including every worker's API key
  (`api_key_env` values for all workers, not just the dispatched one) and
  whatever else lives in the shell (`TF_GATE_ENV` secrets, machine tokens);
- a worktree with the repo's `origin` remote and git credential helpers
  available (a prompt-injected or hallucinating agent can attempt `git push`
  or interactive credential prompts that hang the attempt);
- no seam for OS-level containment at all.

## What Changes

Three portable layers, zero new dependencies:

1. **Env allowlist (default-on).** The agent child starts with an *empty*
   environment and receives only: system basics (PATH, HOME/USERPROFILE, temp
   dirs, …), the **dispatched worker's** `api_key_env`, and an explicit
   `TF_AGENT_ENV_PASSTHROUGH` list. Gates and git keep full env (trusted,
   user/authored by af).
2. **Git hygiene (default-on).** Agent children get `GIT_TERMINAL_PROMPT=0`
   and an empty `credential.helper` via `GIT_CONFIG_*` env — no credential
   popups, no hangs on auth prompts.
3. **Sandbox wrapper hook (opt-in).** `TF_SANDBOX_CMD` ("firejail --net=none",
   "bwrap --unshare-net-all …") is whitespace-split and prepended to the agent
   argv. Real filesystem/network containment is platform-specific; af provides
   the seam and documents recipes instead of embedding an OS sandbox.

## Capabilities

### Modified
- `lifecycle` — the agent spawn step runs under the sandbox policy.

### Added
- `sandbox` — env allowlist, git hygiene, wrapper hook, and their limits.

## Impact

- `src/subprocess.rs`: `EnvMode` (Inherit | Allowlist) on `run`.
- `src/execute.rs`: `agent_env()` policy builder; sandbox argv prefix.
- `src/config.rs`: `Worker.api_key_env`, `Settings.sandbox_cmd`.
- `src/bin/example_agent.rs`: `FAKE_AGENT_ENV` probe for tests.
- Docs: ADR-10, arc42 ch. 8, README security note.
- Backwards compatible: no config changes required; behavior only tightens
  for agent children (call `TF_AGENT_ENV_PASSTHROUGH` if the agent CLI needs
  more env).
