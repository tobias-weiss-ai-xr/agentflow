#!/bin/bash

# run-scs-security.sh - Run SCS K3s Security Hardening tasks via Taskfleet
# Usage: ./run-scs-security.sh [command] [options]
#
# Commands:
#   status          Show current status of security tasks
#   dry-run         Show what would be dispatched (no changes)
#   run             Run all security tasks
#   run-p1          Run only Priority 1 (CRITICAL) tasks
#   run-p1-p2       Run Priority 1 + 2 (CRITICAL + HIGH) tasks
#   run-task <id>   Run specific task by ID
#   setup           Setup SCS security configuration
#   restore         Restore original configuration
#   help            Show this help

set -euo pipefail

# Configuration
SCS_REPO_DIR="/home/weissto_local/git/scs"
TF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_DIR="$TF_DIR/config"
PROMPTS_DIR="$TF_DIR/prompts-scs"

# Original configs (for restoration)
ORIG_WORKERS="$CONFIG_DIR/workers.json"
ORIG_TASKS="$CONFIG_DIR/tasks.json"

# SCS security configs
SCS_WORKERS="$CONFIG_DIR/workers-scs.json"
SCS_TASKS="$CONFIG_DIR/tasks-scs-security.json"

# Backups
BACKUP_WORKERS="$CONFIG_DIR/workers.json.scs-backup"
BACKUP_TASKS="$CONFIG_DIR/tasks.json.scs-backup"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_header() {
    echo -e "\n${CYAN}=== $1 ===${NC}"
}

# Function to backup original configs
backup_configs() {
    if [ ! -f "$BACKUP_WORKERS" ] && [ -f "$ORIG_WORKERS" ]; then
        cp "$ORIG_WORKERS" "$BACKUP_WORKERS"
        log_info "Backed up workers.json to workers.json.scs-backup"
    fi
    if [ ! -f "$BACKUP_TASKS" ] && [ -f "$ORIG_TASKS" ]; then
        cp "$ORIG_TASKS" "$BACKUP_TASKS"
        log_info "Backed up tasks.json to tasks.json.scs-backup"
    fi
}

# Function to setup SCS security configuration
setup_scs_config() {
    log_header "Setting up SCS Security Configuration"
    
    backup_configs
    
    if [ -f "$SCS_WORKERS" ]; then
        cp "$SCS_WORKERS" "$ORIG_WORKERS"
        log_info "Copied workers-scs.json to workers.json"
    else
        log_error "SCS workers config not found: $SCS_WORKERS"
        return 1
    fi
    
    if [ -f "$SCS_TASKS" ]; then
        cp "$SCS_TASKS" "$ORIG_TASKS"
        log_info "Copied tasks-scs-security.json to tasks.json"
    else
        log_error "SCS tasks config not found: $SCS_TASKS"
        return 1
    fi
    
    # Verify files
    if [ -f "$ORIG_WORKERS" ] && [ -f "$ORIG_TASKS" ]; then
        log_info "SCS Security configuration is active"
        
        # Show task count
        local task_count=$(jq '.tasks | length' "$ORIG_TASKS" 2>/dev/null || echo "unknown")
        log_info "Loaded $task_count security tasks"
        
        # Show priority breakdown
        local p1=$(jq '.tasks | map(select(.priority == "CRITICAL")) | length' "$ORIG_TASKS" 2>/dev/null || echo "0")
        local p2=$(jq '.tasks | map(select(.priority == "HIGH")) | length' "$ORIG_TASKS" 2>/dev/null || echo "0")
        local p3=$(jq '.tasks | map(select(.priority == "MEDIUM")) | length' "$ORIG_TASKS" 2>/dev/null || echo "0")
        local p4=$(jq '.tasks | map(select(.priority == "LOW")) | length' "$ORIG_TASKS" 2>/dev/null || echo "0")
        
        log_info "Priority 1 (CRITICAL): $p1 tasks"
        log_info "Priority 2 (HIGH): $p2 tasks"
        log_info "Priority 3 (MEDIUM): $p3 tasks"
        log_info "Priority 4 (LOW): $p4 tasks"
        
        return 0
    else
        log_error "Failed to setup SCS configuration"
        return 1
    fi
}

# Function to restore original configuration
restore_config() {
    log_header "Restoring Original Configuration"
    
    if [ -f "$BACKUP_WORKERS" ]; then
        cp "$BACKUP_WORKERS" "$ORIG_WORKERS"
        log_info "Restored workers.json from backup"
    fi
    
    if [ -f "$BACKUP_TASKS" ]; then
        cp "$BACKUP_TASKS" "$ORIG_TASKS"
        log_info "Restored tasks.json from backup"
    fi
    
    log_info "Original configuration restored"
}

# Function to show status
show_status() {
    log_header "SCS Security Hardening Status"
    
    # Check if TF_REPO_DIR is set
    if [ -z "${TF_REPO_DIR:-}" ]; then
        export TF_REPO_DIR="$SCS_REPO_DIR"
        log_info "Set TF_REPO_DIR to $SCS_REPO_DIR"
    fi
    
    # Run orchestrator status
    if [ -f "$TF_DIR/orchestrator.sh" ]; then
        cd "$TF_DIR"
        ./orchestrator.sh --status --config-url "$CONFIG_DIR/tasks-scs-security.json" 2>&1 | grep -v "^$" || true
    else
        log_error "orchestrator.sh not found in $TF_DIR"
        return 1
    fi
}

# Function to run dry run
run_dry_run() {
    log_header "Dry Run - SCS Security Tasks"
    
    if [ -z "${TF_REPO_DIR:-}" ]; then
        export TF_REPO_DIR="$SCS_REPO_DIR"
        log_info "Set TF_REPO_DIR to $SCS_REPO_DIR"
    fi
    
    cd "$TF_DIR"
    ./orchestrator.sh --dry-run --config-url "$CONFIG_DIR/tasks-scs-security.json"
}

# Function to run all tasks
run_all() {
    log_header "Running ALL SCS Security Tasks"
    log_warn "This will execute ALL 20 security hardening tasks!"
    
    read -p "Are you sure? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        log_info "Aborted. Use specific commands like 'run-p1' for targeted execution."
        return 0
    fi
    
    if [ -z "${TF_REPO_DIR:-}" ]; then
        export TF_REPO_DIR="$SCS_REPO_DIR"
        log_info "Set TF_REPO_DIR to $SCS_REPO_DIR"
    fi
    
    cd "$TF_DIR"
    ./orchestrator.sh --config-url "$CONFIG_DIR/tasks-scs-security.json"
}

# Function to run Priority 1 tasks
run_p1() {
    log_header "Running Priority 1 (CRITICAL) Tasks"
    
    if [ -z "${TF_REPO_DIR:-}" ]; then
        export TF_REPO_DIR="$SCS_REPO_DIR"
        log_info "Set TF_REPO_DIR to $SCS_REPO_DIR"
    fi
    
    cd "$TF_DIR"
    # Get P1 task IDs
    local p1_tasks=$(jq -r '.tasks[] | select(.priority == "CRITICAL") | .id' "$CONFIG_DIR/tasks-scs-security.json" 2>/dev/null || echo "")
    
    if [ -z "$p1_tasks" ]; then
        log_error "No Priority 1 tasks found in configuration"
        return 1
    fi
    
    log_info "Running Priority 1 tasks: $p1_tasks"
    
    # For now, run orchestrator which will process all ready tasks
    # P1 tasks have no deps, so they'll run first
    ./orchestrator.sh --config-url "$CONFIG_DIR/tasks-scs-security.json"
}

# Function to run Priority 1 + 2 tasks
run_p1_p2() {
    log_header "Running Priority 1 + 2 (CRITICAL + HIGH) Tasks"
    log_warn "This will execute CRITICAL and HIGH priority tasks."
    
    read -p "Are you sure? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        log_info "Aborted."
        return 0
    fi
    
    if [ -z "${TF_REPO_DIR:-}" ]; then
        export TF_REPO_DIR="$SCS_REPO_DIR"
        log_info "Set TF_REPO_DIR to $SCS_REPO_DIR"
    fi
    
    cd "$TF_DIR"
    ./orchestrator.sh --config-url "$CONFIG_DIR/tasks-scs-security.json"
}

# Function to run specific task
run_task() {
    local task_id="$1"
    
    if [ -z "$task_id" ]; then
        log_error "Task ID not provided. Usage: run-task <task-id>"
        return 1
    fi
    
    log_header "Running Specific Task: $task_id"
    
    if [ -z "${TF_REPO_DIR:-}" ]; then
        export TF_REPO_DIR="$SCS_REPO_DIR"
        log_info "Set TF_REPO_DIR to $SCS_REPO_DIR"
    fi
    
    cd "$TF_DIR"
    ./orchestrator.sh --task "$task_id" --config-url "$CONFIG_DIR/tasks-scs-security.json"
}

# Function to show help
show_help() {
    cat << 'EOF'
SCS K3s Security Hardening - Taskfleet Runner
===============================================

Usage: ./run-scs-security.sh [command] [options]

Commands:
  setup           Setup SCS security configuration (copies workers-scs.json and tasks-scs-security.json)
  restore         Restore original Taskfleet configuration
  status          Show current status of security tasks
  dry-run         Show what would be dispatched (no changes)
  run             Run all security tasks
  run-p1          Run only Priority 1 (CRITICAL) tasks
  run-p1-p2       Run Priority 1 + 2 (CRITICAL + HIGH) tasks
  run-task <id>   Run specific task by ID (e.g., P1-GRF-001)
  list-tasks      List all security tasks
  list-p1         List Priority 1 tasks
  help            Show this help

Examples:
  # Setup and check status
  ./run-scs-security.sh setup
  ./run-scs-security.sh status

  # Run Critical tasks
  ./run-scs-security.sh run-p1

  # Run specific task
  ./run-scs-security.sh run-task P1-GRF-001

  # Dry run to see what would happen
  ./run-scs-security.sh dry-run

  # Restore original config when done
  ./run-scs-security.sh restore

Priority Levels:
  🔴 P1 - CRITICAL: Immediate action required (4 tasks)
  🟡 P2 - HIGH:     This week (8 tasks)
  🟢 P3 - MEDIUM:   This month (5 tasks)
  🔵 P4 - LOW:      Future (4 tasks)

Environment Variables:
  TF_REPO_DIR      SCS repository directory (default: /home/weissto_local/git/scs)
  TF_DIR           Taskfleet directory (auto-detected)
  TF_MAX_PARALLEL  Maximum parallel tasks (default: number of enabled workers)

Notes:
  - Always run 'setup' before starting security tasks
  - Use 'status' to check progress
  - Use 'dry-run' to preview changes
  - Critical tasks (P1) should be run in maintenance window
  - Run 'restore' to return to original Taskfleet configuration

For more information, see: SECURITY-SCS.md

EOF
}

# Function to list all tasks
list_tasks() {
    log_header "All SCS Security Tasks"
    
    if [ ! -f "$SCS_TASKS" ]; then
        log_error "SCS tasks file not found: $SCS_TASKS"
        return 1
    fi
    
    echo
    echo "$(printf '%-15s %-50s % -10s' 'TASK ID' 'TITLE' 'PRIORITY')"
    echo "$(printf '%-15s %-50s % -10s' '--------' '-----')"
    
    jq -r '.tasks[] | "\(.id) \(.title) \(.priority)"' "$SCS_TASKS" 2>/dev/null | \
        while read -r id title priority; do
            # Color based on priority
            case "$priority" in
                CRITICAL) color="$RED" ;;
                HIGH) color="$YELLOW" ;;
                MEDIUM) color="$GREEN" ;;
                LOW) color="$BLUE" ;;
                *) color="$NC" ;;
            esac
            printf "${color}%-15s %-50s % -10s${NC}\n" "$id" "$title" "$priority"
        done
    
    echo
    local total=$(jq '.tasks | length' "$SCS_TASKS" 2>/dev/null || echo "0")
    log_info "Total: $total tasks"
}

# Function to list Priority 1 tasks
list_p1() {
    log_header "Priority 1 (CRITICAL) Tasks"
    
    if [ ! -f "$SCS_TASKS" ]; then
        log_error "SCS tasks file not found: $SCS_TASKS"
        return 1
    fi
    
    echo
    echo "$(printf '%-15s %-50s' 'TASK ID' 'TITLE')"
    echo "$(printf '%-15s %-50s' '--------' '-----')"
    
    jq -r '.tasks[] | select(.priority == "CRITICAL") | "\(.id) \(.title)"' "$SCS_TASKS" 2>/dev/null | \
        while read -r id title; do
            printf "${RED}%-15s %-50s${NC}\n" "$id" "$title"
        done
    
    echo
    local count=$(jq '.tasks | map(select(.priority == "CRITICAL")) | length' "$SCS_TASKS" 2>/dev/null || echo "0")
    log_info "Total P1 tasks: $count"
}

# Main script logic
case "${1:-help}" in
    setup)
        setup_scs_config
        ;;
    restore)
        restore_config
        ;;
    status)
        show_status
        ;;
    dry-run|dryrun|dry)
        run_dry_run
        ;;
    run)
        run_all
        ;;
    run-p1|runp1|p1)
        run_p1
        ;;
    run-p1-p2|runp1p2|p1p2)
        run_p1_p2
        ;;
    run-task|runtask)
        run_task "${2:-}"
        ;;
    list-tasks|list|tasks)
        list_tasks
        ;;
    list-p1|p1-tasks)
        list_p1
        ;;
    help|--help|-h)
        show_help
        ;;
    *)
        log_error "Unknown command: $1"
        echo
        show_help
        exit 1
        ;;
esac

exit 0
