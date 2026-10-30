#!/bin/bash
set -euo pipefail
# Install wo-loop on legion + smoke test (no LLM calls, no merges).
D=/home/weiss/.wo-loop
mkdir -p "$D"
install -m 0755 /tmp/wo-loop.sh "$D/wo-loop.sh"
install -m 0644 /tmp/wo-gen-round.py "$D/wo-gen-round.py"
install -m 0644 /tmp/wo-wo-loop.service /tmp/wo-loop.service 2>/dev/null || true

echo "=== installed files ==="
ls -la "$D"
python3 -c "import ast; ast.parse(open('$D/wo-gen-round.py').read()); print('gen: syntax OK')"
bash -n "$D/wo-loop.sh" && echo "loop: syntax OK"

echo ""
echo "=== smoke: generate batch 1 and af validate ==="
cd "$D"
python3 wo-gen-round.py --batch 1 --out /tmp/smoke-batch.json
cp /home/weiss/.wo-loop/workers.json /tmp/smoke-workers.json 2>/dev/null || cp /tmp/workers.json /tmp/smoke-workers.json 2>/dev/null || cp /tmp/af-wo-deep1/workers.json /tmp/smoke-workers.json
/usr/local/bin/af validate --tasks /tmp/smoke-batch.json --workers /tmp/smoke-workers.json 2>&1 | tail -1

echo ""
echo "=== smoke: af run --once --dry-run (no dispatch) with batch 1 ==="
cd /home/weiss/git/World-Office/server
TF_REPO_DIR=/home/weiss/git/World-Office/server TF_STATE_DIR=/tmp/af-wo-smoke/state TF_MAX_PARALLEL=1 \
  /usr/local/bin/af run --once --dry-run --tasks /tmp/smoke-batch.json --workers /tmp/smoke-workers.json 2>&1 | head -30

echo ""
echo "=== smoke exit: does the loop script's af invocation parse? (run --dry-run path) ==="
TF_REPO_DIR=/home/weiss/git/World-Office/server TF_STATE_DIR=/tmp/af-wo-smoke/state TF_MAX_PARALLEL=1 \
  /usr/local/bin/af run --dry-run --poll 30 --tasks /tmp/smoke-batch.json --workers /tmp/smoke-workers.json 2>&1 | tail -5
echo "exited rc=$?"
