# Worker Task: WS-2 — Search providers: Exa MCP, SearXNG, DuckDuckGo, Brave, Tavily, Google + auto-fallback registry

You are an autonomous worker agent in the agentflow pipeline.
You have been assigned **exactly one task**. Do it well, verify it, commit it.

## Context documents (READ FIRST)

1. **Repo conventions:** `AGENTS.md` in the repo root (if present) — read it
   fully; it defines build/test/lint commands and architecture rules.
2. **Task spec:** the Acceptance Criteria below is the complete contract for
   this task. Where it conflicts with anything else, the task spec wins.

## Your task

**ID:** `WS-2`
**Title:** Search providers: Exa MCP, SearXNG, DuckDuckGo, Brave, Tavily, Google + auto-fallback registry

Implement all search backends for pi-websearch (see AGENTS.md). All providers implement the SearchProvider interface from src/types.ts and handle an AbortSignal with a default 20s timeout on every fetch.

File per provider (each exports a factory taking the resolved config subset, e.g. createExaProvider(cfg), …):
- src/providers/exa.ts — Exa via MCP JSON-RPC over HTTP at https://mcp.exa.ai/mcp (works anonymously; optional EXA_API_KEY sent as X-API-Key header when present). Send {jsonrpc:'2.0',method:'tools/call',params:{name:'web_search_exa',arguments:{query,numResults,type,dateRange?...}},id}. Parse the SSE/direct JSON response: content[0].text may be JSON or text — parse defensively into SearchResult[] (title/url/snippet/highlights).
- src/providers/searxng.ts — SEARXNG_BASE_URL JSON API: GET {base}/search?q=&format=json (optional Authorization header). Map results → SearchResult.
- src/providers/duckduckgo.ts — keyless: GET https://lite.duckduckgo.com/lite/?q= and parse result links with class 'result-link' (title) + 'result-snippet' (snippet); robust HTML parsing, empty result tolerated.
- src/providers/brave.ts — GET https://api.search.brave.com/res/v1/web/search?q=&count= with X-Subscription-Token header; map web.results.
- src/providers/tavily.ts — POST https://api.tavily.com/search json {api_key,query,max_results}; map results.
- src/providers/google.ts — GET https://www.googleapis.com/customsearch/v1?key=&cx=&q=; map items.
- src/lib/providers.ts — createProvider(config): returns the primary provider PLUS an ordered fallback chain. If config.provider is set explicitly, fallbacks = [exa, duckduckgo]. If 'auto' (default), primary = first available keyed provider (brave/tavily/google if key present), else exa, with duckduckgo as last-resort. Also export searchWithFallback(providerChain, args): tries each provider in order; on error/timeout/empty, moves to the next and records which provider served; returns {results, provider}. NEVER throws if at least one provider returned results.

Write hermetic tests (mock global fetch) for: each provider's URL/body construction + response parsing + key handling, the fallback chain (primary fails → next used → provider name reported), and empty-result fallback. No live endpoints. Typecheck + tests must pass.

## File scope — edit ONLY these paths

```
src/providers/exa.ts
src/providers/searxng.ts
src/providers/duckduckgo.ts
src/providers/brave.ts
src/providers/tavily.ts
src/providers/google.ts
src/lib/providers.ts
tests/providers.test.ts
```

Editing files outside this scope risks merge conflicts with parallel tasks
and may fail the verification gate. If you believe a file is missing from
the scope, note it in your summary but **do not edit it** — the orchestrator
will re-scope and re-dispatch.

## Acceptance gate — the orchestrator WILL run this

```sh
bun install --silent && bun run typecheck && bun test
```

You MUST run this command yourself before committing. If it fails, fix your
work and re-run. **Never commit code that fails the acceptance gate.** If
you cannot make it pass after a genuine effort, commit nothing and report
the blocker in your summary.

## Working style

1. Read AGENTS.md first. Follow its hard rules exactly (they are project-wide
   invariants — conventions > convenience).
2. Make the minimal correct change that satisfies the acceptance criteria.
3. Run the acceptance gate. If green, commit. If red, fix and re-run.
4. Write your own tests where the task asks for them — never ship logic you
   have not run.
5. Do not invent dependencies. If the repo declares "zero runtime deps",
   honor it.

{{PREVIOUS_ERROR}}
{{EPISODES}}
{{MISSION}}

## If you are a merge-conflict retry

Your previous attempt passed the acceptance gate but failed to merge because
main advanced while you worked (conflicting files are named in the error
above). Your work is **preserved on this branch**. Do this:
1. Run `git rebase main` — resolve any conflict markers in the named files
   (keep BOTH your work and the new main changes where they don't collide).
2. Re-run the acceptance gate; it must pass on the rebased code.
3. Commit the resolution and finish as normal.

## HARD REQUIREMENT: you MUST modify files

Your task is judged ONLY by real file changes in your scope. The
orchestrator checks `git diff` against the base commit before running the
gate.

**If you do not modify at least one in-scope file, the task FAILS
immediately** — regardless of what you write in your summary. Do NOT:
- Claim success without making changes (this is detected and counted as a
  failure)
- "Provide the commit message" or "report completion" as a substitute for
  work
- Stop after reading files / analysis only

If the task is too large, do it in this order and commit progressively:
1. Make the minimal correct change that compiles
2. Run the acceptance gate
3. If green, commit. If red, fix and re-run.
4. Only if you genuinely cannot make it compile after real attempts should
   you commit nothing and report the blocker.

## When finished

1. Run the acceptance gate. It must be green.
2. `git add -A` the files in your scope (and ONLY those).
3. Commit with message: `feat(WS-2): Search providers: Exa MCP, SearXNG, DuckDuckGo, Brave, Tavily, Google + auto-fallback registry`
4. Reply with a concise summary:
   - What you implemented (1–4 bullets)
   - Test count added/passed
   - Any deviation from the contract and why
   - Any follow-up needed

Do not push; the orchestrator merges and pushes.
