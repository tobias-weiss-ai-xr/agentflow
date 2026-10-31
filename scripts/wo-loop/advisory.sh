#!/bin/bash
set -euo pipefail
cd /home/weiss/git/World-Office/wo-test-harness/census

echo "=== sync fresh scores into the committed data dir (census/census/) ==="
cp ledger/ledger.json census/ledger.json
cp interact-ledger.json census/interact-ledger.json
echo "synced. L0 now:"
python3 -c "import json; led=json.load(open('census/ledger.json')); from collections import Counter; print(Counter(r.get('status') for r in led.get('ledger',[])))"

echo ""
echo "=== ledger-gap.py advisory via litellm ==="
KEY=$(sudo awk -F= '/^LITELLM_API_KEY=/{gsub(/"/,"",$2); print $2}' /etc/environment)
[ -n "$KEY" ] || { echo "no key"; exit 1; }
VISION_API_BASE=http://192.168.42.42:4000/v1 \
VISION_API_KEY="$KEY" \
VISION_MODEL="${VISION_MODEL:-glm-5.2}" \
  /usr/bin/python3 ledger-gap.py --out-dir census

echo ""
echo "=== fresh gap rows ==="
grep -c "^### " census/ledger-gap.md
grep "^### " census/ledger-gap.md | head -25
