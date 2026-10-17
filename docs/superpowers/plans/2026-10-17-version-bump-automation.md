# Version-Bump Automation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `scripts/bump-version.sh` so a maintainer can cut a release with one command: bump `Cargo.toml` (single source of truth), sync README docker tags, verify the build, and commit + create an annotated `v<version>` git tag.

**Architecture:** A standalone bash script (`set -euo pipefail`) that guards on a clean tree + nonexistent tag, edits `Cargo.toml`/`README.md` with `sed`, runs `cargo build` (updating `Cargo.lock`), smokes `./target/debug/af --version`, then commits and tags. A separate bash contract test runs it against a scratch git repo with stub `cargo`/`af` on PATH so no real build is needed.

**Tech Stack:** Bash (run inside WSL, consistent with `scripts/dist/*.sh`), git, sed, cargo.

**Spec:** `docs/superpowers/specs/2026-10-17-version-bump-automation-design.md`

---

## File Structure

- Create `scripts/bump-version.sh` — the release script (one responsibility: cut a release).
- Create `scripts/test-bump-version.sh` — contract test for the script (scratch repo + stub cargo/af).
- Modify `README.md` — add a short "Cutting a release" note (no other README version strings change; the script owns the docker-tag sync).

---

### Task 1: Write the failing contract test

**Files:**
- Create: `scripts/test-bump-version.sh`

- [ ] **Step 1: Write the contract test**

Create `scripts/test-bump-version.sh`:

```bash
#!/usr/bin/env bash
# Contract test for scripts/bump-version.sh. Runs against a scratch git repo
# with stub cargo/af on PATH (no real build, no changes to this repo).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

mkdir -p "$SCRATCH/scripts" "$SCRATCH/target/debug" "$SCRATCH/bin"

# Fixture: scratch repo with a clean initial commit.
cd "$SCRATCH"
git init -q
git config user.email test@test
git config user.name test
cat > Cargo.toml <<'EOF'
[package]
name = "agentflow"
version = "0.1.0"
EOF
printf 'LOCKED 0.1.0\n' > Cargo.lock
printf '  agentflow:0.1.0 run --dry-run\n' > README.md
git add -A
git commit -q -m init

# Stub cargo: simulate `cargo build` by pinning Cargo.lock to the Cargo.toml
# version that is present when it runs (the bumped one).
cat > bin/cargo <<'EOF'
#!/usr/bin/env bash
V=$(sed -n 's/^version = "\([0-9.]*\)"$/\1/p' Cargo.toml)
printf 'LOCKED %s\n' "$V" > Cargo.lock
EOF
chmod +x bin/cargo

# Stub af: report the version currently carried in Cargo.toml. Placed where the
# script expects the built binary (./target/debug/af).
cat > "$SCRATCH/target/debug/af" <<'EOF'
#!/usr/bin/env bash
V=$(sed -n 's/^version = "\([0-9.]*\)"$/\1/p' "$(dirname "$0")/../../Cargo.toml")
echo "af $V"
EOF
chmod +x "$SCRATCH/target/debug/af"

export PATH="$SCRATCH/bin:$PATH"
cp "$ROOT/scripts/bump-version.sh" "$SCRATCH/scripts/bump-version.sh"

# Run the script under test.
bash "$SCRATCH/scripts/bump-version.sh" 0.2.0

# Assertions.
grep -q '^version = "0.2.0"' Cargo.toml
grep -q 'agentflow:0.2.0' README.md
[ "$(sed -n 's/^LOCKED //p' Cargo.lock)" = "0.2.0" ]
git tag | grep -q '^v0.2.0$'
[ "$(git log -1 --format=%s)" = "release: v0.2.0" ]

echo "OK: bump-version.sh contract passed"
```

Note: the stub `af` path `"$(dirname "$0")/../../Cargo.toml"` resolves from
`$SCRATCH/target/debug/af` up to `$SCRATCH/Cargo.toml` (`debug` → `target` → `$SCRATCH`).

- [ ] **Step 2: Run the test to verify it FAILS**

Run (in WSL, from the repo root):

```bash
bash scripts/test-bump-version.sh
```

Expected: FAILS before any assertion with `cp: cannot stat
'.../scripts/bump-version.sh': No such file or directory` (the script under
test does not exist yet). This is the red state.

- [ ] **Step 3: Commit**

```bash
git add scripts/test-bump-version.sh
git commit -m "test(scripts): contract test for bump-version.sh"
```

---

### Task 2: Implement `scripts/bump-version.sh`

**Files:**
- Create: `scripts/bump-version.sh`

- [ ] **Step 1: Write the implementation**

Create `scripts/bump-version.sh`:

```bash
#!/usr/bin/env bash
# Cut a new agentflow release: bump Cargo.toml (single source of truth), keep
# README docker examples in sync, verify the build, commit + v-tag.
# Usage: bash scripts/bump-version.sh <new-version>   (e.g. 0.2.0)
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -ne 1 ]; then
  echo "usage: bash scripts/bump-version.sh <new-version>" >&2
  exit 2
fi
NEW="$1"
if ! [[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: invalid semver '$NEW' (expected X.Y.Z)" >&2
  exit 2
fi

# Guard: tag must not already exist.
if git rev-parse -q --verify "refs/tags/v$NEW" >/dev/null; then
  echo "error: tag v$NEW already exists" >&2
  exit 1
fi

# Guard: working tree must be clean (avoid sweeping unrelated local work).
if [ -n "$(git status --porcelain)" ]; then
  echo "error: working tree has uncommitted changes; commit or stash first" >&2
  exit 1
fi

# Roll back the three bumped files on any error after this point.
trap 'git checkout -- Cargo.toml Cargo.lock README.md 2>/dev/null || true' ERR

OLD=$(sed -n 's/^version = "\([0-9.]*\)"$/\1/p' Cargo.toml)
if [ -z "$OLD" ]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi
echo "bumping $OLD -> $NEW"

# 1. Bump Cargo.toml.
sed -i "s/^version = \"$OLD\"$/version = \"$NEW\"/" Cargo.toml

# 2. Build (updates Cargo.lock package version).
cargo build

# 3. Sync README docker examples.
sed -i "s/agentflow:$OLD/agentflow:$NEW/g" README.md

# 4. Smoke: the binary must report the new version.
VER=$("./target/debug/af" --version)
echo "smoke: $VER"
if [ "$VER" != "af $NEW" ]; then
  echo "error: version mismatch ($VER != af $NEW); rolling back" >&2
  git checkout -- Cargo.toml Cargo.lock README.md
  exit 1
fi

# 5-7. Commit + tag.
git add Cargo.toml Cargo.lock README.md
git commit -m "release: v$NEW"
git tag -a "v$NEW" -m "v$NEW"

echo "done: v$NEW"
echo "next: git push && git push --tags"
```

- [ ] **Step 2: Run the contract test to verify it PASSES**

Run (in WSL, from the repo root):

```bash
bash scripts/test-bump-version.sh
```

Expected: prints `OK: bump-version.sh contract passed` and exits 0.

- [ ] **Step 3: Commit**

```bash
git add scripts/bump-version.sh
git commit -m "feat(scripts): bump-version.sh to cut a release + v-tag"
```

---

### Task 3: Document the release process in README

**Files:**
- Modify: `README.md` (add a short subsection; do not hand-edit the docker tags — the script owns them)

- [ ] **Step 1: Add a "Cutting a release" note**

Insert after the "Docker / Nix" section (after the line `docker run agentflow:0.1.0 --version`). Add:

```markdown
### Cutting a release

```sh
bash scripts/bump-version.sh 0.2.0   # bumps Cargo.toml, syncs README docker tags,
                                     # builds, verifies `af --version`, commits + tags v0.2.0
git push && git push --tags
```
```

Run `bash scripts/test-bump-version.sh` once more to confirm nothing broke.

- [ ] **Step 2: Verify and commit**

```bash
git add README.md
git commit -m "docs: cutting a release with scripts/bump-version.sh"
```

---

## Self-Review

**Spec coverage:**
- `scripts/bump-version.sh` with semver arg → Task 2.
- Guards: exactly-one-arg + regex, tag-not-exist, clean tree → Task 2 (all present).
- Steps: Cargo.toml → cargo build → README docker tags → smoke → commit → tag → next-steps → Task 2 (all present).
- Failure behavior (`set -euo pipefail` + rollback hint on smoke mismatch) → Task 2.
- README docker tag sync (both occurrences via `sed -i ... g`) → Task 2, verified by Task 1's `grep -q agentflow:0.2.0`.
- No CHANGELOG / no CI / no new `af` surface → respected (nothing added).
- Contract test for the script → Task 1.

**Placeholder scan:** no TBD/TODO; every code step is fully specified.

**Type/name consistency:** `bump-version.sh`, `test-bump-version.sh`, `0.2.0`, `v0.2.0`, stub `cargo`/`af` all consistent across tasks.
