# Tasks: multi-repo

## 1. Config
- [x] 1.1 `Config.repos: BTreeMap<String, PathBuf>`; `load_repos(path)` (missing → empty; relative → against file dir)
- [x] 1.2 `repo_dir_for(task, default)` resolution ("" / "main" / named / fallback) + unknown-name warning
- [x] 1.3 Unit tests: matrix of resolutions, relative paths, missing file

## 2. Wiring
- [x] 2.1 `main.rs` `--repos` flag; `TF_REPOS_JSON` env; default `<tasks dir>/repos.json`; warnings printed
- [x] 2.2 `execute.rs` uses `repo_dir_for` for create/merge/remove
- [x] 2.3 `worktree::heal` takes repo list (configured + default); `run.rs` passes it
- [x] 2.4 `ponytail:` note on the global merge lock

## 3. Verification
- [x] 3.1 E2E two-repo campaign (A on main, B on aux, dep A→B) merges into both repos
- [x] 3.2 E2E unknown repo name → warning + fallback, exit 0
- [x] 3.3 Full suite green (back-compat: no repos.json), corpus 64/64, zero warnings

## 4. Docs
- [x] 4.1 `docs/multi-repo-design.md` implementation summary rewritten for af
- [x] 4.2 ADR-11 (multi-repo via repos.json; warn-and-fallback per ADR-4; global merge lock)
- [x] 4.3 README feature bullet + schema note
