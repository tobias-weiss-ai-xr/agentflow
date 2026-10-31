#!/bin/bash
echo "=== what does ledger-gap.py need? (sources it reads) ==="
grep -nE "ledger.json|interact-ledger|HERE /" /home/weiss/git/World-Office/wo-test-harness/census/ledger-gap.py | head -15
echo ""
echo "=== do the input ledgers exist + are they fresh (re: 2026-10-27 capture)? ==="
ls -la /home/weiss/git/World-Office/wo-test-harness/census/census/ledger.json /home/weiss/git/World-Office/wo-test-harness/census/census/interact-ledger.json 2>/dev/null
echo ""
echo "=== generating ledger-gap.md needs an LLM key (GOOGLE?) — check env ==="
grep -n "GOOGLE_API_KEY\|model" /home/weiss/git/World-Office/wo-test-harness/census/ledger-gap.py | head -5
echo ""
echo "=== is the WO editor locally runnable for a fresh capture? (how deep1/census ran) ==="
grep -rn "127.0.0.1\|localhost\|:8080\|vite\|dev server" /home/weiss/git/World-Office/wo-test-harness/census/census-wo-deep.cjs 2>/dev/null | head -5
echo ""
echo "=== is a WO dev server currently running? ==="
ss -tlnp 2>/dev/null | grep -E ":8080|:5173|:8888|:3000" | head -5 || echo "none listening"
