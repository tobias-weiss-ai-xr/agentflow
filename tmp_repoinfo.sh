#!/bin/bash
TOKEN=$(cd /mnt/c/Users/Tobias/git/ansible && ANSIBLE_VAULT_PASSWORD_FILE=~/.ansible/vault_password ansible-vault view inventory/group_vars/all/vault.yml 2>/dev/null | grep 'GITHUB_TOKEN=' | head -1 | sed 's/^ *GITHUB_TOKEN=//')
echo "=== default branch head ==="
curl -s -H "Authorization: token $TOKEN" https://api.github.com/repos/tobias-weiss-ai-xr/pi-websearch | python3 -c "
import json,sys
d=json.load(sys.stdin)
print('branch:', d.get('default_branch'), '| pushed:', d.get('pushed_at'))
"
echo "=== tree on main ==="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/tobias-weiss-ai-xr/pi-websearch/git/trees/main?recursive=1" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for t in d.get('tree',[]):
    if t['type']=='blob': print(' ', t['path'])
"
