#!/usr/bin/env bash
# verify.sh — acceptance-gate runner for taskfleet.
#
# Runs the task's `accept` command inside its worktree. Returns 0 iff green.
# Captures combined stdout+stderr to the task log.

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# tf_verify <task_id> <worktree_path>
# Echoes "PASS" or "FAIL: <reason>". Exit code mirrors pass/fail.
tf_verify() {
  local id="$1" wt="$2"
  local accept timeout_s log
  accept="$(tf_task_field "$id" .accept)"
  timeout_s="$(tf_default accept_timeout_s)"; timeout_s="${timeout_s:-600}"
  log="$TF_LOG_DIR/$id.verify.log"

  if [[ -z "$accept" || "$accept" == "null" ]]; then
    tf_warn "$id: no accept command defined — skipping gate (manual sign-off)"
    echo "SKIP: no accept command (manual task)"
    return 0
  fi

  tf_info "$id: running acceptance gate (${timeout_s}s): $accept"
  {
    echo "=== $id acceptance gate: $accept ==="
    echo "=== worktree: $wt ==="
    echo "=== started: $(date -u +%Y-%m-%dT%H:%M:%SZ) ==="
  } > "$log"

  # Run in the worktree via login shell (sources .profile/.bashrc).
  # TF_GATE_ENV (set in common.sh from env var) injects project-specific exports
  # like RUSTUP_TOOLCHAIN=nightly. Acceptance commands (cargo test, pnpm test,
  # pytest, etc.) must be on PATH already.
  local rc=0
  (
    cd "$wt" || exit 127
    timeout "$timeout_s" bash -lc "${TF_GATE_ENV:+export $TF_GATE_ENV; }$accept"
  ) >> "$log" 2>&1 || rc=$?

  if [[ $rc -eq 0 ]]; then
    tf_info "$id: gate PASS"
    echo "PASS"
    return 0
  elif [[ $rc -eq 124 ]]; then
    tf_error "$id: gate TIMEOUT after ${timeout_s}s"
    echo "FAIL: timeout after ${timeout_s}s"
    return 1
  else
    tf_error "$id: gate FAIL (exit $rc) — see $log"
    echo "FAIL: exit $rc (see $log)"
    return 1
  fi
}

# tf_verify_scope <task_id> <worktree_path>
#   Advisory check: did the worker edit only in-scope files? Prints warnings
#   for out-of-scope edits but does NOT fail the task (the acceptance gate is
#   authoritative). Helps catch scope drift.
tf_verify_scope() {
  local id="$1" wt="$2"
  local log="$TF_LOG_DIR/$id.scope.log"
  {
    echo "=== $id scope check ==="
  } > "$log"

  # files changed vs main on the task branch
  local changed
  changed="$(cd "$wt" && git diff --name-only main...HEAD 2>/dev/null)" || true
  [[ -z "$changed" ]] && { echo "none"; return 0; }

  # allowed globs
  local allowed
  allowed="$(tf_task_field "$id" '.scope[]')" || true

  local violations=()
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    local ok=0
    # match against scope globs (prefix or shell glob)
    while IFS= read -r pat; do
      [[ -z "$pat" ]] && continue
      # treat trailing / as "directory and below"
      case "$pat" in
        */) [[ "$f" == "$pat"* ]] && ok=1 ;;
        *)  [[ "$f" == "$pat" ]] && ok=1 ;;
      esac
    done <<< "$allowed"
    [[ $ok -eq 0 ]] && violations+=("$f")
  done <<< "$changed"

  if [[ ${#violations[@]} -gt 0 ]]; then
    {
      echo "OUT-OF-SCOPE edits (advisory, non-blocking):"
      printf '  %s\n' "${violations[@]}"
    } >> "$log"
    tf_warn "$id: ${#violations[@]} out-of-scope file(s) edited — see $log"
    printf '%s\n' "${violations[@]}"
  else
    echo "all in-scope" >> "$log"
  fi
}
