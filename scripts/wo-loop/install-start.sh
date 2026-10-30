#!/bin/bash
set -euo pipefail
echo "=== install service ==="
sudo install -m 0644 /tmp/wo-loop.service /etc/systemd/system/wo-loop.service
sudo systemctl daemon-reload
sudo systemctl enable wo-loop
echo "=== start ==="
sudo systemctl start wo-loop
sleep 20
echo "=== status ==="
systemctl status wo-loop --no-pager | head -15
echo ""
echo "=== journal tail ==="
journalctl -u wo-loop --no-pager -n 30 | tail -25
echo ""
echo "=== state dir ==="
ls -la /home/weiss/.wo-loop/
