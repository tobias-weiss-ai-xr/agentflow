# Self-improvement with agentflow + saia models

A reusable campaign harness that dogfoods agentflow against itself: saia/tud/local workers
(via the LiteLLM proxy, `cli: "builtin"`) edit this Rust repo, an acceptance gate (`make check`)
verifies each change, and agentflow merges accepted work to `main`.

## Layout

- `config/self-improve/workers.json` — worker set (`saia-qwen397b`, `tud-glm52`,
  `local-deepseek`). Gitignored (may embed keys).
- `config/self-improve/tasks.json` — the campaign tasks. Gitignored.
- `config/self-improve/launch.sh` — detached-run helper. Gitignored.
- `state-self-improve/` — runtime state (logs/prompts/receipts/worktrees/run-state). Gitignored.
- `docs/self-improve.md` — this runbook (committed).

`/config/self-improve/` and `/state-self-improve/` are in `.gitignore`; only this runbook is
committed.

## Requirements

- **WSL is required**: this host's Windows toolchain lacks `gcc`/`cl` and `cargo test` shows
  3 false failures, so code tasks must run in WSL where `make`/`cargo`/`git` are on PATH.
- **LiteLLM proxy reachable** at `192.168.42.42:4000` (or `42.20`) with `LITELLM_MASTER_KEY`
  exported in the WSL environment. `launch.sh` sources `C:\Users\Tobias\litellm-docker\.env`
  (via `/mnt/c/...`, CRLF-stripped) automatically when the key is not already set, so the key
  does not need to be typed manually.

## Run a campaign

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo build --release'
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && bash config/self-improve/launch.sh'
```

Note: `launch.sh` sources the proxy `.env` (CRLF-stripped) so the builtin-harness workers get
`LITELLM_MASTER_KEY` (plus SAIA/TUD/other keys). Run from a shell whose PATH has `make`/`cargo`/
`git` (WSL Ubuntu satisfies this).

Monitor:

```bash
AF_STATE_DIR=state-self-improve ./target/release/af status
AF_STATE_DIR=state-self-improve ./target/release/af status --json
AF_STATE_DIR=state-self-improve ./target/release/af cost
tail -f state-self-improve/af-run.log
```

## Editing tasks

`tasks.json` follows the standard schema (id, title, deps, scope, accept, acceptance_prose,
manual, priority). The `accept` gate must exit 0 for the work to merge. Keep `scope` a superset
of every file the task may touch, or agentflow rejects the attempt as out-of-scope.

## Safety

The pilot's first task merges the local `feat/verify-tasks` branch. A safety backup
(`backup/verify-tasks-<date>`) is created before the merge. For any local-only branch targeted by
a campaign, create a backup first:

```bash
git branch backup/<branch>-<date> <branch>
```
