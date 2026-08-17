#!/bin/bash
# Watchdog wrapper for taskfleet orchestrator (mailserver project).
# Restarts the orchestrator if it dies (observed: process vanishes without
# error after ~2 min in this sandbox). Each restart recovers dead runstate.
set -u
cd /home/weissto_local/git/agentflow
export TF_REPO_DIR=/home/weissto_local/git/next-mailserver
export TF_TASKS_JSON=/home/weissto_local/git/agentflow/config/tasks-mailserver.json
export TF_PROMPT_DIR=/home/weissto_local/git/agentflow/prompts-mail
# Isolated state dir so concurrent runs (e.g. opencode tasks) don't clash
export TF_STATE_DIR=/home/weissto_local/git/agentflow/state-mail

MAX_RESTARTS=30
restarts=0
while true; do
  ./orchestrator.sh
  rc=$?
  echo "$(date -u +%H:%M:%S) orchestrator exited rc=$rc (restarts=$restarts)" >> /tmp/tf-watchdog.log
  # 0 = ALL TASKS DONE; 1 could be graceful shutdown/error
  if [[ $rc -eq 0 ]]; then
    echo "$(date -u +%H:%M:%S) ALL DONE — watchdog stopping" >> /tmp/tf-watchdog.log
    break
  fi
  restarts=$((restarts + 1))
  if [[ $restarts -ge $MAX_RESTARTS ]]; then
    echo "$(date -u +%H:%M:%S) max restarts reached — giving up" >> /tmp/tf-watchdog.log
    break
  fi
  sleep 10
done
