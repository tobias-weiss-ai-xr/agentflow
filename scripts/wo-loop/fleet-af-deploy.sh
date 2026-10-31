#!/bin/bash
# Deploy fresh af binaries (run in WSL). Backs up the current binary per host.
set -u
deploy() {  # host arch bin
  h=$1; bin=$2
  timeout 30 scp -q -o ConnectTimeout=5 -o BatchMode=yes "$bin" "weiss@$h:/tmp/af.new" || { echo "$h: scp failed"; return 1; }
  timeout 20 ssh -o ConnectTimeout=5 -o BatchMode=yes "weiss@$h" '
    OLD=$(stat -c %y /usr/local/bin/af 2>/dev/null | cut -c1-10 || true)
    [ -n "$OLD" ] && sudo cp /usr/local/bin/af "/usr/local/bin/af.bak-$OLD"
    sudo install -m 0755 /tmp/af.new /usr/local/bin/af && rm -f /tmp/af.new
    echo "$h: $(/usr/local/bin/af --version) (was $OLD)"
  ' || echo "$h: install failed"
}
deploy ai2 /tmp/af-dist/af-linux-arm64
deploy chemie-lernen.org /tmp/af-dist/af-linux-x64
deploy contextual-intelligence.org /tmp/af-dist/af-linux-x64
echo "--- tobias-weiss.org: check toolchain (glibc 2.31 needs native build) ---"
timeout 10 ssh -o ConnectTimeout=5 -o BatchMode=yes weiss@tobias-weiss.org '
  ldd --version | head -1
  which cargo && cargo --version || echo no-cargo
  ls -d ~/.cargo 2>/dev/null || echo no-rustup
' 2>&1 || echo "tobias-weiss.org: unreachable"
