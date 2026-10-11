#!/usr/bin/env bash
#
# verify-backup.sh — validate a backup produced by scripts/backup.sh.
#
# Checks, in order:
#   1. SHA-256 checksum against the sibling .sha256 file (if present)
#   2. gzip archive integrity (decrypting to a stream for .enc archives,
#      so plaintext never touches disk on the integrity check)
#   3. restorability — extracts every entry to a temp dir successfully
#
# Usage:
#   scripts/verify-backup.sh <archive.tar.gz|archive.tar.gz.enc|archive.sha256>
#       [--passphrase P] [--self-test] [--help]
#
# Passphrase also via BACKUP_PASSPHRASE. Self-test builds a throwaway
# encrypted archive locally and runs the full verification on it.

set -euo pipefail

SELF_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
PASSPHRASE="${BACKUP_PASSPHRASE:-}"
SELF_TEST=0

# Effective passphrase must reach both direct and recursive (checksum-file)
# verification of encrypted archives via the env openssl reads.
export BACKUP_PASSPHRASE="$PASSPHRASE"

usage() {
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

[ $# -ge 1 ] || { echo "verify-backup.sh: missing archive path (--help)" >&2; exit 2; }

ARCHIVE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --passphrase) PASSPHRASE="${2:?--passphrase needs a value}"; shift 2 ;;
    --self-test)  SELF_TEST=1; shift ;;
    -h|--help)    usage ;;
    -*) echo "verify-backup.sh: unknown option: $1 (--help)" >&2; exit 2 ;;
    *)  ARCHIVE="$1"; shift ;;
  esac
done

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

check_sha256() {  # $1 = archive path; verifies against its sibling .sha256
  local shafile="$1.sha256"
  [ -f "$shafile" ] || { echo "verify-backup.sh: no ${shafile} — checksum skipped" >&2; return 0; }
  (cd "$(dirname "$shafile")" && sha256sum -c "$(basename "$shafile")") \
    || { echo "verify-backup.sh: SHA-256 MISMATCH for $1" >&2; exit 1; }
  echo "verify-backup.sh: checksum OK ($1)"
}

verify_archive() {  # $1 = path to plain .tar.gz (already checksum'd)
  gzip -t "$1" || { echo "verify-backup.sh: gzip integrity FAILED ($1)" >&2; exit 1; }
  tar -xzf "$1" -C "$workdir" \
    || { echo "verify-backup.sh: extraction FAILED — not restorable ($1)" >&2; exit 1; }
  local n; n="$(tar -tzf "$1" | sed '/\/$/d' | wc -l)"
  echo "verify-backup.sh: OK — integrity verified, $n file(s) restored to ${workdir}"
}

if [ "$SELF_TEST" -eq 1 ]; then
  echo "verify-backup.sh: running self-test (local round-trip)"
  echo "self-test payload $(date +%s)" > "$workdir/payload.txt"
  tar -C "$workdir" -czf "$workdir/self.tar.gz" payload.txt
  openssl enc -aes-256-cbc -salt -pbkdf2 -pass "pass:self-test-pass" \
    -in "$workdir/self.tar.gz" -out "$workdir/self.tar.gz.enc"
  rm -f "$workdir/self.tar.gz"
  ARCHIVE="$workdir/self.tar.gz.enc"
  PASSPHRASE="self-test-pass"
  export BACKUP_PASSPHRASE="$PASSPHRASE"
  echo "verify-backup.sh: self-test PASS (encrypt → verify → restore)"
fi

[ -n "$ARCHIVE" ] || { echo "verify-backup.sh: no archive given (--help)" >&2; exit 2; }
[ -f "$ARCHIVE" ]  || { echo "verify-backup.sh: not found: $ARCHIVE" >&2; exit 2; }

case "$ARCHIVE" in
  *.sha256)
    shafile="$(readlink -f "$ARCHIVE")"
    (cd "$(dirname "$shafile")" && sha256sum -c "$(basename "$shafile")") \
      || { echo "verify-backup.sh: SHA-256 MISMATCH (${shafile})" >&2; exit 1; }
    # recurse into each archive named by the checksum file, skip self
    while read -r _ name; do
      case "$name" in
        *.sha256) continue ;;
        *) "$SELF_PATH" "$(dirname "$shafile")/$name" ;;
      esac
    done < <(grep -v '\.sha256$' "$shafile") ;;
  *.enc)
    [ -n "$PASSPHRASE" ] || { echo "verify-backup.sh: encrypted archive needs --passphrase or BACKUP_PASSPHRASE" >&2; exit 2; }
    check_sha256 "$ARCHIVE"
    openssl enc -d -aes-256-cbc -pbkdf2 -pass "env:BACKUP_PASSPHRASE" \
      -in "$ARCHIVE" -out "$workdir/plain.tar.gz" \
      || { echo "verify-backup.sh: decryption FAILED (bad passphrase or corrupt)" >&2; exit 1; }
    verify_archive "$workdir/plain.tar.gz" ;;
  *.tar.gz)
    check_sha256 "$ARCHIVE"
    verify_archive "$ARCHIVE" ;;
  *)
    echo "verify-backup.sh: unsupported artifact: $ARCHIVE (.tar.gz / .enc / .sha256)" >&2
    exit 2 ;;
esac

echo "verify-backup.sh: all checks passed."
