#!/bin/bash
set -u
AF=/usr/local/bin/af
echo "=== deployed binary ==="
"$AF" --version
ls -la "$AF"
SRC_MTIME=$(stat -c %Y "$AF")
echo "binary mtime epoch: $SRC_MTIME ($(date -d @$SRC_MTIME))"
echo ""
echo "=== commits touching src/ since binary build ==="
cd /home/weiss/git/agentflow
git log --oneline --since="@${SRC_MTIME}" -- src/ Cargo.toml Cargo.lock tests/
echo "(empty above = binary is current)"
echo ""
echo "=== repo HEAD ==="
git log --oneline -1
echo ""
echo "=== other hosts: af version + mtime ==="
for h in ai1 ci nix chn opus hr; do
  V=$(ssh -o ConnectTimeout=4 -o BatchMode=yes weiss@$h '/usr/local/bin/af --version 2>/dev/null; stat -c %y /usr/local/bin/af 2>/dev/null || echo missing' 2>/dev/null | tr '\n' ' ')
  echo "$h: ${V:-unreachable}"
done
