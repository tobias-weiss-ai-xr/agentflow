#!/bin/bash
set -u
KEY=$(sudo awk -F= '/^LITELLM_API_KEY=/{gsub(/"/,"",$2); print $2}' /etc/environment)
for M in saia/qwen3.5-122b-a10b saia/gpt-oss-120b; do
  echo "=== $M ==="
  START=$(date +%s)
  curl -s --max-time 170 http://192.168.42.42:4000/v1/chat/completions \
    -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
    -d "{\"model\":\"$M\",\"max_tokens\":100,\"temperature\":0,\"messages\":[{\"role\":\"user\",\"content\":\"Reply with exactly this JSON: {\\\"ok\\\": true}\"}]}" \
    | python3 -c "import json,sys; d=json.load(sys.stdin); c=d['choices'][0]; print('finish:', c['finish_reason'], '| content:', repr(c['message']['content'])[:80])" 2>&1
  echo "took $(( $(date +%s) - START ))s"
done
