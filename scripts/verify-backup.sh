#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════
# openSME — Verify Backup Integrity
# ═══════════════════════════════════════════════════════════════
# Validates a backup set produced by scripts/backup.sh:
#   * artifacts exist (postgres_<ts>.sql.gz, traefik_<ts>.tar.gz,
#     volumes_<ts>.tar.gz) and are non-empty
#   * gzip / tar archives are structurally valid (gzip -t / tar -tzf)
#   * .enc artifacts decrypt with AES-256 (openssl, PBKDF2 — the same
#     scheme backup.sh --encrypt writes) and the decrypted stream is a
#     valid gzip / tar archive
#
# Usage:
#   ./scripts/verify-backup.sh                     # newest set in ./backups
#   ./scripts/verify-backup.sh --backup DIR        # explicit backup dir
#   ./scripts/verify-backup.sh --timestamp TS      # specific set (YYYYMMDD_HHMMSS)
#   ./scripts/verify-backup.sh --passphrase P      # for .enc artifacts
#   ./scripts/verify-backup.sh --list              # list backup sets, exit 0
#
# Passphrase may also be given via $BACKUP_PASSPHRASE (same env backup.sh
# --encrypt reads). Read-only: never modifies the backup directory.
# Decryption uses `openssl -pass env:BACKUP_PASSPHRASE` — same transport
# backup.sh --encrypt writes with, so a passphrase never shows in `ps` or
# shell history.
# ═══════════════════════════════════════════════════════════════
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

BACKUP_DIR="${BACKUP_DIR:-./backups}"
TIMESTAMP=""
PASSPHRASE="${BACKUP_PASSPHRASE:-}"
export BACKUP_PASSPHRASE="$PASSPHRASE"
LIST_ONLY=false

# ── Parse arguments ───────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --backup)     shift; BACKUP_DIR="$1"; shift ;;
    --timestamp)  shift; TIMESTAMP="$1"; shift ;;
    --passphrase) shift; PASSPHRASE="$1"; export BACKUP_PASSPHRASE="$PASSPHRASE"; shift ;;
    --list)       LIST_ONLY=true; shift ;;
    -h|--help)
      echo "Usage: $0 [--backup DIR] [--timestamp TS] [--passphrase P] [--list]"
      echo ""
      echo "Options:"
      echo "  --backup DIR      Backup directory (default: ./backups)"
      echo "  --timestamp TS    Verify this set (YYYYMMDD_HHMMSS; default: newest)"
      echo "  --passphrase P    Passphrase for .enc artifacts (or \$BACKUP_PASSPHRASE)"
      echo "  --list            List backup sets found and exit"
      exit 0
      ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

[ -d "$BACKUP_DIR" ] || { echo "✗ Backup directory not found: ${BACKUP_DIR}"; exit 1; }

# ── List / pick a set ─────────────────────────
# Artifact timestamps are YYYYMMDD_HHMMSS — lexicographic == chronological.
TIMESTAMPS="$(
  find "$BACKUP_DIR" -maxdepth 1 -type f \
       \( -name "postgres_*.sql.gz*" -o -name "traefik_*.tar.gz*" -o -name "volumes_*.tar.gz*" \) \
       2>/dev/null | grep -o '[0-9]\{8\}_[0-9]\{6\}' | sort -u
)"

if [ "$LIST_ONLY" = true ]; then
  if [ -z "$TIMESTAMPS" ]; then
    echo "No backup sets found in ${BACKUP_DIR}/"
    exit 0
  fi
  echo "Backup sets in ${BACKUP_DIR}/:"
  echo "$TIMESTAMPS" | while read -r ts; do
    echo "  ${ts}  $(find "$BACKUP_DIR" -maxdepth 1 -type f -name "*_${ts}.*" | wc -l) artifact(s)"
  done
  exit 0
fi

if [ -z "$TIMESTAMP" ]; then
  TIMESTAMP="$(echo "$TIMESTAMPS" | tail -n1)"
  if [ -z "$TIMESTAMP" ]; then
    echo "✗ No backup sets found in ${BACKUP_DIR}/ (use --timestamp to pin one)"
    exit 1
  fi
fi

# ── Verify one artifact ───────────────────────
# $1 = base name prefix (postgres_/traefik_/volumes_), $2 = kind (gzip|tar),
# $3 = mandatory (true|false)
check_artifact() {
  local prefix="$1" kind="$2" mandatory="$3" base file rc=0
  base="${BACKUP_DIR}/${prefix}${TIMESTAMP}"
  if [ -f "${base}.sql.gz" ]; then
    file="${base}.sql.gz"; kind="gzip"
  elif [ -f "${base}.tar.gz" ]; then
    file="${base}.tar.gz"; kind="tar"
  elif [ -f "${base}.sql.gz.enc" ]; then
    file="${base}.sql.gz.enc"; kind="gzip"
  elif [ -f "${base}.tar.gz.enc" ]; then
    file="${base}.tar.gz.enc"; kind="tar"
  else
    if [ "$mandatory" = true ]; then
      echo "  ✗ MISSING  ${base}.(sql.gz|tar.gz)[.enc]"
      return 1
    fi
    echo "  – skipped  ${prefix}${TIMESTAMP} (not present in this backup)"
    return 0
  fi

  if [ ! -s "$file" ]; then
    echo "  ✗ EMPTY    ${file}"
    return 1
  fi

  echo -n "  → ${file##*/} ... "
  if [[ "$file" == *.enc ]]; then
    [ -n "$PASSPHRASE" ] || { echo "✗ needs a passphrase (--passphrase or \$BACKUP_PASSPHRASE)"; return 1; }
    if [ "$kind" = "gzip" ]; then
      openssl enc -d -aes-256-cbc -pbkdf2 -iter 100000 \
        -pass env:BACKUP_PASSPHRASE -in "$file" 2>/dev/null | gzip -t
      rc=$?
    else
      openssl enc -d -aes-256-cbc -pbkdf2 -iter 100000 \
        -pass env:BACKUP_PASSPHRASE -in "$file" 2>/dev/null | tar -tzf - >/dev/null 2>&1
      rc=$?
    fi
  elif [ "$kind" = "gzip" ]; then
    gzip -t "$file"; rc=$?
  else
    tar -tzf "$file" >/dev/null 2>&1; rc=$?
  fi

  if [ "$rc" -eq 0 ]; then
    echo "OK ($(du -h "$file" | cut -f1))"
    return 0
  fi
  echo "CORRUPT (integrity check failed)"
  return 1
}

# ── Run ───────────────────────────────────────
echo "🔍 Verifying backup set ${TIMESTAMP} in ${BACKUP_DIR}/"
FAIL=0
check_artifact "postgres_" "gzip" true  || FAIL=1
check_artifact "traefik_"  "tar"   true  || FAIL=1
check_artifact "volumes_"  "tar"   false || FAIL=1

if [ "$FAIL" -eq 0 ]; then
  echo "✅ Backup set ${TIMESTAMP} is intact"
  exit 0
fi
echo "✗ Backup set ${TIMESTAMP} has problems — re-run backup.sh"
exit 1
