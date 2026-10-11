#!/usr/bin/env bash
#
# backup.sh — backup OpenCloud/MinIO S3 data to a timestamped local archive.
#
#   * syncs an S3 bucket (AWS or MinIO-compatible) into a staging dir
#   * packs it as <prefix>-backup-<ts>.tar.gz in BACKUP_DEST
#   * --encrypt wraps the archive in AES-256-CBC (openssl, PBKDF2)
#   * writes a SHA-256 checksum file; every artifact lands next to the archive
#
# Zero deps beyond s3cmd, tar, gzip, openssl and sha256sum.
# Backward compatible: no flags = a plain backup driven by environment only.
#
# Environment (s3cmd reads its own creds natively, pass them on the command
# line or via S3_ACCESS_KEY / S3_SECRET_KEY):
#   BACKUP_BUCKET     bucket to back up (required for a real run)
#   BACKUP_ENDPOINT   MinIO host, e.g. https://minio.example.net (empty = AWS)
#   BACKUP_PREFIX     optional key prefix under the bucket
#   BACKUP_DEST       where archives land (default: ./backups)
#   BACKUP_PASSPHRASE passphrase for --encrypt (no default, on purpose)
#
# Usage:
#   scripts/backup.sh [--bucket B] [--endpoint URL] [--prefix P]
#                     [--dest DIR] [--encrypt] [--dry-run] [--help]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# --- config ---------------------------------------------------------------
BUCKET="${BACKUP_BUCKET:-}"
ENDPOINT="${BACKUP_ENDPOINT:-}"
PREFIX="${BACKUP_PREFIX:-}"
DEST="${BACKUP_DEST:-${REPO_ROOT}/backups}"
PASSPHRASE="${BACKUP_PASSPHRASE:-}"
ENCRYPT=0
DRY_RUN=0

usage() {
  sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
}

while [ $# -gt 0 ]; do
  case "$1" in
    --bucket)   BUCKET="${2:?--bucket needs a value}";   shift 2 ;;
    --endpoint) ENDPOINT="${2:?--endpoint needs a value}"; shift 2 ;;
    --prefix)   PREFIX="${2:?--prefix needs a value}";   shift 2 ;;
    --dest)     DEST="${2:?--dest needs a value}";       shift 2 ;;
    --encrypt)  ENCRYPT=1; shift ;;
    --dry-run)  DRY_RUN=1; shift ;;
    -h|--help)  usage ;;
    *) echo "backup.sh: unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
done

TS="$(date +%Y%m%d-%H%M%S)"
STAMP="$(basename "${BUCKET:-backup}")-backup-${TS}"
ARCHIVE="${DEST}/${STAMP}.tar.gz"

echo "backup.sh: plan"
echo "  source : s3://${BUCKET}${PREFIX:+/}${PREFIX}${ENDPOINT:+  (endpoint ${ENDPOINT})}"
ENC_NOTE=""; [ "$ENCRYPT" -eq 1 ] && ENC_NOTE="  (AES-256 encrypted)"
echo "  target : ${ARCHIVE}${ENC_NOTE}"
echo "  checksums: ${ARCHIVE}.sha256"

if [ "$DRY_RUN" -eq 1 ]; then
  echo "backup.sh: dry-run — nothing done."
  exit 0
fi

[ -n "$BUCKET" ] || { echo "backup.sh: BACKUP_BUCKET is required (or --bucket)" >&2; exit 2; }
command -v s3cmd >/dev/null 2>&1 || { echo "backup.sh: s3cmd not found in PATH" >&2; exit 2; }
if [ "$ENCRYPT" -eq 1 ]; then
  [ -n "$PASSPHRASE" ] || { echo "backup.sh: --encrypt needs BACKUP_PASSPHRASE" >&2; exit 2; }
  command -v openssl >/dev/null 2>&1 || { echo "backup.sh: openssl not found in PATH" >&2; exit 2; }
fi

mkdir -p "$DEST"
STAGING="${DEST}/.staging.${TS}"
trap 'rm -rf "$STAGING"' EXIT
mkdir -p "$STAGING"

S3URI="s3://${BUCKET}${PREFIX:+/}${PREFIX}"
if [ -n "$ENDPOINT" ]; then
  s3cmd --host "$ENDPOINT" sync "$S3URI" "$STAGING/"
else
  s3cmd sync "$S3URI" "$STAGING/"
fi

if [ -z "$(ls -A "$STAGING")" ]; then
  echo "backup.sh: warning: sync of ${S3URI} produced no files" >&2
fi

tar -C "$STAGING" -czf "$ARCHIVE" .

if [ "$ENCRYPT" -eq 1 ]; then
  openssl enc -aes-256-cbc -salt -pbkdf2 -pass "env:BACKUP_PASSPHRASE" \
    -in "$ARCHIVE" -out "$ARCHIVE.enc"
  rm -f "$ARCHIVE"
  ARCHIVE_CHECK="$ARCHIVE.enc"
else
  ARCHIVE_CHECK="$ARCHIVE"
fi

(cd "$(dirname "$ARCHIVE_CHECK")" && sha256sum "$(basename "$ARCHIVE_CHECK")" > "$ARCHIVE_CHECK.sha256")

echo "backup.sh: done → $ARCHIVE_CHECK (+ sha256)"
