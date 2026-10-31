#!/bin/bash
echo "=== service ==="
systemctl is-active wo-loop
echo ""
echo "=== queue state ==="
python3 - <<'PY'
import json
st = json.load(open("/home/weiss/.wo-loop/queue-state.json"))
merged = [s for s, v in st.items() if v.get("parked") and "merged" in str(v.get("reason", ""))]
shelved = [s for s, v in st.items() if v.get("parked") and "merged" not in str(v.get("reason", ""))]
pending = [s for s, v in st.items() if not v.get("parked")]
print(f"merged: {len(merged)}  shelved: {len(shelved)}  pending: {len(pending)}")
print("shelved:", ", ".join(f"{s} ({st[s].get('reason')})" for s in shelved))
print("pending:", ", ".join(pending))
PY
echo ""
echo "=== receipts per batch ==="
python3 - <<'PY'
import json, glob, re
from collections import defaultdict
by_batch = defaultdict(lambda: defaultdict(int))
for f in glob.glob("/home/weiss/.wo-loop/state/receipts/*.json"):
    d = json.load(open(f))
    m = re.match(r"WO-(\d+)-(.+)", d.get("task", ""))
    if m:
        by_batch[int(m.group(1))][(m.group(2), d.get("outcome"))] += 1
for b in sorted(by_batch):
    pairs = ", ".join(f"{slug}:{out}" for (slug, out), n in sorted(by_batch[b].items()))
    print(f"batch {b}: {pairs}")
PY
echo ""
echo "=== server main: af commits since reseed start (03:30) ==="
git -C /home/weiss/git/World-Office/server log --oneline --since="2026-10-31 03:30" | head -15
echo ""
echo "=== base repo clean? ==="
git -C /home/weiss/git/World-Office/server status --porcelain | head -5
echo "(empty = clean)"
