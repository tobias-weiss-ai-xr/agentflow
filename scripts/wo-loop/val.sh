#!/bin/bash
echo "=== af validate on generated batch (real schema check) ==="
cd /home/weiss/.wo-loop
cp /tmp/wo-wo-workers.json /tmp/af-wo-deep1/workers.json 2>/dev/null
cp /tmp/af-wo-deep1/workers.json ./workers.json 2>/dev/null || cat > workers.json <<'EOF'
{ "_meta": {"round":"wo-auto"},
  "defaults": {"accept_timeout_s":3000,"max_attempts":4,"retry_delay_s":15,"agent_timeout_s":7200},
  "workers": [{"name":"tud-mistral","provider":"openai","model":"tud/mistral-medium-3.5-128b",
    "api_base":"http://192.168.42.42:4000/v1","api_key_env":"LITELLM_API_KEY",
    "enabled":true,"cli":"builtin","max_turns":96}] }
EOF
/usr/local/bin/af validate --tasks /tmp/test-batch.json --workers ./workers.json 2>&1 | tail -3
echo ""
echo "=== validate a SINGLE-task batch (scope/touch sanity) ==="
python3 -c "
import json
b=json.load(open('/tmp/test-batch.json'))
# take one task alone
one={'tasks':[b['tasks'][0]]}
json.dump(one, open('/tmp/test-batch1.json','w'), indent=2)
"
/usr/local/bin/af validate --tasks /tmp/test-batch1.json --workers ./workers.json 2>&1 | tail -3
