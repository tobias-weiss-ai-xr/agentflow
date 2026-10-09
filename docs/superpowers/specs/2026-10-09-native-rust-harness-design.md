# Native Rust Agent Harness (`cli: "builtin"`)

**Date:** 2026-10-09
**Status:** draft — awaiting operator approval
**Amends:** ADR-1 (thin orchestrator), constraint "no agent loop" — narrowly, for one worker mode

---

## 1. Problem

`af` delegates the entire agent loop to an external CLI (`pi`, `opencode`). That
makes every fleet host need a node-based agent install (heavy, version-fragile)
and hides the loop from the orchestrator: no turn caps, no per-tool-call
logging, no first-class token accounting without CLI-specific transcript
parsing (`output: "json"` knows pi's transcript shape only).

## 2. Decision

Build the harness natively in `af` as **one new worker mode**, alongside the
existing CLI modes. A worker opts in with `"cli": "builtin"`. Everything else
(prompts, worktrees, gates, receipts, router, recovery) is reused unchanged.

Research basis (2026-10): existing Rust harnesses (rig-core 0.44, genai 0.6)
are async-runtime-first and their abstractions collide with af's
thread-based thin design; all af worker targets (litellm, zai, vLLM) speak ONE
wire format — OpenAI `POST /chat/completions` with `tools`. The loop is small;
owning it is cheaper than adapting a framework to a single known protocol.

## 3. Architecture

One new module `src/harness.rs` (~450 lines + tests), called from
`execute_task` where the CLI subprocess would run:

```
execute_task (unchanged)
  └─ worker.cli == "builtin"
       └─ harness::run(worker, prompt, worktree, budget) -> HarnessOutput
            loop (≤ max_turns):
              POST {api_base}/chat/completions   (ureq, rustls)
              ├─ tool_calls? → execute each in worktree, append results
              └─ text only   → done, return
```

- `HarnessOutput { final_text, total_tokens, turns_used, stop: Normal|TurnCap|Timeout|ProviderError }`
- `execute_task` routes native failures through an early-return branch that
  calls the SAME preserve-and-note + cleanup machinery, with the receipt
  `error` carrying the precise reason (`harness: turn cap N reached`,
  `harness: exceeded agent_timeout_s`, `harness: provider …`). Work is
  preserved/archived unchanged; the legacy `agent exited …` CLI message
  texts stay byte-identical for CLI workers.
- After the loop, af's existing dirty-worktree commit + zero-commit failure +
  scope check + gate run unchanged — the harness changes nothing downstream.

## 4. Worker config (additive, zero migration)

```json
{
  "name": "af-native",
  "provider": "litellm",
  "model": "deepseek-v4-flash",
  "api_base": "http://ai1:8000/v1",
  "api_key_env": "LITELLM_KEY",
  "cli": "builtin",
  "max_turns": 32,
  "enabled": true
}
```

- `cli: "builtin"` selects the harness. `command`/`args`/`output` are ignored
  for it (validated: error if combined with `command`).
- `api_base` required for builtin (no global default — explicit beats implicit
  at 3am). `api_key_env` optional (keyless local vLLM allowed).
- `max_turns` per worker, default 32. Env override `TF_AGENT_MAX_TURNS`.
- `api_base` may be http (lan) or https; URL join is `{base}/chat/completions`.

## 5. Wire protocol

One request shape, sent with `Authorization: Bearer $<api_key_env>` when set:

```json
{ "model": "...", "messages": [...], "tools": [bash, write, edit], "tool_choice": "auto" }
```

- messages: system (= task prompt, the existing `render_prompt` output) →
  assistant(tool_calls) → tool(result) → … strictly alternating per OpenAI.
- usage per response is accumulated: `total_tokens` (+ prompt/completion split)
  lands on the receipt — first-class, no transcript parsing. `cost_micros`
  stays `None` (provider-reported cost stays a CLI/json-mode feature).
- Non-streaming. Batch work needs no SSE; per-tool-call logging provides
  liveness. HTTP read timeout = remaining wall budget (`agent_timeout_s`).

## 6. Tools (bash, write, edit)

| Tool | Semantics | Guard |
|---|---|---|
| `bash` | `{command}` → `bash -c` (or `cmd /C`) in worktree, allowlisted env | per-call timeout 300s, stdout/stderr truncated (16 KiB) into the tool result |
| `write` | `{path, content}` — create/overwrite | path must resolve inside the worktree (reject `..`/absolute escapes) |
| `edit` | `{path, old, new}` — replace first unique occurrence | error when `old` missing or not unique; no fuzzy matching |

`read` stays `cat` in bash (YAGNI). Tools are executed under
`EnvMode::Allowlist` via the existing `agent_env` (same allowlist as CLI
children today, plus the worker's `api_key_env`) — native mode is strictly
*tighter* sandboxing than CLI mode, because every side effect passes through af.

## 7. Observability & receipts

Each tool call appends to the attempt log: the call (`$ <command>` /
`write <path>` / `edit <path>`), exit code / byte counts, truncated output —
so a stalled attempt is diagnosable from `af attach`/logs without a TUI.
Receipts: `wall_clock_s`, `tokens` (measured sum), `outcome`, `error` exactly
as today; `af cost` picks tokens up without changes.

## 8. Failure & recovery semantics (all reused)

TurnCap/Timeout/ProviderError route through the same failure path as a CLI
child (branch archived as `<branch>-rejected-<ts>`, receipt `error` carries
`harness: …` reason, retry memory includes it). Provider HTTP 4xx/5xx and
transport errors are `ProviderError`; malformed JSON responses are too
(logged with body head). A response with neither text nor tool_calls is
`ProviderError: empty response`.

## 9. Testing (mirrors existing pyramid; no live network)

- **Unit** (`src/harness.rs` #[cfg(test)]): URL join; message assembly;
  tool-dispatch table; write/edit path-escape rejection; edit uniqueness;
  usage accumulation; turn-cap stop.
- **Contract** (`tests/contract_config.rs`): `"cli": "builtin"` parses;
  `command`+`builtin` rejected; `max_turns` default/override.
- **E2E** (`tests/e2e_native_harness.rs`): stub HTTP server on a local
  `TcpListener` (canned chat-completions JSON, scriptable sequence:
  tool_call → tool_call → final) + scratch git repo, driving real
  `af run`: (a) merge-on-first-attempt happy path, (b) turn-cap archive,
  (c) provider-error archive + receipt, (d) gate fail after clean harness run.
- Existing 340 tests: untouched — default workers keep CLI semantics.

## 10. Docs

- **ADR-11** in `docs/arc42/09-architecture-decisions.md`: amends ADR-1 —
  the orchestrator may originate an agent loop, but only inside the
  `cli: "builtin"` worker mode; the CLI path remains the default contract.
- arc42 glossary: **Native harness** entry; 02-constraints wording updated to
  point at the exception.
- README: worker schema row + short "no external agent CLI" section.

## 11. Non-goals (explicit)

No streaming/SSE, no MCP, no embeddings, no multi-modal, no local inference,
no retries inside the harness (af's attempt/retry machinery owns retries),
no prompt caching. The three tools above are the whole surface until a
campaign proves otherwise.

## 12. Dependency delta

`Cargo.toml` += `ureq` (rustls TLS, default-features off), nothing else —
`serde`/`serde_json` already present. Binary stays a single static artifact.
Estimated code: ~450 lines harness + ~300 lines tests.
