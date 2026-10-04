# 2. Constraints

## Technical

| Constraint | Value |
|------------|-------|
| Language / edition | Rust 2021+ (toolchain: cargo/rustc 1.92) |
| Async runtime | `tokio` |
| Deliverable | Single static binary `af` (plus crate `agentflow`) |
| LLM interaction | **Subprocess** to an OpenAI-compatible agent CLI (`pi --provider X --model Y -p @prompt`). No direct HTTP to LLM providers. *(ADR-1)* |
| Git interaction | `git` CLI as subprocess. No libgit2. |
| Config | JSON via `serde`; `tasks.json` / `workers.json` schema-compatible with taskfleet's shipped examples |
| State | JSON files under `state/`, atomic writes, single writer process *(ADR-3)* |

## Organizational / Process

| Constraint | Value |
|------------|-------|
| Repo visibility | agentflow = **public** (GitHub, MIT); taskfleet = **private** internal *(ADR-8)* |
| Development workflow | **Spec → Contract → Test pyramid** for every change *(ADR-5)*: OpenSpec spec first, executable contract tests, then unit/integration/E2E pyramid |
| Documentation | arc42, one file per chapter in `docs/arc42/` *(ADR-7)* |
| No secrets | Public repo must never contain credentials, hosts, or internal campaign configs |

## Runtime Environment

- Requires on `PATH`: `git`, an OpenAI-compatible agent CLI (e.g. `pi`).
- Environment variables: `TF_*` family (see glossary) for repo/state/config locations.
- Runs on Linux, macOS, and Windows (WSL); CI uses GitHub Actions with Ubuntu.

## Non-negotiable Boundaries

- `af` orchestrates; it does not originate: no agent loop, no tool calling, no embedded git server.
- v1 does **not** include: Bayesian routing, episodic memory, trust/constitution, corrections, transparency, openspec bridge, vLLM worker, multi-repo. These are v2 candidates (referenced in 04).
