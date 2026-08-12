#!/usr/bin/env bash
# dispatch.sh — worker dispatch for taskfleet.
#
# Fills the worker prompt template with task details and runs `pi` headless
# inside the task's worktree. On retry, injects previous failure context so the
# agent can learn from its mistakes.

# shellcheck source=common.sh
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"
# shellcheck source=status.sh
. "$(dirname "${BASH_SOURCE[0]}")/status.sh"
# shellcheck source=worktree.sh
. "$(dirname "${BASH_SOURCE[0]}")/worktree.sh"
# shellcheck source=verify.sh
. "$(dirname "${BASH_SOURCE[0]}")/verify.sh"

# tf_render_prompt <task_id>  → prints filled prompt to stdout.
# Uses bash parameter expansion (NOT envsubst) so only our explicit {{VAR}}
# placeholders are replaced — avoids clobbering literal $1 or template syntax.
# On retry (attempts > 0), injects {{PREVIOUS_ERROR}} with the last failure's
# classified error + relevant log excerpt.
tf_render_prompt() {
  local id="$1" title engine section accept scope_block tmpl
  title="$(tf_task_field "$id" .title)"
  engine="$(tf_task_field "$id" .engine)"
  section="$(tf_task_field "$id" .section)"
  accept="$(tf_task_field "$id" .accept)"
  scope_block="$(tf_task_field "$id" '.scope[]' | sed 's/^/  /')"
  local plan_section="section $section"

  # Build error feedback block for retries
  local error_block=""
  local attempts
  attempts="$(tf_status_get "$id" .attempts 2>/dev/null || echo 0)"
  if [[ "$attempts" -gt 0 ]]; then
    local category summary last_error attempt_n
    category="$(tf_get_error_category "$id")"
    summary="$(tf_get_error_summary "$id")"
    last_error="$(tf_status_get "$id" .last_error)"
    attempt_n="$((attempts + 1))"

    # Build structured feedback
    error_block="
## ⚠️ RETRY — attempt $attempt_n (previous attempt failed)

**Error classification:** $category
**Summary:** $summary
**Gate output:** $last_error

The last error log from the acceptance gate is included below. Study it carefully
and fix the root cause. Do NOT repeat the same mistake.

\`\`\`
$(tf_error_snippet "$TF_LOG_DIR/$id.verify.log" 2>/dev/null | head -40)
\`\`\`

"
  fi

  tmpl="$(cat "$TF_PROMPT_DIR/worker.md")"
  tmpl="${tmpl//\{\{TASK_ID\}\}/$id}"
  tmpl="${tmpl//\{\{TASK_TITLE\}\}/$title}"
  tmpl="${tmpl//\{\{ENGINE\}\}/$engine}"
  tmpl="${tmpl//\{\{PLAN_SECTION\}\}/$plan_section}"
  tmpl="${tmpl//\{\{SCOPE_BLOCK\}\}/$scope_block}"
  # ACCEPT_COMMAND may contain && which breaks ${var//p/r}. Use sed.
  local accept_esc
  accept_esc="$(printf '%s' "$accept" | sed 's/[&\/]/\\&/g')"
  tmpl="$(printf '%s' "$tmpl" | sed "s/{{ACCEPT_COMMAND}}/$accept_esc/g")"
  tmpl="${tmpl//\{\{PREVIOUS_ERROR\}\}/$error_block}"
  printf '%s' "$tmpl"
}

# tf_dispatch_one <task_id> <worker_name>
#   Full lifecycle: worktree → render prompt → run pi → verify → status update.
#   Returns 0 on success (gate passed), non-zero on failure.
tf_dispatch_one() {
  local id="$1" worker="$2"
  local provider model dispatch_timeout log
  provider="$(tf_worker_field "$worker" .provider)"
  model="$(tf_worker_field "$worker" .model)"
  dispatch_timeout="$(tf_default dispatch_timeout_s)"; dispatch_timeout="${dispatch_timeout:-1800}"
  log="$TF_LOG_DIR/$id.dispatch.log"

  local branch="$TF_BRANCH_PREFIX/$id"
  local tmp
  tmp="$(mktemp)"
  jq --arg id "$id" --arg worker "$worker" --arg branch "$branch" \
    --arg now "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '.[$id].status="running" | .[$id].worker=$worker | .[$id].branch=$branch
     | .[$id].started_at=$now | .[$id].next_retry_at=null' \
    "$STATUS_JSON" > "$tmp"
  tf_locked_mv "$tmp" "$STATUS_JSON"

  tf_info "$id: dispatching to worker=$worker ($provider/$model)"

  # 1. Create worktree
  local wt
  wt="$(tf_worktree_create "$id")" || {
    tf_fail_task "$id" "worktree creation failed"
    return 1
  }

  # 2. Render prompt (includes error feedback on retries)
  local prompt_file="$TF_STATE_DIR/$id.prompt.md"
  tf_render_prompt "$id" > "$prompt_file"

  # 3. Run pi headless inside the worktree
  {
    echo "=== $id dispatch ==="
    echo "worker: $worker ($provider/$model)"
    echo "worktree: $wt"
    echo "branch: $branch"
    echo "started: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "attempt: $(tf_status_get "$id" .attempts)"
    echo "============================================"
  } > "$log"

  local rc=0
  (
    cd "$wt" || exit 127
    timeout "$dispatch_timeout" pi \
      --provider "$provider" \
      --model "$model" \
      -p "@$prompt_file"
  ) >> "$log" 2>&1 || rc=$?

  if [[ $rc -ne 0 ]]; then
    local reason
    if [[ $rc -eq 124 ]]; then reason="dispatch timed out after ${dispatch_timeout}s"
    else reason="pi exited $rc"; fi
    tf_warn "$id: $reason — attempting acceptance gate anyway (worker may have committed)"
  fi

  # 3b. No-op detection: check if the worker actually modified any scope files.
  #     If nothing changed, fail immediately rather than wasting gate time.
  local changed_files
  changed_files="$(cd "$wt" && git diff --name-only HEAD 2>/dev/null)"
  local untracked_files
  untracked_files="$(cd "$wt" && git ls-files --others --exclude-standard 2>/dev/null)"
  if [[ -z "$changed_files" && -z "$untracked_files" ]]; then
    tf_fail_task "$id" "no changes: LLM did not modify any files in scope (dispatch log: $TF_LOG_DIR/$id.dispatch.log)"
    # Write error.json for retry feedback
    mkdir -p "$TF_LOG_DIR"
    echo '{"category":"no_op","summary":"LLM produced no tool calls or file modifications","classified_at":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'"}' > "$TF_LOG_DIR/$id.error.json"
    tf_worktree_remove "$id" --force
    tf_worktree_delete_branch "$id"
    return 1
  fi
  tf_info "$id: worker modified $(echo "$changed_files $untracked_files" | wc -w) file(s)"

  # 4. Verify (acceptance gate) — now with error classification
  tf_status_set "$id" verifying
  local verdict
  verdict="$(tf_verify "$id" "$wt")" || true

  # 5. Advisory scope check
  tf_verify_scope "$id" "$wt" >>"$TF_LOG_DIR/$id.scope.log" 2>&1 || true

  # 6. Commit if worker forgot (best-effort) — only in-scope files
  if [[ "$verdict" == PASS* ]]; then
    (cd "$wt" && git add -A && git diff --cached --quiet || \
       git commit -q -m "feat($id): $(tf_task_field "$id" .title)" 2>/dev/null) || true
  fi

  # 7. Status update + merge
  if [[ "$verdict" == PASS* ]]; then
    if tf_worktree_merge "$id"; then
      tf_done_task "$id"
      tf_worktree_remove "$id"
      tf_worktree_delete_branch "$id"
      return 0
    else
      tf_fail_task "$id" "merge conflict (gate passed but main diverged)"
      tf_worktree_remove "$id" --force
      tf_worktree_delete_branch "$id"
      return 1
    fi
  else
    tf_fail_task "$id" "acceptance gate: $verdict"
    tf_worktree_remove "$id" --force
    tf_worktree_delete_branch "$id"
    return 1
  fi
}

# tf_dispatch_one_dryrun <task_id> <worker_name>
#   Prints what would happen without running pi. For testing.
tf_dispatch_one_dryrun() {
  local id="$1" worker="$2"
  local provider model accept
  provider="$(tf_worker_field "$worker" .provider)"
  model="$(tf_worker_field "$worker" .model)"
  accept="$(tf_task_field "$id" .accept)"
  local attempts
  attempts="$(tf_status_get "$id" .attempts 2>/dev/null || echo 0)"
  echo "DRY-RUN $id → worker=$worker ($provider/$model) (attempt $((attempts+1)))"
  echo "  accept: $accept"
  echo "  scope:  $(tf_task_field "$id" '.scope | length') path(s)"
  if [[ "$attempts" -gt 0 ]]; then
    echo "  RETRY: category=$(tf_get_error_category "$id") summary=$(tf_get_error_summary "$id")"
  fi
  echo "  prompt: $TF_STATE_DIR/$id.prompt.md"
  tf_render_prompt "$id" | head -5
  echo "  ..."
}
