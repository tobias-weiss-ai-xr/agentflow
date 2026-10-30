#!/bin/bash
echo "=== gate invocation: cwd + how accept is run ==="
grep -n "run_gate\|Gate">/home/weiss/git/agentflow/src/execute.rs | head
sed -n '/fn run_accept/,/^}/p' /home/weiss/git/agentflow/src/gate.rs | head -40
echo ""
echo "=== looking for where gate runs (current_dir on Command) in gate.rs ==="
grep -n "Command\|current_dir\|\.arg(" /home/weiss/git/agentflow/src/gate.rs | head -20
