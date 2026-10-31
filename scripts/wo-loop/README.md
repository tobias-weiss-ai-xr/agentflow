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
2. `python3 wo-gen-round.py --enqueue-all` (won't clobber existing state;
   rows titled `census-artifact`/`intentional-divergence` are skipped).

### Full reseed after a batch of merges (census re-score)

After the loop merges features, the honest refill is re-running the census
pipeline so the advisory re-ranks what is *still* missing:

```
scp scripts/wo-loop/reseed-queue.py legion:/tmp/
wsl ssh weiss@legion 'cd …/wo-test-harness/census && python3 /tmp/reseed-queue.py'
# advisory (needs VISION_* env; see notes below) then:
wsl ssh weiss@legion 'bash /tmp/advisory3.sh'   # deepseek advisory -> ledger-gap.md
python3 wo-gen-round.py --enqueue-all && sudo systemctl start wo-loop
```

Notes learned the hard way (2026-10-31):

- **build.py writes `census/ledger/ledger.json`; ledger-gap.py reads
  `census/census/ledger.json`** — sync the copy (`advisory.sh` does).
- **Reasoning models cannot run the advisory**: glm-5.2, qwen3.5, even
  mistral-medium burn the whole `max_tokens` on `reasoning_content` and
  return `content=None` (`'NoneType' object has no attribute 'strip'`).
  The **local deepseek-v4-flash via litellm `saia/deepseek-v4-flash-0731`**
  works (7 s, clean JSON, `--chunk 8`).
- `/etc/environment` is root-only on legion — read the key with
  `sudo awk`.
- Never leave a file named `inspect.py` in `/tmp` — running a script from
  `/tmp` puts it first on `sys.path` and shadows stdlib `import inspect`.
- The re-score after 18 merges: L0 44 -> 22, interactions clear; the
  advisory reclassified most shipped slugs as `census-artifact` and
  surfaced 9 new actionable ones.

The task gate requires the agent to author `<slug>.test.mjs` (its established
zero-dep node-test style) plus a set of existing regression tests in the same
editor dir, then runs biome. Task scope is the editor tree; set a per-slug
`editor` override in queue-state.json when a row truly lives elsewhere.

## Failure modes observed (deep1) and how the loop addresses them

| deep1 failure | loop mitigation |
|---|---|
| all tasks × 6 attempts deadlocked → manual restart | fresh ids per batch; shelving; batch limit |
| turn cap 96/128 per task too small for huge specs | `max_turns: 96` per generated task (bounded, retries get fresh budget) |
| manually re-generating tasks.json per round | `wo-gen-round.py` does it from the census queue |
| pi vision loop = no-op drift / hard resets | af worktrees + gates + merge only on pass; no `reset --hard` |

## Operational lessons (first live batch, 2026-10-30)

- **The gate runs with cwd = the task worktree**, NOT the main repo. Never
  `cd /home/weiss/git/World-Office/server` in the gate — the agent's committed
  work lives in `state/worktrees/<task>`, an un-merged worktree is invisible
  from the main checkout (every gate failed with `missing <slug>.test.mjs`
  until this was fixed).
- **Biome must check agent-ADDED files only.** The whole editor dir has ~260
  pre-existing errors and `editor.js` itself has pre-existing violations
  (`"use strict"` at line 36), so a dir-wide or full-file biome check can
  never pass for any agent. Added files are fully agent-authored → must be
  clean; modified files get `node --check` only.
- **Git worktrees have no `node_modules`** — reference
  `.../server/node_modules/.bin/biome` by absolute path inside the gate.
- **A dirty main checkout kills every archive-reuse merge** ("your local
  changes would be overwritten"). Under the first (broken) gate, agents
  `cd`'d to the base repo path themselves and leaked files into main
  (incl. an untracked `content-controls.test.mjs`). Fix: park the leaked work
  on a `tf/parked-af-loop-<ts>` branch, restore main clean, prune scratch
  worktrees.
- **`af stop` drains and returns 3** → `wo-loop.sh` treats exit 3 as "stop
  requested" and exits cleanly (systemd Restart=on-failure keeps it up).

## Troubleshooting

- Watch: `journalctl -u wo-loop -f` or `tail -f /home/weiss/.wo-loop/run.log`
- Receipts: `/home/weiss/.wo-loop/state/receipts/*.json` (outcome,
  failure_reason)
- Base repo never dirty: verify `git -C /home/weiss/git/World-Office/server
  status --porcelain` is empty; if not, park it (see lessons above).
- Simulate a gate from the worktree contract before editing the template:
  `git worktree add /tmp/x -b sim main` → author test+change → run gate from
  `/tmp/x`.
