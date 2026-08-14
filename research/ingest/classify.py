"""Keyword-based topic classification + relevance scoring.

Relevance = baseline + source_weight share + topic-match bonuses + recency.
Core topics (orchestration, parallel-dispatch, git-worktree, acceptance-gates)
are weighted more heavily than generic ones.
"""

from __future__ import annotations

import re
import time
from datetime import datetime


def _txt(item: dict) -> str:
    """Build searchable text from title + summary + topics."""
    parts = [item.get("title", ""), item.get("summary", "")]
    # Include GitHub topics if available
    topics = item.get("extra", {}).get("topics", [])
    if topics:
        parts.append(" ".join(topics))
    return " ".join(parts).lower()


class Classifier:
    def __init__(self, keywords: dict, *, fresh_window_days: int = 7, baseline: float = 0.15):
        self.topics: dict[str, list[re.Pattern]] = {}
        for topic, words in (keywords or {}).items():
            pats = []
            for w in words:
                try:
                    pats.append(re.compile(rf"(?<!\w){re.escape(w)}(?!\w)", re.IGNORECASE))
                except re.error:
                    continue
            if pats:
                self.topics[topic] = pats
        self.fresh_window_days = fresh_window_days
        self.baseline = baseline

    def tags(self, item: dict) -> list[str]:
        text = _txt(item)
        return [t for t, pats in self.topics.items() if any(p.search(text) for p in pats)]

    def score(self, item: dict, *, source_weight: float = 1.0) -> float:
        text = _txt(item)
        s = self.baseline + source_weight * 0.3
        # Core competitive topics get higher boost
        high_value = {"orchestration", "parallel-dispatch", "git-worktree",
                      "acceptance-gates", "code-agent", "llm-verification"}
        for topic, pats in self.topics.items():
            count = sum(len(p.findall(text)) for p in pats)
            if not count:
                continue
            boost = 1.0 if topic in high_value else 0.4
            s += boost * min(count, 3)
        # GitHub stars boost (log scale)
        stars = item.get("extra", {}).get("stars", 0)
        if stars and stars > 0:
            import math
            s += min(math.log10(stars + 1) * 0.3, 2.0)
        # Recency boost
        pub = item.get("published") or ""
        if pub:
            dt = _parse(pub)
            if dt is not None:
                age = time.time() - dt
                if 0 <= age < self.fresh_window_days * 86400:
                    s += 0.5
                elif 0 <= age < self.fresh_window_days * 86400 * 4:
                    s += 0.2
        return round(s, 3)


def _parse(pub: str):
    try:
        dt = datetime.fromisoformat(pub.replace("Z", "+00:00")).timestamp()
    except ValueError:
        try:
            dt = datetime.fromtimestamp(float(pub)).timestamp()
        except (ValueError, TypeError):
            return None
    return dt
