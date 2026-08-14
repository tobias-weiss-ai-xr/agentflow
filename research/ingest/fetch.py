"""Taskfleet-research: fetch adapters for GitHub and arXiv.

Every adapter returns a list of normalized item dicts:

    {
        "digest":      sha1(id-string),        # stable dedup key
        "source":      <source name>,
        "source_label":<human label>,
        "category":    competitor|framework|tooling|paper,
        "title":       str,
        "url":         str,
        "summary":     str,
        "authors":     [str],
        "published":   ISO8601 str or "",
        "extra":       {},
    }
"""

from __future__ import annotations

import hashlib
import html
import json
import re
import urllib.parse
import urllib.request

import feedparser

_CLEAN_RE = re.compile(r"<[^>]+>")
_WS_RE = re.compile(r"\s+")
_GITHUB_API = "https://api.github.com/search/repositories"


def _norm(text: str | None, limit: int = 500) -> str:
    """Strip HTML, unescape entities, collapse whitespace, truncate."""
    if not text:
        return ""
    txt = _CLEAN_RE.sub(" ", text)
    txt = html.unescape(txt)
    txt = _WS_RE.sub(" ", txt).strip()
    return txt[:limit]


def _iso(dt):
    try:
        if hasattr(dt, "year"):
            return dt.strftime("%Y-%m-%dT%H:%M:%SZ")
    except Exception:
        pass
    return None


def _item(**kw) -> dict:
    id_src = f"{kw.get('title','')}|{kw.get('url','')}|{kw.get('source','')}"
    kw["digest"] = hashlib.sha1(id_src.encode("utf-8")).hexdigest()[:16]
    if "source_label" not in kw:
        kw["source_label"] = kw.get("label") or kw.get("source", "")
    kw.setdefault("extra", {})
    return kw


class Fetcher:
    def __init__(self, timeout: int = 25):
        self.timeout = timeout

    # ------------------------------------------------------ GitHub search
    def fetch_github(self, query: str, *, source: str, category: str,
                     weight: float, sort: str = "stars", max_items: int = 20,
                     label: str | None = None) -> list[dict]:
        """Search GitHub repositories and return normalized items."""
        url = f"{_GITHUB_API}?{urllib.parse.urlencode({'q': query, 'sort': sort, 'order': 'desc', 'per_page': min(max_items, 100)})}"
        try:
            req = urllib.request.Request(
                url,
                headers={"Accept": "application/vnd.github+json",
                         "User-Agent": "taskfleet-research"})
            data = json.loads(urllib.request.urlopen(req, timeout=self.timeout).read())
        except Exception as exc:
            print(f"  [warn] GitHub search '{query}': {exc}")
            return []

        items = data.get("items") or []
        out: list[dict] = []
        for r in items[:max_items]:
            full_name = r.get("full_name", "")
            desc = _norm(r.get("description") or "")
            topics = ", ".join(r.get("topics", []))
            lang = r.get("language") or ""
            stars = r.get("stargazers_count", 0)
            updated = r.get("updated_at", "")
            created = r.get("created_at", "")
            license_info = r.get("license")
            license_name = license_info.get("name", "") if license_info else ""

            # Build a rich summary from available metadata
            summary_parts = [p for p in [desc, f"Language: {lang}" if lang else "",
                                          f"Topics: {topics}" if topics else "",
                                          f"License: {license_name}" if license_name else ""] if p]
            summary = " | ".join(summary_parts)

            out.append(_item(
                source=source, label=label or f"github:{full_name}", category=category,
                weight=weight,
                title=full_name,
                url=r.get("html_url") or f"https://github.com/{full_name}",
                summary=summary,
                authors=[],
                published=updated,
                extra={
                    "stars": stars,
                    "language": lang,
                    "topics": topics.split(", ") if topics else [],
                    "license": license_name,
                    "created_at": created,
                    "forks": r.get("forks_count", 0),
                    "open_issues": r.get("open_issues_count", 0),
                },
            ))
        return out

    # ---------------------------------------------------------- arXiv API
    def fetch_arxiv(self, categories, *, source: str, max_r: int,
                    weight: float, query_filter: str = "") -> list[dict]:
        q_parts = " OR ".join(f"cat:{c}" for c in categories)
        if query_filter:
            full_query = f"({query_filter}) AND ({q_parts})"
        else:
            full_query = q_parts

        url = ("https://export.arxiv.org/api/query?" + urllib.parse.urlencode({
            "search_query": full_query, "start": 0, "max_results": max_r,
            "sortBy": "submittedDate", "sortOrder": "descending",
        }))
        return self._parse_arxiv(feedparser.parse(url), source, weight)

    def _parse_arxiv(self, parsed, source, weight) -> list[dict]:
        out: list[dict] = []
        for e in parsed.entries:
            title = _norm(e.get("title", "")).strip()
            link = (e.get("id") or "").strip()
            if not title or not link:
                continue
            authors = [a.get("name") for a in e.get("authors", []) if a.get("name")]
            summary = _norm(e.get("summary", ""))
            abs_url = link.replace("http://", "https://").replace("/pdf/", "/abs/")
            cats = ", ".join(t.get("term", "") for t in e.get("tags", []) if t.get("term"))
            out.append(_item(
                source=source, label=source, category="paper", weight=weight,
                title=title, url=abs_url, summary=summary, authors=authors,
                published=_iso(e.get("published_parsed")),
                extra={"categories": cats},
            ))
        return out
