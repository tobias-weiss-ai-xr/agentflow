#!/usr/bin/env bash
# Nix builds for agentflow: run inside WSL (nix lives at ~/.nix-profile).
# Outputs: result-af (binary), result-af-static (musl), result-image (docker).
# Usage: nix-build.sh [attr]   attr in: default | static | image
set -euo pipefail
export PATH="$HOME/.nix-profile/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export NIX_CONFIG="experimental-features = nix-command flakes"
cd /mnt/c/Users/Tobias/git/agentflow

attr="${1:-default}"
case "$attr" in
  default) out=result-af ;;
  static)  out=result-af-static ;;
  image)   out=result-image ;;
  *) echo "unknown attr: $attr" >&2; exit 2 ;;
esac
nix build ".#packages.x86_64-linux.$attr" -o "$out"
echo "built $out -> $(readlink "$out")"
