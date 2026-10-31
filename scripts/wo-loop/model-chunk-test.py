#!/usr/bin/env python3
"""Test advisory chunk gaps[8:16] (the reasoning-triggering one) per model."""
import json
import os
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path("/home/weiss/git/World-Office/wo-test-harness/census")
import importlib.util as _ilu  # noqa: E402
_s = _ilu.spec_from_file_location("lg", HERE / "ledger-gap.py")
lg = _ilu.module_from_spec(_s)
_s.loader.exec_module(lg)

os.environ["VISION_API_BASE"] = "http://192.168.42.42:4000/v1"
os.environ["VISION_API_KEY"] = os.popen(
    "sudo awk -F= '/^LITELLM_API_KEY=/{gsub(/\"/,\"\",$2); print $2}' /etc/environment"
).read().strip()

gaps = lg.collect_gaps()
part = gaps[8:16]
prompt = (lg.RUBRIC + "\n\nGap rows (JSON lines):\n" +
          "\n".join(json.dumps(g) for g in part))

for model in ["saia/mistral-medium-3.5-128b", "saia/deepseek-v4-flash-0731"]:
    print(f"=== {model} ===", flush=True)
    body = json.dumps({"model": model, "temperature": 0, "max_tokens": 4000,
                       "messages": [{"role": "user", "content": prompt}]}).encode()
    req = urllib.request.Request(
        f"http://192.168.42.42:4000/v1/chat/completions", data=body,
        headers={"Authorization": f"Bearer {os.environ['VISION_API_KEY']}",
                 "Content-Type": "application/json"})
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=170) as r:
            d = json.load(r)
        c = d["choices"][0]
        content = c["message"]["content"]
        print(f"  finish={c['finish_reason']} content={'None' if content is None else len(content)} took={time.time()-t0:.0f}s")
        if content:
            try:
                v = lg.parse_verdict(content)
                print(f"  parsed gaps: {len(v.get('gaps', []))}")
            except Exception as e:
                print(f"  parse fail: {e}")
    except Exception as e:
        print(f"  EXC {type(e).__name__}: {e} took={time.time()-t0:.0f}s")
