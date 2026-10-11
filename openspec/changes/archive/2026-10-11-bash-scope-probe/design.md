# Design: post-bash scope probe

## Detection via git status
- `git status --porcelain -uall` lists all staged, unstaged, and untracked
  files in a machine-readable, stable format.
- Porcelain line: `<XY> <path>` where XY is exactly two status chars followed
  by a space (e.g. `M `, `A `, `??`). Extract `path` from index 3 onwards.

## Integration point
- In `harness::run_tool`, arm `"bash"`:
  1. Call `tool_bash(wt, &cmd, env, env_allow)` → `Result<String, String>`.
  2. On `Ok(out)`, call `scope_probe(wt, scope, log)`.
  3. If probe succeeds (Ok) → `Ok(out)`; otherwise → probe `Err(e)`.
  4. Match arm returns `Result<String, String>`.

## Return type change
- `run_tool`: `String` → `Result<String, String>`. Callers convert `Err(e)` to
  `"ERROR: e"` string so models see them as tool failures (same UX).

## Test plan
- Unit tests on `scope_probe` with temp git repo: in-scope write OK,
  out-of-scope write → Err, empty scope allows all, git absence → Ok.
- Existing harness tools tests updated to handle `Result`.
