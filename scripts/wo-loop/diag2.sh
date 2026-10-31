#!/bin/bash
set -u
cd /home/weiss/git/World-Office/wo-test-harness/census
echo "=== ledger format: new (build.py) vs old (what ledger-gap expects) ==="
python3 - <<'PY'
import json
new = json.load(open("ledger/ledger.json"))
old = json.load(open("census/ledger.json"))
print("new top keys:", list(new.keys())[:8])
print("old top keys:", list(old.keys())[:8])
fe = new.get("features")
if fe:
    print("new features:", len(fe))
    print("sample:", json.dumps(fe[0])[:250])
led = new.get("ledger")
if led:
    print("new ledger rows:", len(led), "sample:", json.dumps(led[0])[:250])
PY
echo ""
echo "=== glm-5.2 through litellm ==="
KEY=$(sudo awk -F= '/^LITELLM_API_KEY=/{gsub(/"/,"",$2); print $2}' /etc/environment)
curl -s --max-time 90 http://192.168.42.42:4000/v1/chat/completions \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"model":"glm-5.2","max_tokens":50,"messages":[{"role":"user","content":"Reply with exactly this JSON: {\"ok\": true}"}]}' | head -c 500
echo ""
