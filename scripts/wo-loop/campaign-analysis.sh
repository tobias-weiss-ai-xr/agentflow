#!/bin/bash
# Analyze the WO campaign receipts + af run logs for harness weaknesses.
set -u
cd /home/weiss/.wo-loop

python3 - <<'PY'
import json, glob, re
from collections import Counter, defaultdict
from datetime import datetime

recs = []
for f in glob.glob("state/receipts/*.json"):
    try:
        d = json.load(open(f))
        recs.append(d)
    except Exception:
        pass

print(f"=== receipts: {len(recs)} ===")
outc = Counter(r.get("outcome") for r in recs)
print("outcomes:", dict(outc))

# per-slug attempt economics
by_slug = defaultdict(list)
for r in recs:
    m = re.match(r"WO-(\d+)-([\w-]+)", r.get("task", ""))
    if m:
        by_slug[m.group(2)].append(r)

multi = {s: rs for s, rs in by_slug.items() if len(rs) > 1}
print(f"\nslugs with >1 receipt: {len(multi)} (retry economics live here)")
fail_first = sum(1 for rs in multi.values()
                 if rs[0].get("outcome") != "merged" and any(r.get("outcome") == "merged" for r in rs))
print(f"  failed-first-then-merged: {fail_first}")

# what fields exist on a receipt? (what af records today)
sample = recs[-1]
print("\n=== receipt fields (what af tracks) ===")
for k, v in sample.items():
    print(f"  {k}: {str(v)[:100]}")

# turn counts / duration if present
turns = [r.get("turns") or r.get("turn_count") or r.get("stats", {}).get("turns") for r in recs if isinstance(r.get("stats"), dict) or r.get("turns")]
print("\nturns field present on:", sum(1 for r in recs if r.get("turns") is not None), "receipts")
PY

echo ""
echo "=== failure reasons in logs (af run stdout archived?) ==="
ls state/logs/ 2>/dev/null | head -3
grep -h -oE "(turn cap [0-9]+ reached|gate failed[^:]*|provider error[^"]*|timeout[^"]*)" run.log 2>/dev/null | sort | uniq -c | sort -rn | head -8
echo ""
echo "=== timing: batches ==="
grep -E "batch [0-9]+:" run.log | head -25
