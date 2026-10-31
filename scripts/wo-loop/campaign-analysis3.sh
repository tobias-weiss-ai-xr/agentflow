#!/bin/bash
set -u
cd /home/weiss/.wo-loop

python3 - <<'PY'
import json, glob
from collections import defaultdict

recs = []
for f in glob.glob("state/receipts/*.json"):
    recs.append(json.load(open(f)))

total_tok = sum(r["tokens"] or 0 for r in recs)
waste_tok = sum((r["tokens"] or 0) for r in recs if r["outcome"] != "merged")
print(f"total tokens: {total_tok/1e6:.1f}M | non-merged (wasted): {waste_tok/1e6:.1f}M ({100*waste_tok/total_tok:.0f}%)")

by_task = defaultdict(dict)
for r in recs:
    by_task[r["task"]][r["attempt"]] = r
print()
print("=== paired retries: does attempt 2 cost less than attempt 1? ===")
n = 0
for t, att in sorted(by_task.items()):
    if 1 in att and 2 in att and att[1]["outcome"] != "merged":
        a1, a2 = att[1], att[2]
        r1 = (a1["tokens"] or 0) / 1e6
        r2 = (a2["tokens"] or 0) / 1e6
        n += 1
        print(f"  {t}: a1={r1:.2f}M/{(a1['wall_clock_s'] or 0)/60:.0f}m({a1['outcome'][:5]}) -> a2={r2:.2f}M/{(a2['wall_clock_s'] or 0)/60:.0f}m({a2['outcome'][:5]}) ratio={r2/max(r1,0.01):.2f}")
print(f"paired: {n}")

print()
print("=== failure kind tally (from error text) ===")
from collections import Counter
kinds = Counter()
for r in recs:
    e = str(r.get("error") or "")
    if "turn cap" in e: kinds["turn-cap"] += 1
    elif "local changes" in e: kinds["dirty-merge"] += 1
    elif r["outcome"] == "failed": kinds["other"] += 1
print(dict(kinds))
PY
