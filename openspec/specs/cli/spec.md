# cli Specification

## Purpose
TBD - created by archiving change rust-orchestrator. Update Purpose after archive.

## Requirements

### Requirement: Run commands

`af run` SHALL run the dispatch loop until all tasks are done or deadlock, honoring `--once` (one dispatch round), `--dry-run` (show plan, change nothing), `--worker <name>`, `--task <id>`, and `--poll <secs>`.

#### Scenario: dry run changes nothing

WHEN `af run --dry-run` runs against a config with pending tasks
THEN it prints the dispatch plan, creates no worktrees, and exits 0.

#### Scenario: full run completes

WHEN `af run` runs against a config whose tasks all pass
THEN all tasks reach `done` and the process exits 0.

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
and trust rate (wins ÷ attempts, two decimals), computed from receipt
outcomes that are VERDICTS on the worker — `interrupted` receipts are
excluded from both the numerator and the denominator because they carry no
agent or gate result. `af cost` SHALL also report wasted spend — the total
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
the task table, immediately after TRUST in the worker table): `$<usd>`
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

#### Scenario: Receipts naming a worker absent from the config are footnoted

GIVEN receipts naming workers that are not in `cfg.workers` alongside receipts for a configured worker
WHEN `af cost` runs
THEN it exits 0, shows `-` for the absent workers' rows, and prints one footnote line after the tables listing the absent names (sorted, deduplicated) with the attempt count.
