#!/bin/bash
# Survey af binary on fleet hosts (run in WSL).
for h in ai2 tobias-weiss.org chemie-lernen.org contextual-intelligence.org tobi-yoga pcrz1334 mac; do
  R=$(timeout 6 ssh -o ConnectTimeout=5 -o BatchMode=yes "weiss@$h" '
    V=$(/usr/local/bin/af --version 2>/dev/null || echo no-af)
    M=$(uname -m)
    T=$(stat -c %y /usr/local/bin/af 2>/dev/null | cut -d. -f1)
    echo "$V $M $T"
  ' 2>/dev/null) || R=unreachable
  echo "$h: $R"
done
