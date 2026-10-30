#!/bin/bash
echo "=== mailmerge receipts: outcomes + failure + durations ==="
for f in /home/weiss/.wo-loop/state/receipts/WO-1-mailmerge-*.json; do
  python3 - "$f" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
print(d.get('task'), d.get('outcome'), 'turns=',d.get('turns_taken'), 'wall=',round(d.get('wall_clock_s',0)/60,1),'min')
print('  fail_reason:', str(d.get('failure_reason') or d.get('error') or '')[:200])
PY
done
echo ""
echo "=== last gate output context in mailmerge log (grep gate/turn-cap/biome) ==="
grep -nE "gate failed|turn cap|max_turns|scope|biome|FAIL|DEADLOCK|blocked" /home/weiss/.wo-loop/state/logs/WO-1-mailmerge.log | tail -15
echo ""
echo "=== tail ==="
tail -8 /home/weiss/.wo-loop/state/logs/WO-1-mailmerge.log
echo ""
echo "=== content-controls progress ==="
tail -3 /home/weiss/.wo-loop/state/logs/WO-1-content-controls.log 2>/dev/null
