#!/bin/bash
set -e
TOKEN=$(cd /mnt/c/Users/Tobias/git/ansible && ANSIBLE_VAULT_PASSWORD_FILE=~/.ansible/vault_password ansible-vault view inventory/group_vars/all/vault.yml 2>/dev/null | grep 'GITHUB_TOKEN=' | head -1 | sed 's/^ *GITHUB_TOKEN=//')
cd /mnt/c/Users/Tobias/git/pi-websearch
git push "https://tobias-weiss-ai-xr:${TOKEN}@github.com/tobias-weiss-ai-xr/pi-websearch.git" main 2>&1 | tail -4
