# Improvement Ideas for taskfleet

Prioritized based on competitive analysis and practical impact.

## 🔴 Critical (High Impact, Medium Effort)

### 1. Web Dashboard / Real-Time Status UI
**Inspired by:** edict (16k⭐), omnigent (8k⭐)
**Problem:** CLI-only status blocks adoption by teams and non-technical users.
**Proposal:** Lightweight web UI (Go/Python HTTP server + SSE or periodic polling):
- Real-time task status board (ready/running/done/failed)
- Live agent output streams per task
- Error classifications and retry status
- Task detail view (scope, gate, logs)
- **Estimate:** 3-5 days

### 2. Cost Tracking and Token Usage
**Inspired by:** General gap across all competitors (first-mover opportunity)
**Problem:** No visibility into spend per task, worker, or run.
**Proposal:** Parse agent output for token counts, add model pricing table:
- Track tokens_in/tokens_out per task
- Calculate cost using pricing tables
- Aggregate: per-worker, per-run, per-day
- Budget limits and alerts
- **Estimate:** 2-3 days

### 3. Human-in-the-Loop (HITL) Gates
**Inspired by:** vortex-dispatch, omnigent
**Problem:** No way to pause for human review before merging risky changes.
**Proposal:** `review: true` flag on tasks → `pending_review` state → CLI/UI approval.
- **Estimate:** 2-3 days

## 🟡 Important (Medium Impact, Medium Effort)

### 4. Task Templates / Generators
**Problem:** Manual JSON entries. The 54-task courses project needed an ad-hoc Python script.
**Proposal:** Built-in template system with variable substitution and bulk generation.
- **Estimate:** 3-4 days

### 5. Smart Task → Worker Assignment
**Problem:** First-available-worker assignment wastes strong models on easy tasks.
**Proposal:** Optional `difficulty` + `required_capability` fields with scoring algorithm.
- **Estimate:** 2-3 days

### 6. Graceful Cancellation
**Problem:** No way to cancel a running task or the orchestrator.
**Proposal:** `taskfleet cancel --task ID` / `--all` + Ctrl+C handler.
- **Estimate:** 1-2 days

### 7. Incremental Resume (Event Sourcing)
**Inspired by:** vortex-dispatch, oh-my-agent
**Problem:** Crashes lose all in-flight work.
**Proposal:** Append-only event log + replay on restart.
- **Estimate:** 2-3 days

## 🟢 Nice to Have

### 8. Streaming Agent Output (2-3 days)
### 9. PR-Based Merge Instead of Direct Merge (3-4 days)
### 10. Distributed Execution (7-10 days)
### 11. Prompt Versioning and A/B Testing (3-4 days)
### 12. Environment Isolation Beyond Git (5-7 days)
### 13. Observability / OpenTelemetry (3-4 days)
### 14. Notification Integrations (Slack/Discord/webhook) (1-2 days)
### 15. Multi-Repo Support (3-4 days)

## ❄️ Long-Term / Experimental

### 16. LLM-Based Task Decomposition
### 17. Self-Improving Prompts
### 18. Cross-Task Context Sharing
