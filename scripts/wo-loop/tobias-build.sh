#!/bin/bash
# Native af build on tobias-weiss.org (glibc 2.31 — needs host-built binary).
# Pulls latest main first (github), then rebuilds + installs with backup.
set -euo pipefail
ssh -o BatchMode=yes weiss@tobias-weiss.org '
  set -e
  cd ~/git/agentflow
  git pull -q --ff-only || echo "WARNING: not fast-forward — building current checkout"
  git log --oneline -1
  export PATH="$HOME/.cargo/bin:$PATH"
  cargo build --release 2>&1 | tail -2
  ./target/release/af --version
  sudo cp /usr/local/bin/af "/usr/local/bin/af.bak-$(stat -c %y /usr/local/bin/af | cut -c1-10)"
  sudo install -m 0755 target/release/af /usr/local/bin/af
  /usr/local/bin/af --version
  ls -la /usr/local/bin/af /usr/local/bin/af.bak-*
'
