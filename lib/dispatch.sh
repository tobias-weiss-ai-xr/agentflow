#!/usr/bin/env bash
# dispatch.sh — worker dispatch for taskfleet.
#
# Fills the worker prompt template with task details and runs `pi` headless
# inside the task's worktree. Then runs the acceptance gate and updates status.

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
# placeholders are replaced — avoids clobbering literal $WO_TIPTAP / $1 in the
# template body and needs no escaping.
tf_render_prompt() {
  local id="$1" title engine section accept scope_block tmpl
  title="$(tf_task_field "$id" .title)"
  engine="$(tf_task_field "$id" .engine)"
  section="$(tf_task_field "$id" .section)"
  accept="$(tf_task_field "$id" .accept)"
  scope_block="$(tf_task_field "$id" '.scope[]' | sed 's/^/  /')"
  local plan_section="section $section of"
  tmpl="$(cat "$TF_PROMPT_DIR/worker.md")"
  tmpl="${tmpl//\{\{TASK_ID\}\}/$id}"
  tmpl="${tmpl//\{\{TASK_TITLE\}\}/$title}"
  tmpl="${tmpl//\{\{ENGINE\}\}/$engine}"
  tmpl="${tmpl//\{\{PLAN_SECTION\}\}/$plan_section}"
  tmpl="${tmpl//\{\{SCOPE_BLOCK\}\}/$scope_block}"
  tmpl="${tmpl//\{\{ACCEPT_COMMAND\}\}/$accept}"
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
  # Direct jq write for running state (carries worker + branch). tf_status_set's
  # 3rd-arg form only allows raw jq referencing $id/$status/$now, so we write
  # explicitly here to keep the richer payload.
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

  # 2. Render prompt
  local prompt_file="$TF_STATE_DIR/$id.prompt.md"
  tf_render_prompt "$id" > "$prompt_file"

  # 3. Run pi headless inside the worktree
  {
    echo "=== $id dispatch ==="
    echo "worker: $worker ($provider/$model)"
    echo "worktree: $wt"
    echo "branch: $branch"
    echo "started: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "============================================"
  } > "$log"

  local rc=0
  (
    cd "$wt" || exit 127
    # pi headless: -p reads prompt from argument. We pass the prompt file via @.
    # shellcheck disable=SC2086
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
    # fall through to verify: worker may have committed before the timeout/error
  fi

  # 4. Verify (acceptance gate)
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
      tf_worktree_remove "$id"        # remove worktree FIRST (branch can't be deleted while checked out)
      tf_worktree_delete_branch "$id"
      return 0
    else
      tf_fail_task "$id" "merge conflict (gate passed but main diverged)"
      tf_worktree_remove "$id" --force
      tf_worktree_delete_branch "$id"   # delete so retry branches fresh from current main
      return 1
    fi
  else
    tf_fail_task "$id" "acceptance gate: $verdict"
    tf_worktree_remove "$id" --force
    tf_worktree_delete_branch "$id"   # fresh retry from current main
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
  echo "DRY-RUN $id → worker=$worker ($provider/$model)"
  echo "  accept: $accept"
  echo "  scope:  $(tf_task_field "$id" '.scope | length') path(s)"
  echo "  prompt: $TF_STATE_DIR/$id.prompt.md"
  tf_render_prompt "$id" | head -5
  echo "  ..."
}
