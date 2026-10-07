# cli Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

### Requirement: recover command

`af recover --task <id> [--dry-run]` SHALL re-validate an archived rejected
branch — the copy a rejected attempt leaves under `<prefix>/<id>-rejected-<ts>`
(with an optional `-<n>` collision suffix) — and merge it into the base
branch when it passes, WITHOUT ever re-invoking the agent. Selection SHALL
pick the NEWEST archived branch for the task by PARSING the numeric `<ts>`
(then the `<n>` collision suffix) and never by git output order or committer
dates, so the same branch set always selects the same branch. `af recover`
SHALL never push and SHALL leave the failed attempt's receipt untouched
(history is append-only).

Before any merge, `af recover` SHALL re-validate in this order: (a) SCOPE —
every path the archived branch changes relative to the merge base with the
base branch MUST be covered by the task's CURRENT `scope`, using the same
enforcement helpers and glob matcher the attempt path uses (an empty scope
means any file); (b) GATE — unless the task is `manual`, run the acceptance
gate on the checked-out archived branch exactly as the attempt path does
(including the task's `gate_replay`). The temporary worktree SHALL be removed
on every exit path. On success `af recover` SHALL merge the archived branch
into the base with the message `af: <id> — <title>`, set the task's state to
`Done` with phase `GatePassed`, persist it, remove the recovered worktree and
the archived branch, and print `✓ <id> recovered to done (agent not re-run)`.

Exit codes: `0` merged (or `--dry-run` reported); `1` re-validation failed
(scope violation or gate failure — the archived branch stays in place); `2`
unknown task, missing `--task ID`, or no archived branch to recover (nothing
to recover is not a failure). With `--dry-run`, `af recover` SHALL perform
the selection, report exactly what it would do, and exit `0` without touching
anything — no worktree, no gate run, no merge.

#### Scenario: recover merges an archived branch after revalidating scope and gate

GIVEN task `C` whose `scope` covers `WORK.txt` and whose gate passes when
`WORK.txt` exists, and an archived `tf/C-rejected-<ts>` branch carrying a
committed in-scope change that the gate accepts
WHEN `af recover --task C` runs
THEN it exits 0, the task is `Done` in the state file, the change is in the
base branch, and the archived branch is removed.

#### Scenario: out-of-scope branch fails and survives

GIVEN an archived branch whose change includes a file the task's CURRENT
scope does not cover
WHEN `af recover --task <id>` runs
THEN it exits 1, prints the out-of-scope file, and the archived branch
survives (no merge, no worktree left behind).

#### Scenario: gate-failing branch fails and survives

GIVEN an archived branch whose checked-out content fails the task's acceptance gate
WHEN `af recover --task <id>` runs
THEN it exits 1 and the archived branch survives (no merge, no worktree left behind).

#### Scenario: dry run reports without changing anything

GIVEN an archived branch for a task
WHEN `af recover --task <id> --dry-run` runs
THEN it exits 0, names the branch it would recover, and changes nothing
(no worktree, no gate run, no merge; the archived branch survives and the
task state is untouched).

#### Scenario: unknown task exits two

WHEN `af recover --task <unknown-id>` runs
THEN it exits 2 with `config error: unknown task '<unknown-id>'`.

#### Scenario: no archived branch exits two

GIVEN a task with no archived rejected branch
WHEN `af recover --task <id>` runs
THEN it exits 2 naming the `<prefix>/<id>-rejected-<ts>` pattern searched
(nothing to recover is not a failure).

### Requirement: Run commands

`af run` SHALL run the dispatch loop until all tasks are done or deadlock, honoring `--once` (one dispatch round), `--dry-run` (show plan, change nothing), `--worker <name>`, `--task <id>`, and `--poll <secs>`. Before dispatching a worker for a ready task, `af run` SHALL look for an archived rejected branch for that task and, when the branch's change is covered by the task's CURRENT `scope` and the task's CURRENT acceptance gate passes on it, merge it and mark the task `Done` WITHOUT invoking the agent (the pre-dispatch reuse required by `lifecycle`), printing a line naming the reused branch. This reuse is ON by default; setting `TF_NO_REUSE=1` disables it for a clean re-run. Reuse never fires for a task with no archive, an out-of-scope archive, a gate-failing archive, or a task already `Done`; those cases fall through to the normal agent dispatch.

#### Scenario: dry run changes nothing

WHEN `af run --dry-run` runs against a config with pending tasks
THEN it prints the dispatch plan, creates no worktrees, and exits 0.

#### Scenario: full run completes

WHEN `af run` runs against a config whose tasks all pass
THEN all tasks reach `done` and the process exits 0.

#### Scenario: pre-dispatch reuse never re-pays for archived work

GIVEN a ready task with an archived rejected branch whose change the task's
CURRENT `scope` covers and whose CURRENT gate passes on it
WHEN `af run` dispatches the task
THEN it reuses the archive (merge, `done`, archived branch deleted) without
invoking the agent, while `TF_NO_REUSE=1` disables that reuse.

### Requirement: Status and inspection commands

`af status` SHALL print a human-readable status board; `af api status [--json]` SHALL output machine-readable status; `af api results --task <id>` SHALL show gate output and result; `af attach <id>` SHALL tail a running task's live log.

#### Scenario: json status

WHEN `af api status --json` runs
THEN valid JSON with every task's state is printed to stdout.

#### Scenario: attach tails log

WHEN a task is running and `af attach <id>` runs
THEN it streams that task's log lines until the task finishes.

### Requirement: Cost report

`af cost` SHALL append a per-worker trust section: worker name, wins/total,
trust rate (wins ÷ attempts, two decimals), and MEAN_S — the mean
wall-clock seconds over the worker's VERDICT attempts, ONE decimal, `-`
when the worker has no verdict attempt — all computed from receipt
outcomes that are VERDICTS on the worker: `interrupted` receipts are
excluded from the numerator, the denominator, and the mean, because they
carry no agent or gate result and their duration is a placeholder, not a
measurement. MEAN_S is the same statistic the router reads as its last
tie-break, so the routing choice is inspectable from the report. `af cost` SHALL also report wasted spend — the total
wall-clock seconds and the attempt count of attempts whose outcome was not
`merged`, the wasted percentage of all selected attempts, and a breakdown
grouped by failure CAUSE (the text before the first `:` in the receipt's
`error`, trimmed, with runs of whitespace collapsed) — computed over the
SAME receipt selection as the rest of the report, so `--last`, `--since`,
and `--task` narrow the waste figures too. `af cost` SHALL additionally
report interrupted attempts distinctly, as their own `INTERRUPTED` outcome
line naming the attempt count, since their duration is unknown (recorded as
0.0s) — while keeping them out of the trust denominator. The table SHALL
include a TOKENS column summing the `tokens` recorded on the selected
receipts, showing `-` when none are recorded, and the report SHALL name
every unreadable receipt file (one warning line per file) without failing.

`af cost` SHALL also estimate EXPENSE, so a campaign whose providers report
no price can still see who is burning the budget. ONE basis line SHALL be
printed above the tables stating the basis in force: `declared prices
(USD per 1M tokens)` when any worker declares `price_per_mtok_usd` (with a
note when some workers declare only `params_b`), `params_b proxy (relative;
cheapest declared worker = 1.00x)` when any declares `params_b` and none a
price, or `none` otherwise — so a relative proxy can never be misread as
money. Both tables SHALL carry a COST column (immediately after TOKENS in
the task table, immediately after MEAN_S in the worker table): `$<usd>`
with four decimals from a declared price (real money, via the cost model's
`estimate_usd`), `N.NNx` with two decimals from the `params_b` proxy (the
rate relative to the cheapest declared `params_b` — never a `$`), or `-`
when the worker declares neither basis, is absent from the config, or the
receipts carry no tokens. A task row spanning several attempts SHALL sum
the dollars when every attempt is priced, use the token-weighted mean ratio
when every attempt is sized, and show `-` otherwise. Receipts naming a
worker absent from `cfg.workers` SHALL render as `-` and be listed in ONE
footnote line after the tables (attempt count plus the sorted, deduplicated
names) — a note, never an error, never blocking the report.

A receipt carrying the provider's OWN reported cost (`cost_micros`, integer
micro-USD) is a MEASUREMENT of real money and SHALL outrank every declared
basis for the attempt it belongs to — even on a worker that also declares a
price, and even on one declaring nothing. On a report where any selected
receipt carries a measured cost, both tables' COST cells SHALL be folded
attempt-by-attempt through that truthfulness ladder (measured dollars,
then price-estimated dollars, then the `params_b` ratio, else unknown),
keeping round 10's row invariants exactly: a row is in DOLLARS only when
every attempt in it can be expressed in dollars (measured or estimated) —
a measured dollar amount and a parameter RATIO are incommensurable, so a
measured+sized row stays `-` just as a price+sized row does — and a row is
a RATIO only when every attempt is sized. A dollars figure containing ANY
estimated term SHALL be marked `~` (for example `~$0.0123`) so an
assumption is never presented as a measurement, while a row of purely
measured costs carries no `~`; `-` still means unknown, never free. On
such a report the basis line SHALL name the measured source —
`cost basis: provider-reported (USD)` — appending how many of the selected
attempts were estimated from a declared price (for example `; 1 of 2
attempt(s) estimated from a declared price`). A report none of whose
selected receipts carries a measured cost SHALL keep the declared-basis
lines byte-for-byte.

#### Scenario: trust section lists each worker with history

GIVEN receipts exist for workers w1 (2 merged, 1 failed) and w2 (1 merged)
WHEN `af cost` runs
THEN the trust section shows `w1 2/3 0.67` and `w2 1/1 1.00`.

#### Scenario: wasted spend surfaces failed attempts

GIVEN receipts where 2 of 4 attempts failed (100.0s on one failure reason, 20.0s on another)
WHEN `af cost` runs
THEN the report shows the waste total `WASTED: 120.0s on 2 of 4 attempt(s) (50.0%)` and a by-reason breakdown naming each failure reason with its seconds, and `--last` narrows the waste figures to the same selected receipts.

#### Scenario: unreadable receipts are reported by the cost report

GIVEN the receipt directory holds valid receipts and one truncated `*.json` receipt
WHEN `af cost` runs
THEN the report names the truncated file and still reports the valid receipts' spend.

#### Scenario: wasted reasons group by cause not by file list

GIVEN two failed receipts whose `error` values share the text before the first `:` but list different files after it
WHEN `af cost` runs
THEN the breakdown shows ONE row for that cause whose seconds are the sum and whose count is both failures, with a key that is not truncated mid-word.

#### Scenario: cost report shows tokens when present

GIVEN receipts whose `tokens` are recorded and receipts whose `tokens` are absent
WHEN `af cost` runs
THEN the TOKENS column shows the summed tokens for the recorded receipts and `-` for the ones with no tokens.

#### Scenario: interrupted attempts are reported distinctly

GIVEN a receipt whose `outcome` is `interrupted` (an attempt lost when the orchestrator was killed mid-attempt)
WHEN `af cost` runs
THEN the report shows a distinct `INTERRUPTED` line for it and its worker's `WINS/TOTAL` is unchanged by it.

#### Scenario: Cost report shows relative expense when no provider reports a price

GIVEN two workers declaring different `params_b` (8 and 40) and receipts carrying `tokens`, one of which has no `tokens`
WHEN `af cost` runs
THEN one basis line above the tables says `cost basis: params_b proxy (relative; cheapest declared worker = 1.00x)`, the cheapest declaring worker shows `1.00x`, the bigger one shows its real ratio (`5.00x`), and the attempt whose receipt has no tokens shows `-` — and no `$` appears anywhere.

#### Scenario: A declared price is reported as dollars

GIVEN a worker declaring `price_per_mtok_usd` (and also `params_b`) with a receipt carrying `tokens`
WHEN `af cost` runs
THEN its COST cell shows `$<usd>` computed from its tokens with four decimals and never an `x` ratio, and the basis line says `declared prices` (noting any workers that declare only `params_b`).

#### Scenario: The cost report prefers a measured cost over a declared one

GIVEN a receipt carrying a provider-reported `cost_micros` on a worker that ALSO declares `price_per_mtok_usd`, a task row mixing that measured attempt with an estimated-priced attempt, and a task row mixing a measured attempt with a `params_b`-only attempt
WHEN `af cost` runs
THEN the measured attempt's COST cell shows its measured dollars with NO `~` (not the figure its declared price would estimate), the mixed measured+estimated row shows the summed dollars marked `~`, the measured+sized row stays `-`, and the basis line says `cost basis: provider-reported (USD)` naming how many attempts were estimated from a declared price.

#### Scenario: The cost report shows the duration the router reads

GIVEN receipts where worker w1 has verdict attempts of 2.0s and 6.0s plus one `interrupted` attempt of 999s, and worker w2 has one verdict attempt of 3.0s
WHEN `af cost` runs
THEN the worker table shows a MEAN_S column with `4.0` for w1 and `3.0` for w2 — the `interrupted` attempt is excluded from the mean exactly as it is from wins/total, and stays visible on its own INTERRUPTED line.

#### Scenario: Receipts naming a worker absent from the config are footnoted

GIVEN receipts naming workers that are not in `cfg.workers` alongside receipts for a configured worker
WHEN `af cost` runs
THEN it exits 0, shows `-` for the absent workers' rows, and prints one footnote line after the tables listing the absent names (sorted, deduplicated) with the attempt count.

#### Scenario: A declared basis is never blank

GIVEN workers that declare `params_b` or `price_per_mtok_usd` whose receipts record no `tokens` at all (the default output mode captures none)
WHEN `af cost` runs
THEN each declaring worker's COST cell still shows its declared rate — `N.NNx` relative to the cheapest declaring worker, or `$<price>/Mtok` for a declared price, unit-suffixed so a rate is never misread as a spend — because a declared rate is a property of the worker and not of the tokens that happened to be recorded; only a worker declaring neither basis shows `-`.

#### Scenario: The interrupted placeholder is not a missing worker

GIVEN an `interrupted` receipt whose `worker` is the placeholder `unknown` (the startup heal cannot know which worker a killed attempt was running)
WHEN `af cost` runs
THEN the report shows the `INTERRUPTED` line for that attempt but does NOT footnote `unknown` as a worker absent from the config, while a receipt naming a worker that genuinely is not in `cfg.workers` is still footnoted.
