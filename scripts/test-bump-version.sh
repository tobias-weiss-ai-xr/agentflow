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
