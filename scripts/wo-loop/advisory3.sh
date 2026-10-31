#!/bin/bash
set -euo pipefail
cd /home/weiss/git/World-Office/wo-test-harness/census
KEY=$(sudo awk -F= '/^LITELLM_API_KEY=/{gsub(/"/,"",$2); print $2}' /etc/environment)
VISION_API_BASE=http://192.168.42.42:4000/v1 \
VISION_API_KEY="$KEY" \
VISION_MODEL=saia/qwen3.5-122b-a10b \
  /usr/bin/python3 ledger-gap.py --out-dir census --chunk 8
echo "=== fresh rows ==="
grep "^### " census/ledger-gap.md
