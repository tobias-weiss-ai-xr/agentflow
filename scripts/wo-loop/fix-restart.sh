#!/bin/bash
set -euo pipefail
echo "=== stop loop ==="
sudo systemctl stop wo-loop || true
sleep 2
# kill any lingering af so no attempt runs under the old gate
pkill -f 'af run --poll 30' 2>/dev/null && sleep 2 || true
systemctl is-active wo-loop || echo "stopped"

echo "=== update generator ==="
install -m 0644 /tmp/wo-gen-round.py /home/weiss/.wo-loop/wo-gen-round.py
python3 -c "import ast; ast.parse(open('/home/weiss/.wo-loop/wo-gen-round.py').read()); print('syntax OK')"

echo "=== inspect new gate (no cd, absolute biome) ==="
cd /home/weiss/.wo-loop
rm -f /tmp/nb.json
python3 wo-gen-round.py --batch 1 --out /tmp/nb.json >/dev/null
python3 - <<'PY'
import json
t=json.load(open('/tmp/nb.json'))['tasks'][0]
print(t['accept'])
PY

echo ""
echo "=== reset queue state + receipts (fresh fair attempt under fixed gate) ==="
rm -f /home/weiss/.wo-loop/queue-state.json
rm -rf /home/weiss/.wo-loop/state/receipts
rm -f /home/weiss/.wo-loop/next-batch
python3 wo-gen-round.py --enqueue-all

echo "=== start loop ==="
sudo systemctl start wo-loop
sleep 15
systemctl status wo-loop --no-pager | head -5
journalctl -u wo-loop --no-pager -n 4 | tail -4
