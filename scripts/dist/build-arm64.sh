#!/usr/bin/env bash
# Build the linux aarch64 af binary natively on ai1 (docker, bookworm base)
# and pull it back to /tmp/af-dist on the controller. Binary requires
# glibc >= 2.32 (ai1/ai2 run Ubuntu 24.04, glibc 2.39).
# Run inside WSL.
set -euo pipefail
mkdir -p /tmp/af-dist
ssh -i ~/.ssh/id_ed25519_ansible -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
  -J weiss@192.168.42.42 weiss@192.168.0.27 '
    mkdir -p ~/af-out
    docker run --rm -v ~/af-out:/out rust:1-slim-bookworm bash -c "
      apt-get update -qq >/dev/null
      apt-get install -y -qq git ca-certificates >/dev/null 2>&1
      git clone --depth 1 https://github.com/tobias-weiss-ai-xr/agentflow /src -q
      cd /src
      cargo build --release 2>&1 | tail -1
      cp target/release/af /out/af-linux-arm64
      echo ARM64_OK
    "'
scp -i ~/.ssh/id_ed25519_ansible -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
  -J weiss@192.168.42.42 weiss@192.168.0.27:~/af-out/af-linux-arm64 /tmp/af-dist/
ls -la /tmp/af-dist/
