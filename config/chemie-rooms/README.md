# chemie-rooms — Model definitions & campaign

Validated worker/model definitions for the chemie-lernen.org campaigns
(via the local litellm gateway `http://localhost:4000/v1`).

## Verified model availability (2026-10-28, endpoint localhost:4000)

| model                          | auth key        | status                          |
| ------------------------------ | --------------- | ------------------------------- |
| `tud/mistral-medium-3.5-128b`  | MASTER          | ✅ works, tools ok (agent loop verified) |
| `tud/ministral-3-14b-instruct` | MASTER          | ✅ works                         |
| `tud/gemma-4-31b-it`           | MASTER **and** VIRTUAL | ✅ works                 |
| `deepseek-v4-flash/local`      | VIRTUAL only    | ✅ works (master → empty content)|
| `tud/glm-5.2-awq`              | both            | ⚠️ silent (empty content)       |
| `saia/qwen3-coder-30b-a3b-instruct`, `saia/devstral-2-123b-instruct-2512`, `saia/qwen3.5-27b`, … | — | ❌ "No deployments available" / "Unknown API key" (saia/* backends down) |

- MASTER key: `LITELLM_MASTER_KEY` in `/opt/litellm-compose/.env` (51 chars,
  works as Bearer for `tud/*`).
- VIRTUAL key: `LITELLM_API_KEY` in `/opt/litellm/pipeline.env` (25 chars,
  `sk-…`; bound to `deepseek-v4-flash/local` + `tud/gemma-4-31b-it`).
- Cluster gateway `192.168.42.20:4000` is a DIFFERENT litellm instance: it
  rejects the pipeline key and serves its own ids (`local/deepseek-v4-flash/local`,
  `saia/qwen3-coder-next`, `tud/glm-5.2-awq`) — used by taskfleet workers, not af.

## Worker schema gotchas (af 0.1.0)

- Field is **`api_base`** (OpenAI-compatible `/v1` URL), not `endpoint`.
  `cli: "builtin"` REQUIRES `api_base` (validate fails otherwise).
- `cli: "builtin"` = native harness (bash/write/edit + turn cap `max_turns`);
  default `cli` is `"pi"` (external coding-agent CLI, `--provider/--model/-p @file`).
- `api_key_env` names the env var af reads; af passes ONLY that key to the
  agent child. Set the value in the launcher env (keys are never in git).
- `price_per_mtok_usd` / `params_b` give `af cost` a basis; without it you get
  a "no cost basis" warning and neutral cost estimates.
- End-to-end verified 2026-10-28: full loop (dispatch → agent edits →
  `accept` gate → merge to main) works with `tud/mistral-medium-3.5-128b`
  via `cli: "builtin"` + litellm api_base + MASTER key.

## Campaign: merge discipline

Each task merge to `main` auto-triggers the chemie-lernen.org GH Actions
deploy (≈7 min, incl. Chat-API docker image — that image build has flaked
once with a buildx EOF; fix = `gh run rerun`, no code change).

Run:

```bash
export TF_REPO_DIR=/opt/git/hugo-chemie-lernen-org
export TF_STATE_DIR=/opt/git/taskfleet/state-chemie-rooms
mkdir -p "$TF_STATE_DIR"
set -a; source /opt/litellm-compose/.env; set +a   # LITELLM_MASTER_KEY
/usr/local/bin/af validate --tasks config/chemie-rooms/tasks.json --workers config/chemie-rooms/workers.json
nohup /usr/local/bin/af run --tasks config/chemie-rooms/tasks.json \
  --workers config/chemie-rooms/workers.json >"$TF_STATE_DIR/af.log" 2>&1 &
echo $! > /tmp/chemie-rooms-af.pid
```
