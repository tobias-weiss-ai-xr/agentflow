#!/usr/bin/env python3
"""reseed-queue.py — re-run the documented census re-score and refill wo-loop.

The pipeline (per docs/arc42/census-ledger-pipeline.md cheatsheet):
  1. spawn a FRESH docserver from server main (now carrying the 18 merged
     parity features) — reconcile.spawn_docserver, canonical rust stack
  2. census-wo-deep.cjs  -> census/census-wo-deep.json   (WO deep inventory)
     interact-wo.cjs     -> /tmp interact-wo.json
     interact-diff.py    -> census/interact-ledger.json  (join vs OO ref)
  3. ledger/build.py     -> census/ledger.json re-score (merged features
                            should now match; L0 shrinks)
  4. ledger-gap.py       -> census/ledger-gap.md fresh advisory rows
     (VISION_* env routes the advisory LLM through litellm — proxy policy)
  5. wo-gen-round.py --enqueue-all (setdefault: parked slugs stay parked)

Leaves the wo-test-harness tree dirty with refreshed census JSONs for human
review/commit — that is the documented re-score maintenance flow.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HARNESS = Path("/home/weiss/git/World-Office/wo-test-harness")
CENSUS = HARNESS / "census"
SERVER = Path("/home/weiss/git/World-Office/server")
sys.path.insert(0, str(CENSUS))
import reconcile  # noqa: E402

NODE = shutil.which("node") or "node"


def run(cmd, **kw):
    print(f"  $ {' '.join(map(str, cmd))}", flush=True)
    subprocess.run(cmd, check=True, **kw)


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="reseed-") as td:
        print("== spawn fresh docserver from server main ==", flush=True)
        base, procs = reconcile.spawn_docserver(SERVER, Path(td), "rust")
        url = f"{base}/word/?access_token=stub&file_id=demo.docx"
        try:
            print("== census-wo-deep.cjs (fresh deep inventory) ==", flush=True)
            env = {**os.environ, "WO_URL": url,
                   "WO_DEEP_OUT": str(CENSUS / "census" / "census-wo-deep.json")}
            run([NODE, "census-wo-deep.cjs"], cwd=CENSUS, env=env)

            print("== interact-wo.cjs (click-through) ==", flush=True)
            out = Path(td) / "census"
            out.mkdir(exist_ok=True)
            env = {**os.environ, "CENSUS_WO_URL": url, "CENSUS_OUT": str(out)}
            run([NODE, "interact-wo.cjs"], cwd=CENSUS, env=env)
        finally:
            reconcile._kill(procs)

        print("== interact-diff.py -> census/interact-ledger.json ==", flush=True)
        run([sys.executable, "interact-diff.py", "--wo", str(out / "interact-wo.json"),
             "--out", str(CENSUS)], cwd=CENSUS)

    print("== ledger/build.py re-score ==", flush=True)
    run([sys.executable, "ledger/build.py"], cwd=CENSUS)

    print("== ledger-gap.py advisory (via litellm) ==", flush=True)
    key = ""
    for line in open("/etc/environment", encoding="utf-8"):
        if line.startswith("LITELLM_API_KEY="):
            key = line.split("=", 1)[1].strip().strip('"')
    if not key:
        print("no LITELLM_API_KEY in /etc/environment", file=sys.stderr)
        return 1
    env = {**os.environ, "VISION_API_BASE": "http://192.168.42.42:4000/v1",
           "VISION_API_KEY": key, "VISION_MODEL": os.environ.get("VISION_MODEL", "glm-5.2")}
    run([sys.executable, "ledger-gap.py", "--out-dir", str(CENSUS / "census")],
        cwd=CENSUS, env=env)

    print("== enqueue new slugs ==", flush=True)
    run([sys.executable, "/home/weiss/.wo-loop/wo-gen-round.py", "--enqueue-all"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
