#!/bin/bash
echo "=== what is :8080 and :3000? ==="
ss -tlnp 2>/dev/null | grep -E ":8080|:3000"
echo "--- proc ---"
for pid in $(ss -tlnp 2>/dev/null | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u); do ps -p "$pid" -o pid,etime,cmd --no-headers 2>/dev/null | cut -c1-140; done
echo ""
echo "=== WO dev server reachable? ==="
curl -s -o /dev/null -w "%{http_code}\n" --max-time 5 http://localhost:8080/ 2>/dev/null
curl -s -o /dev/null -w "%{http_code}\n" --max-time 5 http://localhost:3000/ 2>/dev/null
echo ""
echo "=== census-wo-deep.cjs: what does it hit? ==="
grep -nE "PAGE_URL|url ?=|WOPI|editor\?|localhost|127.0.0.1|:8080|:3000" /home/weiss/git/World-Office/wo-test-harness/census/census-wo-deep.cjs 2>/dev/null | head -8
echo ""
echo "=== census rerun entry point (how WO capture is started) ==="
ls /home/weiss/git/World-Office/wo-test-harness/census/ | grep -iE "run|capture|wo-deep" | head
grep -rn "census-wo-deep" /home/weiss/git/World-Office/wo-test-harness/census/*.py /home/weiss/git/World-Office/wo-test-harness/*.md 2>/dev/null | head -8
