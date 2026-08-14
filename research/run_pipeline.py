"""taskfleet-research: competitive landscape ingestion pipeline.

Run:
    python run_pipeline.py                # full fetch -> digest
    python run_pipeline.py --dry-run       # do not write files or mark seen
    python run_pipeline.py --fetch-only    # append to repos.yaml, skip digest
    python run_pipeline.py --digest-only   # re-render from existing repos.yaml
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import yaml

from ingest import digest, fetch
from ingest.classify import Classifier
from ingest.dedup import DedupStore

ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data"
CONFIG_PATH = ROOT / "config" / "sources.yml"
STATE_PATH = DATA / "seen.json"
REPOS_PATH = DATA / "repos.yaml"


def main():
    ap = argparse.ArgumentParser(description="taskfleet competitive landscape pipeline")
    ap.add_argument("--config", default=str(CONFIG_PATH))
    ap.add_argument("--state", default=str(STATE_PATH))
    ap.add_argument("--repos", default=str(REPOS_PATH))
    ap.add_argument("--dry-run", action="store_true", help="don't write files or mark items seen")
    ap.add_argument("--fetch-only", action="store_true", help="fetch and append, skip digest rendering")
    ap.add_argument("--digest-only", action="store_true", help="re-render digest from existing repos.yaml")
    ap.add_argument("--top", type=int, default=80, help="max digest items")
    args = ap.parse_args()

    conf = yaml.safe_load(Path(args.config).read_text())
    DATA.mkdir(parents=True, exist_ok=True)

    clf = Classifier(
        conf.get("keywords", {}),
        fresh_window_days=conf.get("fresh_window_days", 7),
    )
    f = fetch.Fetcher(timeout=25)
    store = DedupStore(args.state, retention_days=conf.get("retention_days", 90))

    # --- Digest-only mode: re-render from existing repos.yaml ---
    if args.digest_only:
        return _render_digest_only(args, conf)

    # --- Full / fetch-only mode ---
    print(f"[pipeline] ingesting from {len(conf['sources'])} sources")
    fresh: list[dict] = []

    for src in conf["sources"]:
        name = src["name"]
        if delay := src.get("delay", 0):
            import time as _t
            print(f"  (waiting {delay}s for {name})")
            _t.sleep(delay)
        category = src.get("category", "framework")
        weight = float(src.get("weight", 1.0))
        min_topics = src.get("min_topics", 0)
        try:
            if src["type"] == "github":
                raw = f.fetch_github(
                    src["query"],
                    source=name, category=category, weight=weight,
                    sort=src.get("sort", "stars"),
                    max_items=src.get("max", 20),
                    label=src.get("label"),
                )
            elif src["type"] == "arxiv":
                raw = f.fetch_arxiv(
                    src.get("categories", []),
                    source=name, max_r=src.get("max", 40), weight=weight,
                    query_filter=src.get("query_filter", ""),
                )
                # Override category for arXiv
                for item in raw:
                    item["category"] = src.get("category", "paper")
            else:
                print(f"  [warn] unknown type for {name}: {src['type']}")
                continue
        except Exception as exc:  # noqa: BLE001
            print(f"  [error] {name}: {exc}")
            continue

        print(f"  {name}: {len(raw)} items")
        for it in raw:
            tags = clf.tags(it)
            score = clf.score(it, source_weight=weight)
            it["tags"] = tags
            it["score"] = score
            if min_topics and len(tags) < min_topics:
                continue
            if score >= conf.get("min_score", 0.5) and not store.seen(it["digest"]):
                fresh.append(it)

    fresh.sort(key=lambda x: x["score"], reverse=True)
    top = fresh[: args.top]

    print(f"[2] fresh items: {len(fresh)}; showing top {len(top)}")

    if args.dry_run:
        # Preview
        print("\n--- Top 15 items (dry run) ---")
        for it in top[:15]:
            stars = it.get("extra", {}).get("stars", "")
            stars_str = f" ⭐{stars}" if stars else ""
            print(f"  [{it['score']:.2f}] {it['title']}{stars_str}")
            print(f"    tags: {', '.join(it.get('tags', []))}")
            print(f"    {it['url']}")
        print(f"\n[dry-run] {len(fresh)} fresh items, no files written")
        return

    # Write to repos.yaml (primary data store)
    _append_repos(args.repos, top)

    # Mark seen
    for it in top:
        store.mark_seen(it["digest"], it["title"], it["url"])
    store.save()

    print(f"[3] appended {len(top)} items to {args.repos}")

    if args.fetch_only:
        print("[fetch-only] skipping digest render")
        return

    # Render digest
    _render_digest(args, conf, top)
    print(f"[done] {store.stats()}")


def _append_repos(repos_path: str, items: list[dict]):
    """Append new items to repos.yaml."""
    path = Path(repos_path)
    if path.exists():
        with open(path, encoding="utf-8") as fh:
            data = yaml.safe_load(fh) or {}
    else:
        data = {}
    repos = data.get("repos", [])

    # Build URL set for dedup within the YAML file itself
    existing_urls = {r.get("url", "") for r in repos}
    added = 0
    for it in items:
        if it["url"] in existing_urls:
            continue
        repo_entry = {
            "title": it.get("title", ""),
            "url": it.get("url", ""),
            "category": it.get("category", ""),
            "source": it.get("source", ""),
            "tags": it.get("tags", []),
            "score": it.get("score", 0),
            "summary": it.get("summary", "")[:300],
        }
        # Enrich with GitHub extras if available
        extra = it.get("extra", {})
        for key in ("stars", "language", "license", "forks", "open_issues", "created_at"):
            if extra.get(key):
                repo_entry[key] = extra[key]
        if extra.get("topics"):
            repo_entry["topics"] = extra["topics"]
        # Paper extras
        if it.get("authors"):
            repo_entry["authors"] = it.get("authors", [])[:5]
        if it.get("published"):
            repo_entry["published"] = it.get("published", "")

        repos.append(repo_entry)
        existing_urls.add(it["url"])
        added += 1

    data["repos"] = repos
    with open(path, "w", encoding="utf-8") as fh:
        yaml.dump(data, fh, default_flow_style=False, allow_unicode=True, sort_keys=False)
    return added


def _render_digest(args, conf, items: list[dict]):
    """Render landscape.md, landscape.html, landscape.json from items."""
    gen = __import__("datetime").datetime.now(
        __import__("datetime").timezone.utc).strftime("%Y-%m-%d %H:%M UTC")

    # JSON
    json_path = Path(args.repos).parent / "landscape.json"
    json_path.write_text(
        json.dumps(digest.render_json(items), indent=2, ensure_ascii=False),
        encoding="utf-8")

    # Markdown
    md_path = Path(args.repos).parent / "landscape.md"
    md_path.write_text(digest.render_markdown(items, gen), encoding="utf-8")

    # HTML
    html_path = Path(args.repos).parent / "landscape.html"
    html_path.write_text(digest.render_html(items, gen), encoding="utf-8")

    print(f"[4] wrote landscape.md, landscape.html, landscape.json")


def _render_digest_only(args, conf):
    """Re-render digest from existing repos.yaml."""
    repos_path = Path(args.repos)
    if not repos_path.exists():
        print("[error] repos.yaml not found — run full pipeline first")
        return 1

    with open(repos_path, encoding="utf-8") as fh:
        data = yaml.safe_load(fh) or {}

    repos = data.get("repos", [])
    # Convert YAML entries back to item dicts for digest renderer
    items = []
    for r in repos:
        it = {
            "title": r.get("title", ""),
            "url": r.get("url", ""),
            "category": r.get("category", ""),
            "source": r.get("source", ""),
            "tags": r.get("tags", []),
            "score": r.get("score", 0),
            "summary": r.get("summary", ""),
            "extra": {
                "stars": r.get("stars", 0),
                "language": r.get("language", ""),
                "topics": r.get("topics", []),
            },
        }
        items.append(it)

    items.sort(key=lambda x: x.get("score", 0), reverse=True)
    items = items[:conf.get("max_digest_items", 80)]
    _render_digest(args, conf, items)
    print(f"[digest-only] re-rendered {len(items)} items from repos.yaml")
    return 0


if __name__ == "__main__":
    main()
