## Why

The worker CLI is hardcoded to pi's flag shape (`cli --provider p --model m -p @file`). On hosts where pi is unavailable or broken (e.g. legion uses opencode), af cannot run campaigns. A configurable worker command template fixes this with one field.

## What Changes

- `workers.json` gains optional `command`: a shell template with `{prompt}` replaced by the absolute prompt file path; run via the platform shell (`sh -c` / `cmd /C`) in the task worktree
- Absent `command` → existing pi-shaped dispatch (backward compatible)
- Prompt path is absolute (agent cwd is the worktree)
- E2E: a worker driven purely by `command` completes a full campaign (dispatch → gate → merge)

## Impact

- `go/config.go` (parse), `go/execute.go` (dispatch); e2e gains a command-template case
