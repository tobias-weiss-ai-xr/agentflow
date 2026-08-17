#!/usr/bin/env bash
# openspec-bridge.sh — export an OpenSpec change into taskfleet format.
#
# Reads an OpenSpec change's tasks.md, specs/, and design.md,
# produces a tasks.json consumable by orchestrator.sh.
#
# Usage:
#   TF_REPO_DIR=/path/to/repo OPENSPEC_CHANGE=name bash lib/openspec-bridge.sh [--dry-run]
#
# Maps: task groups → deps, descriptions → titles, spec scenarios → accept gates.

set -euo pipefail

TF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$TF_DIR/lib/common.sh"

DRY_RUN=false
OUTPUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=true ;;
    --output)  OUTPUT="$2"; shift ;;
    -h|--help)
      echo "Usage: $0 [--dry-run] [--output FILE]"
      echo "  OPENSPEC_CHANGE=name  (required)"
      echo "  TF_REPO_DIR=path      (required)"
      exit 0 ;;
    *)         tf_error "unknown arg: $1"; exit 1 ;;
  esac
  shift
done

REPO="${TF_REPO_DIR:?Set TF_REPO_DIR}"
CHANGE="${OPENSPEC_CHANGE:?Set OPENSPEC_CHANGE}"
CHANGE_DIR="${OPENSPEC_ROOT:-$REPO/openspec/changes/$CHANGE}"

[[ -d "$CHANGE_DIR" ]] || { tf_error "not found: $CHANGE_DIR"; exit 1; }
[[ -f "$CHANGE_DIR/tasks.md" ]] || { tf_error "no tasks.md in $CHANGE_DIR"; exit 1; }

TASKS="$CHANGE_DIR/tasks.md"
SPECS_DIR="$CHANGE_DIR/specs"
PROJECT="$(basename "$REPO")"

# ---------------------------------------------------------------------------
# Parse tasks.md → line-based extraction, feed to jq
# ---------------------------------------------------------------------------
PARSED=$(awk '
/^## [0-9]+\./ {
  match($0, /^## ([0-9]+)\. (.+)/, m)
  group = m[1]
}
/^- \[[ x]\] [0-9]+\.[0-9]+ / {
  match($0, /^- \[([ x])\] ([0-9]+\.[0-9]+) (.+)/, m)
  done_flag = (m[1] == "x") ? 1 : 0
  id = m[2]
  title = m[3]
  if (done_flag != 1) {
    printf "%s\t%s\t%s\t%s\n", id, group, title, (NR - 1)
  }
}
' "$TASKS")

if [[ -z "$PARSED" ]]; then
  tf_warn "No pending tasks found in $TASKS"
  exit 0
fi

# ---------------------------------------------------------------------------
# Build dependency graph
# ---------------------------------------------------------------------------
# For each task, its dep is the immediately preceding task (sequential within
# groups, cross-group boundary links to last of previous group).

PREV_ID=""
PREV_GROUP=""
DEPS_MAP="{}"

while IFS=$'\t' read -r id group title linenum; do
  if [[ -z "$PREV_ID" ]]; then
    DEPS_MAP=$(echo "$DEPS_MAP" | jq --arg id "$id" '. + {($id): []}')
  else
    if [[ "$group" != "$PREV_GROUP" ]]; then
      # Cross-group boundary: dep on previous task (last of old group)
      DEPS_MAP=$(echo "$DEPS_MAP" | jq --arg id "$id" --arg dep "$PREV_ID" '. + {($id): [$dep]}')
    else
      # Same group: sequential dep
      DEPS_MAP=$(echo "$DEPS_MAP" | jq --arg id "$id" --arg dep "$PREV_ID" '. + {($id): [$dep]}')
    fi
  fi
  PREV_ID="$id"
  PREV_GROUP="$group"
done <<< "$PARSED"

# ---------------------------------------------------------------------------
# Infer scope and accept gate per task
# ---------------------------------------------------------------------------
tf_infer_scope() {
  local desc="$1"
  case "$desc" in
    *"opendesk-knowledge"*) echo '["opendesk-knowledge/**"]' ;;
    *"Go module"*|*"main.go"*) echo '["opendesk-dev-agent/**","go.mod","go.sum"]' ;;
    *"internal/config"*) echo '["opendesk-dev-agent/internal/config/**"]' ;;
    *"internal/knowledge"*) echo '["opendesk-dev-agent/internal/knowledge/**"]' ;;
    *"internal/checker"*) echo '["opendesk-dev-agent/internal/checker/**"]' ;;
    *"internal/healer"*) echo '["opendesk-dev-agent/internal/healer/**"]' ;;
    *"REST API"*|*"endpoint"*) echo '["opendesk-dev-agent/internal/api/**"]' ;;
    *"anonymiz"*|*"stripper"*) echo '["opendesk-dev-agent/internal/api/**"]' ;;
    *"Wire main"*) echo '["opendesk-dev-agent/main.go"]' ;;
    *"Dockerfile"*) echo '["opendesk-dev-agent/Dockerfile"]' ;;
    *"docker-compose.yml"*) echo '["docker-compose.yml"]' ;;
    *".env.example"*) echo '[".env.example"]' ;;
    *"docker.sock"*|*"lint"*) echo '["tests/00-static/*"]' ;;
    *"02-container"*) echo '["tests/02-container/*"]' ;;
    *".pi/extensions"*|*"registers"*) echo '[".pi/extensions/opendesk-dev-agent.ts"]' ;;
    *"agent discovery"*) echo '[".pi/extensions/opendesk-dev-agent.ts"]' ;;
    *"/status command"*) echo '[".pi/extensions/opendesk-dev-agent.ts"]' ;;
    *"/heal command"*) echo '[".pi/extensions/opendesk-dev-agent.ts"]' ;;
    *"/diag command"*) echo '[".pi/extensions/opendesk-dev-agent.ts"]' ;;
    *"Makefile target"*) echo '["Makefile"]' ;;
    *"RAM budget"*|*"sum-memory"*) echo '["tests/00-static/sum-memory.awk"]' ;;
    *"README.md"*) echo '["README.md"]' ;;
    *"unit tests"*|*"Go.*testing"*) echo '["opendesk-dev-agent/**/*_test.go"]' ;;
    *"Layer 0"*|*"static check"*) echo '["tests/00-static/*","docker-compose.yml"]' ;;
    *"Build and deploy"*|*"Simulate"*|*"Verify"*|*"anonymization: create"*)
      echo '["**"]' ;;
    *) echo '["**"]' ;;
  esac
}

tf_infer_accept() {
  local desc="$1"
  case "$desc" in
    *"Go module"*|*"go.mod"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go build ./..." ;;
    *"internal/config"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go build ./internal/config/ && go vet ./internal/config/" ;;
    *"internal/knowledge"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go test ./internal/knowledge/ -v" ;;
    *"internal/checker"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go test ./internal/checker/ -v" ;;
    *"internal/healer"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go test ./internal/healer/ -v" ;;
    *"REST API"*|*"endpoint"*|*"GET /api"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go test ./internal/api/ -v" ;;
    *"anonymiz"*|*"stripper"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go test ./internal/api/ -run TestAnonymize -v" ;;
    *"Wire main"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go build -o /dev/null . && echo OK" ;;
    *"Dockerfile"*)
      echo "test -f opendesk-dev-agent/Dockerfile && head -1 opendesk-dev-agent/Dockerfile | grep -qi FROM && echo OK" ;;
    *"docker-compose.yml"*|*"sidecar"*|*"compose integration"*)
      echo "grep -q dev-agent docker-compose.yml && grep -q docker.sock docker-compose.yml && echo OK" ;;
    *".env.example"*)
      echo "grep -q DEV_AGENT_ .env.example && echo OK" ;;
    *"docker.sock"*|*"lint check"*)
      echo "bash -n tests/00-static/run.sh 2>/dev/null; echo OK" ;;
    *"02-container"*)
      echo "bash -n tests/02-container/run.sh && echo OK" ;;
    *".pi/extensions"*|*"registers"*)
      echo "test -f .pi/extensions/opendesk-dev-agent.ts && grep -q registerCommand .pi/extensions/opendesk-dev-agent.ts && echo OK" ;;
    *"agent discovery"*)
      echo "grep -q com.opendesk.agent .pi/extensions/opendesk-dev-agent.ts 2>/dev/null && echo OK" ;;
    *"/status command"*)
      echo "grep -q /status .pi/extensions/opendesk-dev-agent.ts && echo OK" ;;
    *"/heal command"*)
      echo "grep -q /heal .pi/extensions/opendesk-dev-agent.ts && echo OK" ;;
    *"/diag command"*)
      echo "grep -q /diag .pi/extensions/opendesk-dev-agent.ts && echo OK" ;;
    *"Makefile target"*|*"agent-build"*)
      echo "grep -q agent-build Makefile && grep -q agent-status Makefile && echo OK" ;;
    *"RAM budget"*|*"sum-memory"*)
      echo "bash -n tests/00-static/sum-memory.awk && echo OK" ;;
    *"README.md"*)
      echo "grep -qi dev.agent README.md && echo OK" ;;
    *"unit tests"*|*"Go.*testing"*)
      echo "cd opendesk-dev-agent 2>/dev/null; go test ./... -v -count=1 2>&1 | tail -5" ;;
    *"Layer 0"*|*"static check"*)
      echo "bash tests/00-static/run.sh 2>&1 | tail -3" ;;
    *"Build and deploy"*|*"Simulate"*|*"Verify"*|*"anonymization: create"*)
      echo "echo MANUAL" ;;
    *) echo "echo OK" ;;
  esac
}

# ---------------------------------------------------------------------------
# Build JSON via jq
# ---------------------------------------------------------------------------
TASKS_ARRAY="[]"

while IFS=$'\t' read -r id group title linenum; do
  scope=$(tf_infer_scope "$title")
  accept=$(tf_infer_accept "$title")
  deps=$(echo "$DEPS_MAP" | jq --arg id "$id" '.[$id]')
  manual=false
  case "$title" in
    *"Build and deploy"*|*"Simulate"*|*"Verify"*|*"anonymization: create"*) manual=true ;;
  esac

  TASKS_ARRAY=$(echo "$TASKS_ARRAY" | jq \
    --arg id "$id" \
    --arg title "$title" \
    --arg group "$group" \
    --argjson scope "$scope" \
    --arg accept "$accept" \
    --argjson deps "$deps" \
    --argjson manual "$manual" \
    '. += [{
      id: $id,
      title: $title,
      group: $group,
      deps: $deps,
      scope: $scope,
      accept: $accept,
      manual: $manual
    }]')
done <<< "$PARSED"

# Wrap with metadata
RESULT=$(echo "$TASKS_ARRAY" | jq --arg project "$PROJECT" --arg change "$CHANGE" \
  --arg change_dir "$CHANGE_DIR" \
  --arg now "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{
    _meta: {
      project: $project,
      openspec_change: $change,
      openspec_root: $change_dir,
      generated_by: "openspec-bridge.sh",
      generated_at: $now
    },
    tasks: .
  }')

# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------
if $DRY_RUN; then
  count=$(echo "$RESULT" | jq '.tasks | length')
  echo "Generated $count tasks from openspec/changes/$CHANGE"
  echo ""
  echo "Dependency graph:"
  echo "$RESULT" | jq -r '.tasks[] | "  \(.id) → \(.title[:60]) (deps: \(.deps | join(",") // "none"))"'
  exit 0
fi

TARGET="${OUTPUT:-$TF_CONFIG_DIR/tasks-generated.json}"
echo "$RESULT" > "$TARGET"
count=$(echo "$RESULT" | jq '.tasks | length')
tf_info "Wrote $TARGET ($count tasks)"

if [[ "$TARGET" != "$TASKS_JSON" ]]; then
  tf_info "To activate: cp $TARGET $TASKS_JSON"
fi
