#!/usr/bin/env bash
# Build a fully STATIC linux aarch64 af binary (musl, crt-static) on ai1 and
# pull it back to /tmp/af-dist on the controller. Run inside WSL.
set -euo pipefail
mkdir -p /tmp/af-dist
ssh -i ~/.ssh/id_ed25519_ansible -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
  -J weiss@192.168.42.42 weiss@192.168.0.27 '
    mkdir -p ~/af-out
    docker run --rm -v ~/af-out:/out rust:1-slim-bookworm bash -c "
      apt-get update -qq >/dev/null
      apt-get install -y -qq git ca-certificates musl-tools >/dev/null 2>&1
      git clone --depth 1 https://github.com/tobias-weiss-ai-xr/agentflow /src -q
      cd /src
      rustup target add aarch64-unknown-linux-musl >/dev/null
      CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
      RUSTFLAGS=\"-C target-feature=+crt-static -C link-arg=-static\" \
      cargo build --release --target aarch64-unknown-linux-musl 2>&1 | tail -1
      cp target/aarch64-unknown-linux-musl/release/af /out/af-linux-arm64
      echo ARM64_OK
    "'
scp -i ~/.ssh/id_ed25519_ansible -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
  -J weiss@192.168.42.42 weiss@192.168.0.27:~/af-out/af-linux-arm64 /tmp/af-dist/
file /tmp/af-dist/af-linux-arm64 | grep -o "statically linked" || { echo "NOT STATIC — abort"; exit 1; }
ls -la /tmp/af-dist/
