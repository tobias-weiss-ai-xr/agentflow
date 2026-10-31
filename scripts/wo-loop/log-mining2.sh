#!/bin/bash
# Per-attempt turn economics from agent logs.
set -u
cd /home/weiss/.wo-loop

python3 - <<'PY'
import re, glob, json
from collections import Counter

# split logs into attempts
attempts = []  # (task, attempt_n, [lines])
for path in sorted(glob.glob("state/logs/*.log")):
    task = path.split("/")[-1][:-4]
    cur = None
    for line in open(path, errors="replace"):
        m = re.match(r"== attempt (\d+) on worker (\S+)", line)
        if m:
            cur = (task, int(m.group(1)), m.group(2), [])
            attempts.append(cur)
            continue
        if cur is not None:
            cur[3].append(line)

def stats(lines):
    turns = 0
    tools = Counter()
    cmds = Counter()
    fails = 0
    for line in lines:
        m = re.match(r"harness: turn (\d+)/(\d+)", line)
        if m:
            turns = int(m.group(1))
        m = re.match(r"tool (\w+) (\{.*)", line)
        if m:
            tools[m.group(1)] += 1
            try:
                body = json.loads(m.group(2))
                c = body.get("command") or body.get("path") or ""
                if isinstance(c, str):
                    cmds[c[:80]] += 1
            except Exception:
                pass
        if re.match(r"  -> exit [1-9]", line):
            fails += 1
    return turns, tools, cmds, fails

rows = []
for task, att, worker, lines in attempts:
    outcome = "merged" if lines and any("-- merged --" in l for l in lines[-5:]) else ("?" )
    turns, tools, cmds, fails = stats(lines)
    top_dup = cmds.most_common(1)
    dup_count = top_dup[0][1] if top_dup else 0
    dup_cmd = (top_dup[0][0][:60] if top_dup else "")
    rows.append((task, att, turns, sum(tools.values()), fails, dup_count, dup_cmd, dict(tools.most_common(3))))

print(f"{'task#att':40} {'turns':>5} {'calls':>5} {'fails':>5} {'maxdup':>6} top-dup-command")
for r in sorted(rows, key=lambda x: -(x[2] or 0))[:20]:
    t = f"{r[0]}#{r[1]}"
    print(f"{t:40} {r[2]:>5} {r[3]:>5} {r[4]:>5} {r[5]:>6} {r[6]}")

# aggregate: exit-fail rate and duplication overall
tot_calls = sum(r[3] for r in rows)
tot_fails = sum(r[4] for r in rows)
print(f"\ntotal tool calls: {tot_calls}, failing exits: {tot_fails} ({100*tot_fails/max(tot_calls,1):.0f}%)")

# how many attempts end with repeated identical command >= 5 times (looping)
loop = [r for r in rows if r[5] >= 5]
print(f"attempts running one command >=5 times: {len(loop)}")
for r in loop[:5]:
    print(f"  {r[0]}#{r[1]}: {r[5]}x [{r[6]}]")
PY
