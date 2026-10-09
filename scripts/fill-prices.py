#!/usr/bin/env python3
"""Fill price_per_mtok_usd from OpenRouter for workers without a cost tag.

af declares ONE price per worker (USD / 1M tokens, applied to total tokens).
OpenRouter publishes separate prompt/completion prices; agent-loop traffic is
input-heavy (the conversation is re-sent every turn), so the default basis is
a 3:1 input:output blend. Override with --blend IN_RATIO.

Usage:
  python3 scripts/fill-prices.py config/dogfood15-workers.json          # dry run
  python3 scripts/fill-prices.py config/dogfood15-workers.json --write  # apply
Skips workers that already declare price_per_mtok_usd (use --force to refill).
Unmatched model names are left alone and reported.
"""
import argparse, json, sys, urllib.request

QUANT_TOKENS = {"awq", "int4", "int8", "gptq", "fp8", "fp16", "gguf", "exl2"}


def normalize(name: str) -> str:
    name = name.split("/")[-1].lower().lstrip("~")
    parts = [p for p in name.replace("_", "-").split("-") if p not in QUANT_TOKENS]
    return "-".join(parts)


def lookup(table: dict, model: str):
    """Match an OpenRouter slug for `model`, litellm route names included:
    'local/deepseek-v4-flash/ai1' falls back to its middle segment."""
    candidates = [normalize(model)] + [normalize(s) for s in model.split("/")[:-1][::-1]]
    for cand in candidates:
        if cand in table:
            return table[cand]
    return None


def fetch_openrouter() -> dict:
    req = urllib.request.Request("https://openrouter.ai/api/v1/models")
    data = json.load(urllib.request.urlopen(req, timeout=60))["data"]
    table = {}
    for m in data:
        p = m.get("pricing", {})
        try:
            prompt, completion = float(p["prompt"]), float(p["completion"])
        except (KeyError, TypeError, ValueError):
            continue
        table[normalize(m["id"])] = (m["id"], prompt, completion)
    return table


def blend_price(prompt: float, completion: float, in_ratio: int) -> float:
    return (in_ratio * prompt + completion) / (in_ratio + 1) * 1e6


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--write", action="store_true")
    ap.add_argument("--force", action="store_true", help="refill workers that already declare a price")
    ap.add_argument("--blend", type=int, default=3, metavar="IN_RATIO")
    args = ap.parse_args()

    if "--selftest" in sys.argv or True:  # cheap guard: the matcher must handle the known fleet
        assert normalize("GLM-5.2-AWQ-INT4") == "glm-5.2"
        assert normalize("qwen3.5-397b-a17b") == "qwen3.5-397b-a17b"
        assert normalize("~/deepseek-v4-flash-latest") == "deepseek-v4-flash-latest"
        assert "deepseek-v4-flash" in {normalize(s) for s in "local/deepseek-v4-flash/ai1".split("/")}

    table = fetch_openrouter()
    changed = False
    for path in args.files:
        cfg = json.load(open(path, encoding="utf-8"))
        for w in cfg.get("workers", []):
            if w.get("price_per_mtok_usd") is not None and not args.force:
                continue
            hit = lookup(table, w.get("model", ""))
            if not hit:
                print(f"UNMATCHED  {w['name']:16s} {w.get('model')} — fill manually")
                continue
            slug, prompt, completion = hit
            price = round(blend_price(prompt, completion, args.blend), 3)
            tag = "REFILL  " if w.get("price_per_mtok_usd") is not None else "FILL    "
            print(f"{tag}{w['name']:16s} {w.get('model'):28s} <- {slug:40s} "
                  f"in={prompt*1e6:.3f} out={completion*1e6:.3f} -> basis {price} $/Mtok ({args.blend}:1)")
            w["price_per_mtok_usd"] = price
            changed = True
        if args.write and changed:
            json.dump(cfg, open(path, "w", encoding="utf-8"), indent=2, ensure_ascii=False)
            open(path, "a", encoding="utf-8").write("\n")
    print("dry run — pass --write to apply" if not args.write else "written")
    return 0


if __name__ == "__main__":
    sys.exit(main())
