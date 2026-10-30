#!/usr/bin/env python3
"""wo-gen-round.py — turn the parity gap queue into the next af task batch.

Reads the pending ledger-gap rows (parsed from ledger-gap.md) plus a small
per-slug state file and emits a tasks.json batch with FRESH ids
(WO-<BATCH>-<slug>) so terminal-failed state never blocks a rerun. A slug
is skipped when it is parked(done) or shelved(too many failed batches).

State: /home/weiss/.wo-loop/queue-state.json
  { "<slug>": {"batches_failed": int, "parked": bool, "reason": str} }

Gate: af runs `accept` verbatim. The template refers to
apps/web/apps/documenteditor-wysiwyg/ (the editor trio all prior WO parity
rounds touch); {SLUG} is substituted. The agent is instructed in the
acceptance prose to author <slug>.test.mjs — the gate requires it.

Usage:
  wo-gen-round.py --enqueue-all          # seed queue from ledger-gap.md
  wo-gen-round.py --batch N --out FILE   # write tasks.json for batch N
  wo-gen-round.py --dry-run
  wo-gen-round.py --record --state STATE_DIR  # mark done/shelve from receipts
"""
import argparse
import json
import os
import re
import sys
from pathlib import Path

REPO = "/home/weiss/git/World-Office/server"
EDITOR = "apps/web/apps/documenteditor-wysiwyg"
LEDGER = Path("/home/weiss/git/World-Office/wo-test-harness/census/census/ledger-gap.md")
STATE = Path(os.environ.get("WO_QUEUE_STATE", "/home/weiss/.wo-loop/queue-state.json"))
SHELVE_AFTER = int(os.environ.get("WO_SHELVE_AFTER", "2"))
# Max tasks generated per batch: the loop drains the queue a bounded slice
# per cycle so a single batch cannot stall the whole port on a peer task.
BATCH_LIMIT = int(os.environ.get("WO_BATCH_LIMIT", "3"))

# Editor tests known to exist and pass (regression net for every task).
REGRESSION = ["font-style", "view-styles", "paragraph-shading",
              "paste-sanitize", "comments-thread", "presence-avatar"]

# Gate runs with cwd = af's task worktree (git worktree — NO node_modules),
# so biome must be referenced by absolute path in the main checkout.
BIOME = f"{REPO}/node_modules/.bin/biome"

GATE_TEMPLATE = """#!/bin/sh
set -e
D={EDITOR}
[ -f "$D/{SLUG}.test.mjs" ] || {{ echo 'FAIL: missing {SLUG}.test.mjs'; exit 1; }}
node "$D/{SLUG}.test.mjs"
for t in {REGRESSION}; do node "$D/$t.test.mjs" >/dev/null 2>&1 || {{ echo "FAIL: $t.test.mjs"; exit 1; }}; echo "ok $t"; done
{BIOME} check "$D" 2>&1 | tail -3
echo '{SLUG}-PASS'
"""


def parse_ledger(path: Path):
    """ledger-gap.md rows -> list of {slug, title, priority, spec, first_step}."""
    rows = []
    cur = None
    for line in path.read_text(encoding="utf-8").splitlines():
        m = re.match(r"^### (\S+) — (.+)$", line)
        if m:
            if cur:
                rows.append(cur)
            cur = {"slug": m.group(1), "title": m.group(2).strip(),
                   "priority": "P?", "spec": "", "first_step": ""}
            continue
        if cur is None:
            continue
        pm = re.match(r"^- (value|effort|spec|first step):?(.*)$", line)
        if pm:
            key, val = pm.group(1), pm.group(2).strip()
            if key == "value":
                cur["priority"] = val
            elif key == "spec":
                cur["spec"] = val
            elif key == "first step":
                cur["first_step"] = val
    if cur:
        rows.append(cur)
    return rows


def load_state() -> dict:
    if STATE.exists():
        return json.loads(STATE.read_text(encoding="utf-8"))
    return {}


def save_state(s):
    STATE.parent.mkdir(parents=True, exist_ok=True)
    STATE.write_text(json.dumps(s, indent=2), encoding="utf-8")


def shelve_reason(row, st):
    if st.get("parked"):
        return "parked"
    if st.get("batches_failed", 0) >= SHELVE_AFTER:
        return f"shelved after {st['batches_failed']} failed batches"
    return None


def build_task(row, batch):
    slug = row["slug"]
    spec = (row["first_step"] or row["spec"]).strip()
    # Fresh id per batch: terminal-failed task state from an earlier batch
    # can never block this rerun.
    tid = f"WO-{batch}-{slug}"
    # Per-slug editor override in queue-state ({"<slug>": {"editor": "..."}}),
    # defaulting to the vanilla wysiwyg trio where all prior parity work
    # landed. Set it manually for a row that genuinely lives in another tree.
    st = load_state().get(slug, {})
    editor = st.get("editor") or EDITOR
    gate = GATE_TEMPLATE.format(REPO=REPO, EDITOR=editor, SLUG=slug,
                                REGRESSION=" ".join(REGRESSION), BIOME=BIOME)
    acceptance = gate
    prose = (
        f"{row['title']} — OnlyOffice parity (slugs:{slug}).\n"
        f"SPEC: {row['spec']}\n"
        f"FIRST STEP: {row['first_step']}\n"
        "Work in the WO editor (index.html / editor.js / i18n.js or sibling "
        f"modules) under {editor}/. Scope is restricted there — stay inside "
        "it. Implement the spec honestly and wire the control to an EXISTING "
        "verified command or an existing dialog opener; no stub/no-op "
        "handlers. You MUST also author a zero-dependency node test at "
        f"{editor}/{slug}.test.mjs in the repo's established style "
        "(node:assert + readFileSync, runnable with 'node "
        f"{editor}/{slug}.test.mjs') asserting the new behavior — the "
        "acceptance gate runs it. Commit your work; the gate decides the merge."
    )
    return {
        "id": tid,
        "title": f"{tid}: {row['title']}",
        "deps": [],
        "manual": False,
        "priority": 4,
        "scope": [f"{editor}/**"],
        "touch": [
            f"{editor}/index.html",
            f"{editor}/editor.js",
            f"{editor}/i18n.js",
            f"{editor}/{slug}.test.mjs",
        ],
        "acceptance_prose": prose,
        "accept": acceptance,
        "gate_replay": True,
        "max_turns": 64,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--enqueue-all", action="store_true")
    ap.add_argument("--batch", type=int)
    ap.add_argument("--out")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--record", dest="record", metavar="STATE_DIR")
    args = ap.parse_args()

    rows = parse_ledger(LEDGER)
    st = load_state()

    if args.enqueue_all:
        for r in rows:
            st.setdefault(r["slug"], {"batches_failed": 0, "parked": False})
        save_state(st)
        print(f"queued {len(rows)} slugs")
        return 0

    if args.record:
        # Read receipts: merged -> parked(done); count failed batches per slug.
        rec_dir = Path(args.record) / "receipts"
        merged = {}
        failed = {}
        if rec_dir.exists():
            for f in rec_dir.glob("*.json"):
                try:
                    d = json.loads(f.read_text(encoding="utf-8"))
                    out = d.get("outcome")
                    m = re.match(r"^WO-\d+-([\w-]+)", d.get("task", ""))
                    if not m:
                        continue
                    slug = m.group(1)
                    if out == "merged":
                        merged[slug] = True
                    elif out in ("failed", "interrupted", "recovered"):
                        failed[slug] = failed.get(slug, 0) + 1
                except Exception:
                    pass
        for slug, was_merged in merged.items():
            old = st.setdefault(slug, {"batches_failed": 0, "parked": False})
            if not old.get("parked"):
                old["parked"] = True
                old["reason"] = "merged (accepted by gate)"
        for slug, n in failed.items():
            old = st.setdefault(slug, {"batches_failed": 0, "parked": False})
            if old.get("parked") or n < old.get("batches_failed", 0):
                continue
            old["batches_failed"] = n
            if n >= SHELVE_AFTER and not old.get("parked"):
                old["parked"] = True
                old["reason"] = f"shelved after {n} failed batches"
        save_state(st)
        print(f"recorded: merged={len(merged)} failed_batches={len(failed)}")
        return 0

    if args.batch is None:
        ap.error("need --batch N (with --out)")
    batch = args.batch
    pending = []
    for r in rows:
        reason = shelve_reason(r["slug"], st.get(r["slug"], {}))
        if reason is None:
            pending.append(r)
    tasks = [build_task(r, batch) for r in pending[:BATCH_LIMIT]]

    if args.dry_run:
        print(f"batch {batch}: {len(tasks)} pending of {len(rows)} rows (limit {BATCH_LIMIT})")
        for t in tasks:
            print(f"  - {t['id']}")
        return 0

    out = {
        "_meta": {
            "round": f"wo-auto-batch{batch}",
            "generated": "wo-gen-round.py",
            "queue": str(LEDGER),
            "shelve_after": SHELVE_AFTER,
        },
        "tasks": tasks,
    }
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(out, indent=2), encoding="utf-8")
    print(f"wrote {args.out}: {len(tasks)} task(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
