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
echo "note: fleet deploy should use dist builds from tag v$NEW (see scripts/dist/*, ansible/playbooks/agentflow-sync.yaml)"
