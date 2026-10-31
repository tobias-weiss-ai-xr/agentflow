#!/usr/bin/env python3
"""Reproduce the advisory chunk call to see why content was None."""
import json
import os
import sys
from pathlib import Path

HERE = Path("/home/weiss/git/World-Office/wo-test-harness/census")
sys.path.insert(0, str(HERE))
os.environ["VISION_API_BASE"] = "http://192.168.42.42:4000/v1"
os.environ["VISION_MODEL"] = "saia/mistral-medium-3.5-128b"
os.environ["VISION_API_KEY"] = os.popen(
    "sudo awk -F= '/^LITELLM_API_KEY=/{gsub(/\"/,\"\",$2); print $2}' /etc/environment"
).read().strip()

import importlib.util as _ilu  # noqa: E402
_s = _ilu.spec_from_file_location(
    "lg", "/home/weiss/git/World-Office/wo-test-harness/census/ledger-gap.py")
lg = _ilu.module_from_spec(_s)  # noqa: E402
_s.loader.exec_module(lg)  # noqa: E402

gaps = lg.collect_gaps()
print(f"gaps: {len(gaps)}")
prov = lg.resolve_provider()
base, key, model = prov
print("model:", model)

part = gaps[:8]  # smaller chunk for the repro
import urllib.request  # noqa: E402

for ci in range(0, len(gaps), 8):
    part = gaps[ci:ci + 8]
    prompt = (lg.RUBRIC + "\n\nGap rows (JSON lines):\n" +
              "\n".join(json.dumps(g) for g in part))
    body = json.dumps({"model": model, "temperature": 0, "max_tokens": 4000,
                       "messages": [{"role": "user", "content": prompt}]}).encode()
    req = urllib.request.Request(f"{base.rstrip('/')}/chat/completions", data=body,
                                 headers={"Authorization": f"Bearer {key}",
                                          "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=170) as r:
            d = json.load(r)
        c = d["choices"][0]
        content = c["message"]["content"]
        print(f"chunk {ci//8}: finish={c['finish_reason']} content={'None' if content is None else len(content)}")
        if content is None:
            print("  RAW MESSAGE:", json.dumps(c["message"])[:400])
    except Exception as e:
        print(f"chunk {ci//8}: EXC {type(e).__name__}: {e}")
sys.exit(0))
