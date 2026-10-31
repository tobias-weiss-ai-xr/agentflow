#!/bin/bash
set -u
cd /home/weiss/.wo-loop

python3 - <<'PY'
import json, glob, re
from collections import Counter

recs = []
for f in glob.glob("state/receipts/*.json"):
    recs.append((f, json.load(open(f))))

print("=== FAILED receipts: error field content ===")
for f, r in recs:
    if r.get("outcome") == "failed":
        err = str(r.get("error"))[:90]
        print(f"  {r['task']}: attempt={r.get('attempt')} err={err}")

print()
print("=== token / time economics ===")
merged = [r for _, r in recs if r["outcome"] == "merged"]
failed = [r for _, r in recs if r["outcome"] == "failed"]
for name, group in (("merged", merged), ("failed", failed)):
    tok = sorted(r["tokens"] for r in group)
    wall = sorted(r["wall_clock_s"] for r in group)
    n = len(group)
    print(f"{name} n={n}: tokens med={tok[n//2]/1e6:.2f}M max={tok[-1]/1e6:.2f}M | wall med={wall[n//2]/60:.0f}m max={wall[-1]/60:.0f}m")

total_tok = sum(r["tokens"] for _, r in recs)
waste_tok = sum(r["tokens"] for _, r in recs if r["outcome"] != "merged")
print(f"\ntotal tokens: {total_tok/1e6:.1f}M | non-merged (wasted): {waste_tok/1e6:.1f}M ({100*waste_tok/total_tok:.0f}%)")

print()
print("=== retry handoff: attempt-2 vs attempt-1 token ratio (paired) ===")
from collections import defaultdict
by_task = defaultdict(dict)
for _, r in recs:
    by_task[r["task"]][r["attempt"]] = r
paired = 0
for t, att in by_task.items():
    if 1 in att and 2 in att:
        a1, a2 = att[1], att[2]
        if a1["outcome"] != "merged":
            paired += 1
            print(f"  {t}: a1={a1['tokens']/1e6:.2f}M/{a1['wall_clock_s']/60:.0f}m({a1['outcome']}) -> a2={a2['tokens']/1e6:.2f}M/{a2['wall_clock_s']/60:.0f}m({a2['outcome']})")
print(f"paired retries: {paired}")
PY

echo "=== failure reasons from per-task logs ==="
for f in state/logs/*.log; do
  tail -3 "$f" | grep -oE "(turn cap [0-9]+ reached|gate.*fail|merge.*fail|Error: [^.]{10,60})" | head -1
done | sort | uniq -c | sort -rn | head -10
