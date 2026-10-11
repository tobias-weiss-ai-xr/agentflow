# Proposal: extend scope fail-fast to the builtin bash tool

The harness write-time scope guard (#3) covers only the `write` and `edit` tools;
a model can still create/modify out-of-scope files via the `bash` tool
(`cat >`, `echo >>`, running a script) and only get caught at attempt-end,
burning the attempt. A *post-bash scope probe* closes that hole: after every
bash tool call, we run `git status --porcelain -uall` and flag any
out-of-scope paths immediately (fail-fast within seconds). This is the
#3 follow-up applied to the remaining surface.

## Goals
- Extend agents' scope fail-fast to bash-origin file writes.
- Detect out-of-scope bash writes within the same turn, not at the end.
- Keep CLI workers unchanged (they cannot be interrupted).

## Non-goals
- No changes to the existing end-of-attempt scope check (still needed for
  CLI workers and as a backstop).
- No changes to the `write`/`edit` pre-guard (already covered).

## Approach
- `scope_probe(wt, scope, log)` after every bash tool call.
- Use `git status --porcelain -uall` to detect new/modified/untracked files.
- Report first out-of-scope file in the tool result as `ERROR: ...` so the
  model can revert/delete and retry within this turn.
- Git unavailable → skip silently (no false positives for non-git repos).
