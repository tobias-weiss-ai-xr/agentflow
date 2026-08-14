# taskfleet-research

Competitive landscape research for the taskfleet parallel LLM task orchestration system.

Fetches, classifies, deduplicates, and digests similar projects from GitHub,
arXiv, and other sources to maintain an up-to-date competitive analysis.

## Quick Start

```bash
# Full pipeline: fetch → classify → dedup → digest
./run_pipeline.sh

# Dry run (preview, no writes)
./run_pipeline.sh --dry-run

# Fetch only (append to repos.yaml)
./run_pipeline.sh --fetch-only

# Re-digest existing data (after editing repos.yaml)
./run_pipeline.sh --digest-only
```

## Pipeline Stages

```
config/sources.yml          — GitHub search queries + arXiv categories + keyword taxonomy
        │
        ▼
  ① fetch                   — GitHub API + arXiv API → normalized items
        │
        ▼
  ② classify                — keyword matching → tags + relevance score
        │
        ▼
  ③ dedup                   — SHA-based dedup against data/repos.yaml
        │
        ▼
  ④ digest                  — data/landscape.md + data/landscape.html + data/landscape.json
```

## File Layout

```
research/
├── run_pipeline.sh             # entrypoint (bootstrap venv, run pipeline)
├── run_pipeline.py             # orchestrator: fetch → classify → dedup → digest
├── requirements.txt            # Python deps
├── config/
│   └── sources.yml             # source definitions, keyword taxonomy, scoring params
├── ingest/
│   ├── __init__.py
│   ├── fetch.py                # GitHub + arXiv adapters (normalize → item dicts)
│   ├── classify.py             # keyword-based tagger + relevance scorer
│   ├── dedup.py                # SHA-based seen-item store
│   └── digest.py               # Markdown / HTML / JSON renderers
├── data/
│   ├── repos.yaml              # primary data store (all discovered repos)
│   ├── landscape.md             # rendered digest (Markdown)
│   ├── landscape.html           # rendered digest (standalone HTML)
│   ├── landscape.json           # rendered digest (JSON)
│   ├── seen.json                # dedup state (auto-managed)
│   └── run.log                  # pipeline run log
├── docs/
│   ├── current-system-analysis.md   # taskfleet architecture analysis
│   ├── improvement-ideas.md          # prioritized improvement roadmap
│   └── competitive-matrix.md         # feature comparison matrix
└── README.md
```

## Source Types

| Type | Adapter | Example |
|---|---|---|
| `github` | GitHub Search API | `q=multi-agent LLM orchestration sort=stars` |
| `arxiv` | arXiv API | `categories=["cs.AI", "cs.SE"]` |

## Relevance Scoring

Score = baseline + source_weight + topic_match_bonuses + recency_boost

Topics: orchestration, parallel-dispatch, git-worktree, acceptance-gates, multi-agent,
code-agent, llm-verification, dependency-dag, retry-recovery, observability

## Deduplication

Items are deduplicated by SHA-1 of `title|url|source`. Seen items are stored in
`data/seen.json` with configurable retention. The primary data store (`data/repos.yaml`)
also deduplicates by URL.
