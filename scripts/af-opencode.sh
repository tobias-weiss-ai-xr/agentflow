#!/usr/bin/env bash
# af↔opencode adapter.
#
# agentflow (Rust port) drives worker CLIs as:
#     cli --provider P --model M [-p @prompt_file]
# opencode's real shape is:
#     opencode run -m M "<prompt>"
# This wrapper swallows the agentflow flags, inlines the prompt file, and
# execs opencode's actual CLI. Also used as the reference for how a worker
# CLI must accept --provider/--model/-p (ADR-1: agentflow stays CLI-agnostic).
set -euo pipefail

model=""
file=""

while [ $# -gt 0 ]; do
  case "$1" in
    --provider)
      shift 2 ;;
    --model)
      model="${2:?--model needs a value}"; shift 2 ;;
    -p)
      if [ "$#" -ge 2 ]; then
        f="$2"
        if [ "${f#@}" != "$f" ]; then file="${f#@}"; shift 2; else shift; file="${1#@}"; shift; fi
      else shift; fi ;;
    *)
      shift ;;
  esac
done

[ -n "$model" ] || { echo "af-opencode: missing --model" >&2; exit 1; }
[ -n "$file" ] && { [ -f "$file" ] || { echo "af-opencode: prompt file missing: $file" >&2; exit 1; }; }

exec opencode run -m "$model" "$(cat "$file")"