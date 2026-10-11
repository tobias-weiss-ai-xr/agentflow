#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════
# openSME — Enhanced Backup
# ═══════════════════════════════════════════════════════════════
# Backs up:
#   1. PostgreSQL (all databases, via pg_dumpall)
#   2. Traefik ACME/SSL data
#   3. Named Docker volumes (optional)
#
# Usage:
#   ./scripts/backup.sh                         # Full backup (PG + Traefik)
#   ./scripts/backup.sh --volumes              # Also back up Docker volumes
#   ./scripts/backup.sh --volumes --services opencloud,redis  # Specific volumes
#   ./scripts/backup.sh --dry-run              # Preview what would be backed up
#   ./scripts/backup.sh --no-stop              # Don't stop services (risk: inconsistent)
#   ./scripts/backup.sh --encrypt              # AES-256-encrypt every artifact
#   ./scripts/backup.sh --s3                   # Sync backups to an S3 bucket
#                                             #   (MinIO/OpenCloud-compatible)
#
# --encrypt wraps each artifact in AES-256-CBC (openssl, PBKDF2). The
#   passphrase comes from $BACKUP_PASSPHRASE (never a flag; no default).
# --s3 uploads $BACKUP_DIR to s3://$S3_BUCKET/$S3_PREFIX via s3cmd.
#   Point $S3_ENDPOINT at a MinIO/OpenCloud S3 endpoint (empty = AWS S3);
#   credentials via $S3_ACCESS_KEY / $S3_SECRET_KEY (or s3cmd's own config).
#
# Volume names (without project prefix):
#   postgres-data, redis-data, opencloud-config, opencloud-data,
#   stalwart-etc, stalwart-data, sogo-config, sogo-data,
#   traefik-data, casdoor-data, zitadel-machinekey,
#   cryptpad-data, cryptpad-blob, synapse-data, notes-data,
#   seaweedfs-data, paperless-data, paperless-media, paperless-export,
#   invoiceninja-public, invoiceninja-storage
# ═══════════════════════════════════════════════════════════════
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

# ── Config ────────────────────────────────────
COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml:idm/zitadel.yml:opencloud/opencloud.yml}"
export COMPOSE_FILE

BACKUP_DIR="${BACKUP_DIR:-./backups}"
TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
DRY_RUN=false
BACKUP_VOLUMES=false
NO_STOP=false
ENCRYPT=false
S3=false
SERVICES=""

# --encrypt / --s3 config (env, no defaults beyond these)
BACKUP_PASSPHRASE="${BACKUP_PASSPHRASE:-}"
S3_BUCKET="${S3_BUCKET:-}"
S3_ENDPOINT="${S3_ENDPOINT:-}"          # MinIO/OpenCloud host; empty = AWS
S3_ACCESS_KEY="${S3_ACCESS_KEY:-}"
S3_SECRET_KEY="${S3_SECRET_KEY:-}"
S3_PREFIX="${S3_PREFIX:-backups}"
# openssl reads it via `-pass env:`; export so a non-exported caller var works.
export BACKUP_PASSPHRASE

# ── Known volumes ─────────────────────────────
KNOWN_VOLUMES=(
  postgres-data redis-data opencloud-config opencloud-data
  stalwart-etc stalwart-data sogo-config sogo-data
  traefik-data casdoor-data zitadel-machinekey
  cryptpad-data cryptpad-blob synapse-data notes-data
  seaweedfs-data paperless-data paperless-media paperless-export
  invoiceninja-public invoiceninja-storage
  nosdesk-uploads cms-data store-data
  # ai-models deliberately excluded: re-downloadable model cache
)

# ── Parse arguments ───────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run)    DRY_RUN=true; shift ;;
    --volumes)    BACKUP_VOLUMES=true; shift ;;
    --no-stop)    NO_STOP=true; shift ;;
    --encrypt)    ENCRYPT=true; shift ;;
    --s3)         S3=true; shift ;;
    --services)   shift; SERVICES="$1"; shift ;;
    -h|--help)
      echo "Usage: $0 [--volumes] [--services vol1,vol2,...] [--encrypt] [--s3] [--dry-run] [--no-stop]"
      echo ""
      echo "Options:"
      echo "  --volumes           Also back up Docker volumes"
      echo "  --services <list>   Comma-separated volume names (implies --volumes)"
      echo "  --encrypt           Encrypt every artifact with AES-256 (openssl, PBKDF2)"
      echo "  --s3                Upload backups to an S3 bucket (MinIO/OpenCloud)"
      echo "  --dry-run           Preview without making changes"
      echo "  --no-stop           Don't stop services (risk: inconsistent backup)"
      echo ""
      echo "--encrypt needs BACKUP_PASSPHRASE; --s3 needs S3_BUCKET (+ S3_ENDPOINT for"
      echo "MinIO/OpenCloud, credentials via S3_ACCESS_KEY/S3_SECRET_KEY)."
      exit 0
      ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

if [ -n "$SERVICES" ]; then
  BACKUP_VOLUMES=true
fi

mkdir -p "$BACKUP_DIR"

echo "📦 openSME Backup"
echo "   Directory:  ${BACKUP_DIR}/"
echo "   Timestamp:  ${TIMESTAMP}"
echo "   PG dump:    yes"
echo "   Traefik:    yes"
echo "   Volumes:    $([ "$BACKUP_VOLUMES" = true ] && echo 'yes' || echo 'no')"
echo "   Encrypt:    $([ "$ENCRYPT" = true ] && echo 'yes (AES-256)' || echo 'no')"
echo "   S3 upload:  $([ "$S3" = true ] && echo "s3://${S3_BUCKET}/${S3_PREFIX}/" || echo 'no')"
[ -n "$SERVICES" ] && echo "   Services:   ${SERVICES}"
echo ""

# ── Determine volume list ─────────────────────
if [ -n "$SERVICES" ]; then
  SELECTED_VOLUMES=()
  for vol in $(echo "$SERVICES" | tr ',' ' '); do
    found=false
    for v in "${KNOWN_VOLUMES[@]}"; do
      if [ "$v" = "$vol" ]; then
        SELECTED_VOLUMES+=("$vol")
        found=true
        break
      fi
    done
    if [ "$found" = false ]; then
      echo "⚠ Unknown volume: $vol (available: ${KNOWN_VOLUMES[*]})"
      exit 1
    fi
  done
elif [ "$BACKUP_VOLUMES" = true ]; then
  SELECTED_VOLUMES=("${KNOWN_VOLUMES[@]}")
fi

# ── Dry run ───────────────────────────────────
if [ "$DRY_RUN" = true ]; then
  echo "DRY RUN — What would be backed up:"
  echo "  PostgreSQL:  ${BACKUP_DIR}/postgres_${TIMESTAMP}.sql.gz"
  echo "  Traefik:     ${BACKUP_DIR}/traefik_${TIMESTAMP}.tar.gz"
  if [ "$BACKUP_VOLUMES" = true ]; then
    echo "  Volumes:"
    for vol in "${SELECTED_VOLUMES[@]:-}"; do
      echo "    - $vol"
    done
  fi
  if [ "$ENCRYPT" = true ]; then
    [ -n "$BACKUP_PASSPHRASE" ] || { echo "  ✗ --encrypt needs BACKUP_PASSPHRASE set"; exit 1; }
    echo "  * every .sql.gz/.tar.gz becomes <artifact>.enc (AES-256-CBC, PBKDF2)"
  fi
  if [ "$S3" = true ]; then
    [ -n "$S3_BUCKET" ] || { echo "  ✗ --s3 needs S3_BUCKET set"; exit 1; }
    echo "  S3:          s3cmd sync ${BACKUP_DIR}/ → s3://${S3_BUCKET}/${S3_PREFIX}/"
  fi
  exit 0
fi

# ── Validate flag preconditions (real run) ─────
if [ "$ENCRYPT" = true ] && [ -z "$BACKUP_PASSPHRASE" ]; then
  echo "✗ --encrypt needs BACKUP_PASSPHRASE set (passphrase, no default)"
  exit 1
fi
if [ "$S3" = true ]; then
  [ -n "$S3_BUCKET" ] || { echo "✗ --s3 needs S3_BUCKET set"; exit 1; }
  command -v s3cmd >/dev/null 2>&1 || { echo "✗ --s3 needs s3cmd installed"; exit 1; }
fi

# ── Stop services for consistent backup ───────
# NOTE: we no longer `compose down` before the dumps — pg_dumpall and a tar
# of /etc/traefik are safe against a RUNNING stack, and downing the stack
# first made both dumps silently EMPTY (nothing to connect to). The stack
# is only stopped below, and only for volume backups.
if [ "$BACKUP_VOLUMES" = true ]; then
  echo "🛑 Will stop services for the volume backup (after the dumps)"
fi

# ── PostgreSQL dump ───────────────────────────
echo "   → PostgreSQL..."
# Dump the RUNNING server (exec). A second container from `compose run` has
# no server on its socket — it produced a silently EMPTY dump.
if docker compose ps --services --filter status=running 2>/dev/null \
     | grep -q '^postgres$' \
  && docker compose exec -T postgres pg_dumpall -U "${POSTGRES_USER:-opensme}" 2>/dev/null \
       | gzip > "${BACKUP_DIR}/postgres_${TIMESTAMP}.sql.gz"; then
  :  # dump written
else
  echo "   ⚠ PostgreSQL backup skipped (postgres not running)"
fi

# ── Traefik ACME/SSL ──────────────────────────
echo "   → Traefik (ACME certs)..."
# Stream the archive from the RUNNING traefik container. `docker compose run`
# cannot work here: a second traefik instance would collide with the live
# container's published ports 80/443 (which is why the old approach silently
# produced no artifact).
if docker compose ps --services --filter status=running 2>/dev/null \
     | grep -q '^traefik$' \
  && docker compose exec -T traefik tar czf - -C /etc/traefik . \
       > "${BACKUP_DIR}/traefik_${TIMESTAMP}.tar.gz" 2>/dev/null; then
  :  # archive written
else
  echo "   ⚠ Traefik backup skipped (not running or no volume)"
fi

# ── Volume backup ─────────────────────────────
if [ "$BACKUP_VOLUMES" = true ]; then
  PROJECT_NAME="${COMPOSE_PROJECT_NAME:-opensme-compose}"
  PARTIAL_DIR="${BACKUP_DIR}/partial-${TIMESTAMP}"
  mkdir -p "$PARTIAL_DIR"

  # volumes are only consistent while the writers are down
  echo "🛑 Stopping services for volume backup..."
  docker compose down --remove-orphans 2>/dev/null || true
  sleep 5

  for vol in "${SELECTED_VOLUMES[@]}"; do
    FULL_VOL="${PROJECT_NAME}_${vol}"
    echo "   → Volume: $vol..."
    docker run --rm \
      -v "${FULL_VOL}:/data" \
      -v "${PARTIAL_DIR}:/backups" \
      alpine:3.20 \
      tar czf "/backups/${vol}.tar.gz" -C /data . 2>/dev/null \
      || echo "   ⚠ Volume $vol skipped (not found)"
  done

  # Combine volume backups into ONE outer tar containing the per-volume
  # <vol>.tar.gz members — that is exactly the shape restore.sh expects
  # (extract outer archive → loop over member tarballs → one volume each).
  # Byte-concatenating the gzip members (cat a.tar.gz b.tar.gz) is NOT a
  # valid archive: tar stops at the first member's end-of-archive marker
  # and the remaining volumes are silently dropped.
  if [ -d "$PARTIAL_DIR" ] && ls "$PARTIAL_DIR"/*.tar.gz >/dev/null 2>&1; then
    ( cd "$PARTIAL_DIR" && tar czf "${BACKUP_DIR}/volumes_${TIMESTAMP}.tar.gz" ./*.tar.gz )
    rm -rf "$PARTIAL_DIR"
  fi
fi

# ── Restart services (only if volume backup stopped them) ──
if [ "$BACKUP_VOLUMES" = true ] && [ "$NO_STOP" = false ]; then
  echo "🚀 Restarting services..."
  docker compose up -d
  sleep 10
fi

# ── Encrypt (AES-256-CBC, PBKDF2) ──────────────
if [ "$ENCRYPT" = true ]; then
  echo "🔐 Encrypting artifacts (AES-256-CBC)..."
  for f in "${BACKUP_DIR}"/*"_${TIMESTAMP}".sql.gz "${BACKUP_DIR}"/*"_${TIMESTAMP}".tar.gz; do
    [ -f "$f" ] || continue
    openssl enc -aes-256-cbc -pbkdf2 -iter 100000 -salt \
      -pass env:BACKUP_PASSPHRASE -in "$f" -out "$f.enc"
    rm -f "$f"   # plaintext never left at rest
    echo "   → ${f##*/} → ${f##*/}.enc"
  done
fi

# ── S3 upload (MinIO/OpenCloud via s3cmd) ──────
if [ "$S3" = true ]; then
  echo "☁  Uploading to s3://${S3_BUCKET}/${S3_PREFIX}/ ..."
  S3_ARGS=()
  [ -n "$S3_ACCESS_KEY" ] && S3_ARGS+=(--access_key="$S3_ACCESS_KEY")
  [ -n "$S3_SECRET_KEY" ] && S3_ARGS+=(--secret_key="$S3_SECRET_KEY")
  if [ -n "$S3_ENDPOINT" ]; then
    # Path-style URL on the custom host (MinIO/OpenCloud): s3cmd switches
    # off bucket-in-DNS when --host-bucket equals --host.
    S3_ARGS+=(--host="$S3_ENDPOINT" --host-bucket="$S3_ENDPOINT")
  fi
  # shellcheck disable=SC2068
  s3cmd sync "${S3_ARGS[@]}" --no-check-md5 --acl-private \
    "${BACKUP_DIR}/" "s3://${S3_BUCKET}/${S3_PREFIX}/"
fi

# ── Summary ───────────────────────────────────
echo ""
echo "✅ Backup complete!"
echo "   Directory: ${BACKUP_DIR}/"
ls -lh "${BACKUP_DIR}/"*"_${TIMESTAMP}.*" 2>/dev/null || true

# ── Retention (keep 7 days) ───────────────────
find "$BACKUP_DIR" -name "*_*.sql.gz" -type f -mtime +7 -delete 2>/dev/null || true
find "$BACKUP_DIR" -name "*_*.tar.gz" -type f -mtime +7 -delete 2>/dev/null || true
find "$BACKUP_DIR" -name "*.enc" -type f -mtime +7 -delete 2>/dev/null || true
echo "   Retention: Keeping last 7 days"

# ── Log ───────────────────────────────────────
echo "[$(date +'%Y-%m-%d %H:%M:%S')] Backup completed: ${TIMESTAMP}" >> "${BACKUP_DIR}/backup.log"
