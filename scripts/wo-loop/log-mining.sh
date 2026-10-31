#!/bin/bash
# Mine per-task agent logs: what do turn-cap attempts spend turns on?
set -u
cd /home/weiss/.wo-loop

echo "=== log inventory ==="
ls state/logs/ | wc -l
du -sh state/logs/

echo ""
echo "=== tool-call frequency per FAILED (turn-cap) vs MERGED attempt ==="
python3 - <<'PY'
import json, glob, re
from collections import Counter

# Receipts -> which task/attempt turn-capped vs merged
import json as j
turn_cap = {}
merged = {}
for f in glob.glob("state/receipts/*.json"):
    r = j.load(open(f))
    key = f"{r['task']}#{r['attempt']}"
    if "turn cap" in str(r.get("error")):
        turn_cap[key] = f
    elif r["outcome"] == "merged":
        merged[key] = f

def log_for(key):
    task, att = key.rsplit("#", 1)
    # log naming: WO-<batch>-<slug>.log or with attempt?
    for pat in (f"state/logs/{task}-{att}.log", f"state/logs/{task}.log"):
        try:
            open(pat)
            return pat
        except OSError:
            pass
    return None

def tool_counts(path):
    c = Counter()
    with open(path, errors="replace") as fh:
        for line in fh:
            # native harness logs each tool call as e.g. "-- tool: bash --" / JSON bodies
            m = re.match(r"-- tool[:=] (\w+)", line)
            if m:
                c[m.group(1)] += 1
    return c

print(f"turn-cap attempts: {len(turn_cap)}, merged: {len(merged)}")
sample_tc = sorted(turn_cap)[:3]
sample_m = sorted(merged)[:3]
for label, keys in (("TURN-CAP", sample_tc), ("MERGED", sample_m)):
    for k in keys:
        p = log_for(k)
        if not p:
            print(f"{label} {k}: no log")
            continue
        print(f"{label} {k}: {tool_counts(p)}")
PY
echo ""
echo "=== one turn-cap log: last 30 lines (how it died) + first 10 (how it started) ==="
F=$(ls state/logs/ | grep -m1 "WO-11-table")
head -12 "state/logs/$F"
echo "  [...]"
tail -12 "state/logs/$F"
