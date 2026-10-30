#!/bin/bash
sleep 90
echo "=== wo-loop status ==="
systemctl status wo-loop --no-pager | head -6
echo ""
echo "=== run.log tail ==="
tail -20 /home/weiss/.wo-loop/run.log
echo ""
echo "=== af state: any attempt started? ==="
ls /home/weiss/.wo-loop/state/logs/ 2>/dev/null
for f in /home/weiss/.wo-loop/state/logs/*.log; do
  [ -f "$f" ] && echo "--- $(basename $f)" && tail -6 "$f" && echo
done
echo "=== receipts ==="
ls /home/weiss/.wo-loop/state/receipts/ 2>/dev/null | wc -l
echo "=== worktrees ==="
ls /home/weiss/git/World-Office/server/.worktrees/ 2>/dev/null | head
echo "=== queue-state ==="
cat /home/weiss/.wo-loop/queue-state.json
