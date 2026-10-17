# Version-bump automation — design

Date: 2026-10-17
Status: Approved (design); pending implementation plan

## Purpose

Give agentflow a manual, scripted way to cut a new version: bump the crate
version in `Cargo.toml` (the single source of truth), keep the README docker
examples from drifting, verify the build, and create a git tag — all in one
command. Today there is no such tooling; version is `0.1.0` everywhere and
`flake.nix` already derives package/image versions from `Cargo.toml`.

## Scope

- Add `scripts/bump-version.sh` (bash, run in WSL — consistent with `scripts/dist/*.sh`).
- No CHANGELOG, no CI release workflow, no new runtime surface in `af`.

## Interface

```
scripts/bump-version.sh <new-version>
```

`<new-version>` is a strict semver, e.g. `0.2.0`. Pre-release/build metadata
(e.g. `0.2.0-rc.1`) is intentionally **not** supported in this iteration —
the tag `v<new>` and the regex guard assume plain `X.Y.Z`.

## Safety guards (all-or-nothing, checked before any mutation)

1. Exactly one argument; must match `^[0-9]+\.[0-9]+\.[0-9]+$`.
2. Tag `v<new-version>` must not already exist (`git rev-parse -q --verify refs/tags/v$NEW`).
3. Working tree must be clean (`git status --porcelain` empty). This prevents
   unrelated local work (e.g. a modified `.gitignore` or untracked configs)
   from being swept into the release commit.

## Steps

1. Rewrite `Cargo.toml` `version = "<old>"` → `version = "<new>"` (line 3).
2. `cargo build` (debug profile) — also updates the package version in `Cargo.lock`.
3. Rewrite README docker tags: `agentflow:<old>` → `agentflow:<new>` (the two
   examples currently at README lines ~113 and ~117). Replace every occurrence
   of the old docker tag, not just the first.
4. Smoke: `./target/debug/af --version` must print exactly `af <new>`; else fail.
5. `git add Cargo.toml Cargo.lock README.md`
6. `git commit -m "release: v<new>"`
7. `git tag -a v<new> -m "v<new>"`
8. Print summary + next steps (`git push && git push --tags`).

## Failure behavior

`set -euo pipefail` aborts on the first failing command. Because the commit is
near the end, a failure leaves `Cargo.toml`, `Cargo.lock`, and `README.md`
modified in the working tree; the script prints a rollback hint:

```
git checkout -- Cargo.toml Cargo.lock README.md
```

## Exclusions / deferred ideas (captured for later, out of scope)

These were raised during brainstorming but deliberately not included in this
plan (the user chose to implement only the bump script now):

1. **Pin the Rust toolchain** — local `cargo clippy -D warnings` is blocked by
   E0514 from mixed choco cargo 1.92 vs rustup 1.98. A `rust-toolchain.toml`
   would align local lint with CI.
2. **Make the 3 env-fragile tests hermetic** — `worktree::create_rejects_non_repo`
   (runs under `%TEMP%`, which sits inside a git workdir) and the two POSIX-gate
   tests (git-bash not on default PATH) currently fail locally.
3. **Rebase the AF_ env rename onto origin/main** — local `main` is 20 commits
   behind `origin/main` (`c245317`); the 4 rename commits sit on the old
   `24c2a0c` base and should be rebased so the rename lands cleanly before the
   first tag.
4. **Tie the fleet deploy to tagged releases** — `ansible agentflow-sync.yaml`
   ships whatever is on `origin/main`; pairing the bump with a `v<tag>` build
   would make "ship latest" version-pinned and auditable.
5. **Conventional-commits changelog** (e.g. `git-cliff`) — pairs naturally with
   tags; deferred.
6. **Optional `git push` step** in the bump script — kept manual by design.
