#!/bin/bash
set -uo pipefail
echo "=== how to re-run the WO census capture (the re-score input) ==="
sed -n '1,60p' /home/weiss/git/World-Office/wo-test-harness/census/census-wo-deep.cjs | grep -nE "usage|Usage|node |PLAYWRIGHT|puppeteer|browser|PAGE|URL|launch|npm" | head
echo ""
echo "=== reader for WO deep census (expects running WO editor where?) ==="
grep -nE "process\.env|WOPI|editor_url|BASE_URL|WORLD_OFFICE|127\.0\.0\.1|localhost" /home/weiss/git/World-Office/wo-test-harness/census/census-wo-deep.cjs | head -10
echo ""
echo "=== README for harness (run instructions) ==="
head -50 /home/weiss/git/World-Office/wo-test-harness/README.md 2>/dev/null | grep -iE "node |npm |python|census|run" | head -10
