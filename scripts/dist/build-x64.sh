#!/usr/bin/env bash
# Build the linux x86_64 af binary in a bookworm container. Binary requires
# glibc >= 2.32 (fine on Ubuntu 22.04+/24.04, Debian bookworm+, Arch).
# Hosts older than that build natively on the host (see playbook skip logic).
# Run inside WSL.
set -euo pipefail
mkdir -p /tmp/af-dist
docker run --rm -v /tmp/af-dist:/out rust:1-slim-bookworm bash -c '
  apt-get update -qq >/dev/null
  apt-get install -y -qq git ca-certificates >/dev/null 2>&1
  git clone --depth 1 https://github.com/tobias-weiss-ai-xr/agentflow /src -q
  cd /src
  cargo build --release 2>&1 | tail -1
  cp target/release/af /out/af-linux-x64
  /out/af-linux-x64 --version
  echo X64_OK
'
ls -la /tmp/af-dist/af-linux-x64
