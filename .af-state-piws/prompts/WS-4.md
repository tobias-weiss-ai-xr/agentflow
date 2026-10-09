# Worker Task: WS-4 — Drop-in parity checks + README + publish polish

You are an autonomous worker agent in the agentflow pipeline.
You have been assigned **exactly one task**. Do it well, verify it, commit it.

## Context documents (READ FIRST)

1. **Repo conventions:** `AGENTS.md` in the repo root (if present) — read it
   fully; it defines build/test/lint commands and architecture rules.
2. **Task spec:** the Acceptance Criteria below is the complete contract for
   this task. Where it conflicts with anything else, the task spec wins.

## Your task

**ID:** `WS-4`
**Title:** Drop-in parity checks + README + publish polish

Finish pi-websearch as a polished drop-in replacement and document it.

1. tests/dropin.test.ts — a test asserting the FULL tool-name surface for drop-in parity: importing src/index.ts and simulating session_start, the extension registers tools named WebSearch, websearch, web_search, webfetch and fetch_content (no duplicates, all executable stubs respond). This guards the drop-in contract.

2. README.md — complete documentation: what it is (drop-in replacement for @mammothb/pi-websearch, @alfonzjanfrithz/pi-websearch, pi-web-search, xz-pi-websearch), feature list (multi-provider with auto-fallback, result caching, date filtering, LLM-optimized context, zero runtime deps), install instructions (settings.json "packages": ["npm:@tobias-weiss-ai-xr/pi-websearch"] or pi install), config reference (PI_WEBSEARCH_PROVIDER, EXA_API_KEY, SEARXNG_BASE_URL, BRAVE_API_KEY, TAVILY_API_KEY, GOOGLE_API_KEY/GOOGLE_CX, ~/.pi/agent/pi-websearch.json and project override), a provider comparison table (defaults, keyless support, privacy), and a short performance section (caching, timeouts).

3. package.json — bump description to advertise the complete feature set; ensure "pi":{ "extensions":["./src/index.ts"] }, keywords include drop-in, provider list, caching; keep zero runtime deps (devDependencies only).

All tests + typecheck must pass and README.md must exist.

## File scope — edit ONLY these paths

```
README.md
tests/dropin.test.ts
package.json
```

Editing files outside this scope risks merge conflicts with parallel tasks
and may fail the verification gate. If you believe a file is missing from
the scope, note it in your summary but **do not edit it** — the orchestrator
will re-scope and re-dispatch.

## Acceptance gate — the orchestrator WILL run this

```sh
bun install --silent && bun run typecheck && bun test && test -f README.md
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
3. Commit with message: `feat(WS-4): Drop-in parity checks + README + publish polish`
4. Reply with a concise summary:
   - What you implemented (1–4 bullets)
   - Test count added/passed
   - Any deviation from the contract and why
   - Any follow-up needed

Do not push; the orchestrator merges and pushes.
