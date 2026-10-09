# Worker Task: WS-3 — Tools: websearch + webfetch (all drop-in tool names), LLM-optimized rendering, index wiring

You are an autonomous worker agent in the agentflow pipeline.
You have been assigned **exactly one task**. Do it well, verify it, commit it.

## Context documents (READ FIRST)

1. **Repo conventions:** `AGENTS.md` in the repo root (if present) — read it
   fully; it defines build/test/lint commands and architecture rules.
2. **Task spec:** the Acceptance Criteria below is the complete contract for
   this task. Where it conflicts with anything else, the task spec wins.

## Your task

**ID:** `WS-3`
**Title:** Tools: websearch + webfetch (all drop-in tool names), LLM-optimized rendering, index wiring

Implement the pi tools for pi-websearch (see AGENTS.md) and wire them into src/index.ts. This is a DROP-IN REPLACEMENT: register every tool name the other extensions use.

src/websearch.ts — createWebsearchTool(config): returns a ToolDefinition registering under a primary name 'WebSearch' with these params (JSON-schema style object): query (required string), numResults (number, default 8), type (enum auto/fast/deep), dateRange (string, optional), contextMaxCharacters (number, default 10000). Execute: build SearchArgs, call searchWithFallback through src/lib/providers.ts, apply cache (cacheTtlSeconds from config), render results into a single LLM-optimized text block: per result 'Title\nURL\n<highlights|snippet>\n' separated by blank lines, truncated to contextMaxCharacters total. Provide renderCall (shows the query) and renderResult using @earendil-works/pi-tui (Container/Text/Spacer + theme fg colors) with an expand/collapse preview pattern (SearXNG-style) for long output, and a partial-state 'Searching…' indicator.

src/webfetch.ts — createWebfetchTool(): register tool with params {url (required string), maxCharacters (number, default 20000)}. Fetch the URL (20s timeout, follow redirects), convert with htmlToMarkdown, truncate to maxCharacters.

src/index.ts — in session_start, register the search tool under ALL FOUR names the replaced extensions use: 'WebSearch', 'websearch', 'web_search' (same ToolDefinition, don't duplicate if the name collides; dedupe by tracking registered names), and register 'webfetch' AND 'fetch_content' for the fetch tool. Notify via ctx.ui.notify with the resolved provider + detail. Keep a session_shutdown handler that is idempotent. Do not create processes/sockets/timers at factory time.

Tests: hermetic — mock fetch, assert tool executes return proper text, cache is used on second identical call (fetch called once), fallback renders provider used in output, webfetch truncates. Typecheck + tests must pass.

## File scope — edit ONLY these paths

```
src/websearch.ts
src/webfetch.ts
src/index.ts
tests/websearch.test.ts
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
3. Commit with message: `feat(WS-3): Tools: websearch + webfetch (all drop-in tool names), LLM-optimized rendering, index wiring`
4. Reply with a concise summary:
   - What you implemented (1–4 bullets)
   - Test count added/passed
   - Any deviation from the contract and why
   - Any follow-up needed

Do not push; the orchestrator merges and pushes.
