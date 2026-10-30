# wo-loop — autonomous World Office parity loop

Replaces the two manually-run mechanisms on legion:
- the disabled `wo-agent.service`/pi vision loop (degenerated into 176 no-op
  cycles — no gates, no census linkage, and it `git reset --hard`s).
- the manual one-shot `af run` rounds (`/tmp/af-wo-s10..s16`, `deep1`) where a
  human hand-wrote `tasks.json` every round and relaunched af by hand.

`wo-loop` turns the **census gap queue** into af batches, runs them, records
outcomes, and advances — forever, until the queue is drained. af itself does
the hard work (worktrees, scope enforcement, gates, merge/reject + archived
branches), so the loop stays tiny.

## How it works

```
wo-loop.service (systemd, Restart=on-failure)
└─ wo-loop.sh:
   1. record  — read receipts, mark slug parked(merged) / shelved(2×failed)
   2. generate— wo-gen-round.py --batch N → tasks.json (fresh WO-<batch>-<slug> ids)
   3. run     — af run --poll 30 --tasks … --workers … (hot-reload aware)
   4. bump N, sleep 10, repeat
```

- **Fresh ids per batch** — a terminal-failed task from a previous batch never
  blocks the rerun (af won't retry a failed task in the same id).
- **Shelving after 2 failed batches** — a persistently failing slug is parked
  (with a reason in `queue-state.json`) so one hard row cannot stall the whole
  port. `WO_SHELVE_AFTER` controls the threshold.
- **Batched** — `WO_BATCH_LIMIT` (default 3) tasks per batch keeps cost and
  deadlock blast radius bounded.

## Install / manage

```sh
sudo install -m 0755 wo-loop.sh wo-gen-round.py /home/weiss/.wo-loop/
sudo cp wo-loop.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now wo-loop
```

- Watch: `journalctl -u wo-loop -f` or `tail -f /home/weiss/.wo-loop/run.log`
- Stop gracefully: `af stop` (drains; loop exits) or `sudo systemctl stop wo-loop`
- Per-slug state: `/home/weiss/.wo-loop/queue-state.json`
  (`parked:true` + reason, or `editor:"apps/web/apps/documenteditor-react"` to
  point a slug at a different editor tree)

## Queue source

Seeds are parsed from the census ledger-gap report
(`wo-test-harness/census/census/ledger-gap.md`). To add work to the loop:

1. add a row to ledger-gap.md (`### slug — description` + `spec:`/`first step:`),
2. `python3 wo-gen-round.py --enqueue-all` (won't clobber existing state).

The task gate requires the agent to author `<slug>.test.mjs` (its established
zero-dep node-test style) plus a set of existing regression tests in the same
editor dir, then runs biome. Task scope is the editor tree; set a per-slug
`editor` override in queue-state.json when a row truly lives elsewhere.

## Failure modes observed (deep1) and how the loop addresses them

| deep1 failure | loop mitigation |
|---|---|
| all tasks × 6 attempts deadlocked → manual restart | fresh ids per batch; shelving; batch limit |
| turn cap 96/128 per task too small for huge specs | `max_turns: 64` per generated task (bounded, retries get fresh budget) |
| manually re-generating tasks.json per round | `wo-gen-round.py` does it from the census queue |
| pi vision loop = no-op drift / hard resets | af worktrees + gates + merge only on pass; no `reset --hard` |
