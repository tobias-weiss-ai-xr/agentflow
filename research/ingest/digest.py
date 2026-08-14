"""Render competitive landscape digest: Markdown, standalone HTML, and JSON."""

from __future__ import annotations

import datetime
from datetime import timezone

CATEGORY_ORDER = ["competitor", "framework", "tooling", "paper"]
CATEGORY_LABEL = {
    "competitor": "🎯 Direct Competitors",
    "framework": "🏗️ Related Frameworks",
    "tooling": "🔧 Adjacent Tooling",
    "paper": "📄 Research Papers",
}


def _md_escape(t: str) -> str:
    return t.replace("|", "\\|")


def render_json(items: list[dict]) -> dict:
    return {
        "generated": datetime.datetime.now(timezone.utc).isoformat(),
        "count": len(items),
        "items": items,
    }


def render_markdown(items: list[dict], generated: str) -> str:
    grouped: dict[str, list] = {}
    for it in items:
        grouped.setdefault(it["category"], []).append(it)

    lines = [
        "# Taskfleet Competitive Landscape",
        "",
        f"_Generated {generated} · {len(items)} items_",
        "",
        "---",
        "",
    ]

    for cat in CATEGORY_ORDER:
        rows = grouped.get(cat)
        if not rows:
            continue
        lines.append(f"## {CATEGORY_LABEL.get(cat, cat)}")
        lines.append("")
        lines.append(f"| # | Project | Stars | Tags | Score | Summary |")
        lines.append(f"|---|---------|-------|------|-------|---------|")
        for i, it in enumerate(rows, 1):
            url = it["url"]
            title = _md_escape(it["title"])
            tags = ", ".join(f"`{t}`" for t in it.get("tags", [])[:4]) if it.get("tags") else ""
            score = it.get("score", 0)
            stars = it.get("extra", {}).get("stars", "")
            stars_str = f"⭐{stars}" if stars else ""
            summary = _md_escape(it.get("summary", ""))[:120]
            lines.append(f"| {i} | [{title}]({url}) | {stars_str} | {tags} | {score} | {summary} |")
        lines.append("")

    # Tags summary
    all_tags: dict[str, int] = {}
    for it in items:
        for t in it.get("tags", []):
            all_tags[t] = all_tags.get(t, 0) + 1
    if all_tags:
        lines.append("## Tag Frequency")
        lines.append("")
        lines.append("| Tag | Count |")
        lines.append("|-----|-------|")
        for tag, count in sorted(all_tags.items(), key=lambda x: -x[1]):
            lines.append(f"| `{tag}` | {count} |")
        lines.append("")

    return "\n".join(lines)


def render_html(items: list[dict], generated: str) -> str:
    grouped: dict[str, list] = {}
    for it in items:
        grouped.setdefault(it["category"], []).append(it)

    html_parts = [
        "<!DOCTYPE html>",
        "<html><head>",
        "<meta charset='utf-8'>",
        "<meta name='viewport' content='width=device-width, initial-scale=1'>",
        f"<title>Taskfleet Competitive Landscape</title>",
        "<style>",
        "body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; max-width: 1200px; margin: 0 auto; padding: 20px; color: #333; }",
        "h1 { color: #1a1a2e; border-bottom: 2px solid #e94560; padding-bottom: 10px; }",
        "h2 { color: #16213e; margin-top: 40px; }",
        "table { width: 100%; border-collapse: collapse; margin: 16px 0; }",
        "th { background: #1a1a2e; color: white; padding: 10px; text-align: left; }",
        "td { padding: 10px; border-bottom: 1px solid #eee; }",
        "tr:hover { background: #f5f5f5; }",
        "a { color: #e94560; text-decoration: none; }",
        "a:hover { text-decoration: underline; }",
        ".tag { display: inline-block; background: #eee; border-radius: 4px; padding: 2px 8px; margin: 2px; font-size: 0.85em; }",
        ".meta { color: #888; font-size: 0.85em; }",
        ".score { font-weight: bold; color: #e94560; }",
        ".stars { color: #f1c40f; }",
        "</style>",
        "</head><body>",
        f"<h1>Taskfleet Competitive Landscape</h1>",
        f"<p class='meta'>Generated {generated} · {len(items)} items</p>",
        "<hr>",
    ]

    for cat in CATEGORY_ORDER:
        rows = grouped.get(cat)
        if not rows:
            continue
        html_parts.append(f"<h2>{CATEGORY_LABEL.get(cat, cat)}</h2>")
        html_parts.append("<table>")
        html_parts.append("<tr><th>#</th><th>Project</th><th>Stars</th><th>Tags</th><th>Score</th><th>Summary</th></tr>")
        for i, it in enumerate(rows, 1):
            url = it["url"]
            title = it["title"]
            tags_html = " ".join(f'<span class="tag">{t}</span>' for t in it.get("tags", [])[:4])
            score = it.get("score", 0)
            stars = it.get("extra", {}).get("stars", "")
            stars_html = f'<span class="stars">⭐{stars}</span>' if stars else ""
            summary = it.get("summary", "")[:150]
            html_parts.append(
                f"<tr><td>{i}</td><td><a href='{url}'>{title}</a></td>"
                f"<td>{stars_html}</td><td>{tags_html}</td>"
                f"<td class='score'>{score}</td><td>{summary}</td></tr>"
            )
        html_parts.append("</table>")

    html_parts.extend(["</body></html>"])
    return "\n".join(html_parts)
