#!/usr/bin/env bash
# constitution.sh — deterministic pre-merge checks.
#
# Inspired by moe-sovereign's sovereign-constitution.yaml — machine-checkable
# rules evaluated against the diff before merge. No LLM involved.
# Rules: no secrets in diff, no files outside declared scope, no debug code.
#
# Configuration: $TF_CONFIG_DIR/constitution.yaml (optional, simple key:value)
# Env: TF_CONSTITUTION=1 to enable (default: off)

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# tf_constitution_check <task_id> <worktree_path>
# Runs deterministic checks on the diff. Returns 0 if all pass, 1 if any fail.
# Prints violations to stderr.
tf_constitution_check() {
  local id="$1" wt="$2"
  [[ "${TF_CONSTITUTION:-0}" == "1" ]] || return 0
  local violations=0
  local changed
  changed="$(cd "$wt" && git diff --name-only "${TF_BASE_BRANCH}"...HEAD 2>/dev/null)" || true
  [[ -z "$changed" ]] && return 0

  # 1. No secrets/credentials in diff
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    # Check for common secret patterns in the diff
    local diff_content
    diff_content="$(cd "$wt" && git diff "${TF_BASE_BRANCH}"...HEAD -- "$f" 2>/dev/null)"
    if echo "$diff_content" | grep -qP '(?i)(api[_-]?key|secret|password|token|private[_-]?key)\s*[=:]\s*["\x27]?[A-Za-z0-9+/]{20,}'; then
      # Exclude common false positives: env var references, placeholder text
      if ! echo "$diff_content" | grep -qP '(?i)(\$\{|getenv|process\.env|os\.environ|placeholder|example|redacted|REDACTED)'; then
        tf_warn "$id: constitution violation — potential secret in $f"
        violations=$((violations + 1))
      fi
    fi
  done <<< "$changed"

  # 2. No files outside declared scope (advisory — already checked by tf_verify_scope)
  local allowed
  allowed="$(tf_task_field "$id" '.scope[]' 2>/dev/null || echo "")"
  if [[ -n "$allowed" ]]; then
    while IFS= read -r f; do
      [[ -z "$f" ]] && continue
      local ok=0
      while IFS= read -r pat; do
        [[ -z "$pat" ]] && continue
        case "$pat" in
          */) [[ "$f" == "$pat"* ]] && ok=1 ;;
          *)  [[ "$f" == "$pat" ]] && ok=1 ;;
        esac
      done <<< "$allowed"
      [[ $ok -eq 0 ]] && {
        tf_warn "$id: constitution violation — out-of-scope file: $f"
        violations=$((violations + 1))
      }
    done <<< "$changed"
  fi

  # 3. No debug code (console.log, print, debugger, breakpoint)
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    local diff_content
    diff_content="$(cd "$wt" && git diff "${TF_BASE_BRANCH}"...HEAD -- "$f" 2>/dev/null)"
    if echo "$diff_content" | grep -qP '^\+.*\b(console\.log|print\(|debugger|breakpoint\(\)|pdb\.set_trace)\b'; then
      tf_warn "$id: constitution violation — debug code in $f"
      violations=$((violations + 1))
    fi
  done <<< "$changed"

  [[ $violations -eq 0 ]] && return 0
  tf_error "$id: constitution check failed ($violations violation(s))"
  return 1
}
