#!/bin/bash
set -u
KEY=$(sudo awk -F= '/^LITELLM_API_KEY=/{gsub(/"/,"",$2); print $2}' /etc/environment)
echo "=== saia/mistral-medium-3.5-128b JSON test ==="
curl -s --max-time 90 http://192.168.42.42:4000/v1/chat/completions \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"model":"saia/mistral-medium-3.5-128b","max_tokens":200,"temperature":0,"messages":[{"role":"user","content":"Reply with exactly this JSON and nothing else: {\"ok\": true}"}]}' | python3 -c "import json,sys; d=json.load(sys.stdin); c=d[\"choices\"][0]; print(\"finish:\", c[\"finish_reason\"], \"| content:\", repr(c[\"message\"][\"content\"])[:120])"
