# Ranked Learnings for taskfleet

Analysis of 204 discovered items (10 high-relevance GitHub repos, ~130 arXiv papers, 17 seeded repos).
Each learning is scored by **relevance × actionability × competitive impact**.

---

## 🔴 Tier S — Steal These (Highest Priority)

### S1. amux: Web Dashboard as First-Class Citizen
**Source:** mixpeek/amux (⭐346, TypeScript+Rust)
**Relevance:** ★★★★★ | **Actionability:** ★★★★☆ | **Impact:** ★★★★★

amux is the most complete reference implementation for what taskfleet lacks most. Key features:
- **Kanban board** with atomic task claiming, status gates (`done` ≠ `verified`)
- **Web dashboard** (embedded SPA in Rust binary, no npm needed)
- **Real-time SSE** + delta sync for live updates
- **Per-scope memory and environment** (global → group → worker layers)
- **Inter-worker messaging** — agents can talk to each other
- **SQLite-backed** — single-writer event journal
- **Self-healing recovery** — automatic restart of crashed sessions
- **iOS app** for monitoring from your phone
- **Daily log sweep** — automated anomaly detection via scheduled LLM review

**What taskfleet should steal:**
1. The **8-primitives** design (board, workers, schedulers, filesystem, groups, memories, environment, messages) — taskfleet has workers + status, but lacks groups, memories, and inter-worker messaging
2. The **scope layer** concept (`GET/PUT /api/scope`) — layered config global→group→worker
3. The **web-first** approach — the dashboard IS the product, not an afterthought
4. The **self-healing** pattern (crash → detect → restart with context)

**Effort:** 5-7 days for a minimal dashboard; 2-3 weeks for the full primitives

---

### S2. VNX Orchestration: Append-Only Receipt Ledger
**Source:** Vinix24/vnx-orchestration (⭐57, Python)
**Relevance:** ★★★★★ | **Actionability:** ★★★★★ | **Impact:** ★★★★☆

VNX is the closest conceptual match to taskfleet in terms of governance philosophy. Built on **3,000+ hours of production use** with **18,816 test functions**. Key innovations:

- **Append-only NDJSON receipt per dispatch** with hash-chain verification (`audit_chain`)
- **Per-gate cost tracking** — real `token_usage` harvested from claude-harness transcripts
- **Review gates** (codex + gemini) with **deterministic CI** as the third gate — three independent verifiers
- **No vendor SDK** — calls CLIs as subprocesses (same philosophy as taskfleet)
- **Governed memory** (past + current)
- **Self-learning proposal tier** — mines receipt stream for recurring failures → human-approved rules
- **Zero-LLM context injection** — repo map for fast context assembly without burning tokens
- **Tiered maturity model** — explicit Tier 1 (production), Tier 2 (opt-in), Tier 3 (designed)

**What taskfleet should steal:**
1. **Receipt ledger** — every dispatch writes an NDJSON receipt. This is the missing observability layer. `orchestrator.sh` currently loses dispatch context after the run.
2. **Per-gate cost tracking** — VNX harvests real token usage from agent transcripts. Taskfleet should parse `pi` output for `usage` fields.
3. **Three-tier review gate** — taskfleet's single `accept` command is a single point of failure. A second independent verifier (different model/provider) would catch false positives.
4. **Self-learning rules** — mine `state/logs/*.error.json` for recurring failure patterns and auto-suggest gate/config improvements.
5. **Explicit maturity tiers** — publish which features are production-proven vs. experimental.

**Effort:** 3-4 days for receipt ledger + cost tracking; 2-3 days for second verifier gate

---

### S3. Rove: Interactive Agent Multiplexer
**Source:** Sma1lboy/rove (⭐91, TypeScript)
**Relevance:** ★★★★★ | **Actionability:** ★★★★☆ | **Impact:** ★★★★☆

rove is a **terminal-first** agent multiplexer — the anti-amux. Key innovations:

- **Attach/detach/reattach** — agent sessions persist across disconnects (like tmux but for agents)
- **Programmatic API** — `rove api add/read-output/send/land` — shell scripts AND other agents can drive the multiplexer
- **Cross-session routing** — a task spawned by Claude Code routes results back to the Claude Code tab that asked for it
- **One git worktree + branch per task** (identical to taskfleet)
- **TUI interface** — interactive terminal UI with live agent output
- **Pi as a supported agent** — already has a `pi` preset

**What taskfleet should steal:**
1. **The `rove api` pattern** — make taskfleet scriptable from other agents and CI. `taskfleet api add --task ...` / `taskfleet api status --json`
2. **Attach/detach model** — currently taskfleet's dispatched agents run as backgrounded subshells. There's no way to "attach" to watch a running agent. Adding `taskfleet attach TASK_ID` would be huge for debugging.
3. **Cross-session routing** — if task A spawns a sub-task, the result routes back to task A's agent context.

**Effort:** 2-3 days for `taskfleet api` + attach

---

## 🟡 Tier A — Adopt These (High Value, Medium Effort)

### A1. groundcrew: Jira/Linear Integration + Task Sources
**Source:** ClipboardHealth/groundcrew (⭐61, TypeScript)
**Relevance:** ★★★★☆ | **Actionability:** ★★★★☆ | **Impact:** ★★★☆☆

groundcrew dispatches task backlogs from **Linear** and **Jira** to local agents. Key features:
- **Pluggable task sources** — Linear by default, Jira and local files
- **Agent routing via labels** — `agent-claude`, `agent-codex`, `agent-pi`, `agent-any` labels on tickets
- **Docker sandbox isolation** — Safehouse or Docker Sandboxes per worktree
- **Budget-aware routing** — `agent-any` routes to the agent with most session headroom and budget room
- **Task description IS the prompt** — sends the ticket title + description as the agent prompt

**What taskfleet should steal:**
1. **External task sources** — instead of only `config/tasks.json`, also read from Linear/Jira/GitHub Issues
2. **Budget-aware routing** — if worker A has $5 remaining and worker B has $50, route to B
3. **Sandbox configuration per task** — some tasks need databases, others don't

**Effort:** 2-3 days for Linear/GitHub Issues task source adapter

---

### A2. omux: Loop Engineering + Cross-Vendor Review
**Source:** Happenmass/omux (⭐95, TypeScript)
**Relevance:** ★★★★☆ | **Actionability:** ★★★☆☆ | **Impact:** ★★★★☆

omux is a **meta-agent** that orchestrates Claude Code and Codex. Its key insight:

- **Execute-then-review with different vendors** — Claude implements, Codex independently reviews the diff (different model, different vendor). This is structurally impossible with a same-context plugin.
- **Loop engineering** — the meta-agent IS the loop: it writes prompts, reads agent output, decides next move, repeats until verifiably done
- **Push-based waiting** — parks the meta-agent (zero tokens) while sub-agents work, wakes on callback
- **Per-agent MCP scoping** — a docs agent doesn't inherit the DB agent's MCP tools

**What taskfleet should steal:**
1. **Cross-vendor verification gate** — add a `verify_worker` field to tasks: after the primary worker completes, dispatch the verification to a DIFFERENT provider. Two independent models agreeing is much stronger than one.
2. **Token-efficient waiting** — taskfleet currently uses `sleep $TF_POLL` which is fine (no tokens burned), but the meta-agent concept is relevant for the orchestration layer.
3. **Per-task MCP scoping** — if using pi with MCP tools, some tasks shouldn't see all tools.

**Effort:** 1-2 days for cross-vendor verify gate

---

### A3. Evgeniy-Mikhailove: Six Production Patterns (Documentation)
**Source:** Evgeniy-Mikhailove/multi-agent-orchestration
**Relevance:** ★★★★☆ | **Actionability:** ★★★★★ | **Impact:** ★★★☆☆

This is a **patterns document**, not a tool. Six production patterns:

1. **Three-Tier Dispatch** — Command → Agent → Skill (taskfleet has this implicitly)
2. **Parallel Wave Execution** — wave-based parallelism (taskfleet has this via `deps`)
3. **Git Worktree Isolation** — identical to taskfleet
4. **Cross-Model Routing** — route subtasks to the cheapest capable model
5. **Two-Stage Review Gate** — spec compliance + code quality (taskfleet has single stage)
6. **Observer Loop Prevention** — max depth 3, duplicate detection

**What taskfleet should steal:**
1. **Cross-model routing** — add `model_tier` to tasks: `booster|fast|standard|deep`. Route `booster` tasks to cheap models, `deep` tasks to expensive ones.
2. **Two-stage gate** — split `accept` into `accept_spec` + `accept_quality` (or add `accept_review` as a separate field)
3. **Pattern documentation** — write a `docs/patterns.md` that describes how to use taskfleet effectively (currently the README is config-reference, not how-to-think-about-it)

**Effort:** 1 day for cross-model routing; 1 day for two-stage gate

---

### A4. agent-dispatch: MCP-Based Cross-Repo Delegation
**Source:** ginkida/agent-dispatch (⭐30, Python)
**Relevance:** ★★★☆☆ | **Actionability:** ★★★☆☆ | **Impact:** ★★★☆☆

An MCP server that lets Claude Code agents delegate tasks to agents in **other project directories**. Each sub-agent runs as a separate `claude -p` session inheriting that project's MCP servers and CLAUDE.md.

**What taskfleet should steal:**
1. **Multi-repo task support** — allow tasks in `tasks.json` to reference different repos. This is the only project we found that solves cross-repo orchestration cleanly.
2. **Project-scoped context** — each worktree inherits the target project's CLAUDE.md / config, not the orchestrator's.

**Effort:** 3-4 days for multi-repo support

---

## 🟢 Tier B — Worth Watching (Lower Priority)

### B1. squad: Claude→Codex/Gemini MCP Dispatch
**Source:** jfikrat/squad (⭐3)
Relevance: Low — MCP-based dispatch from Claude Code to Codex/Gemini in parallel via tmux. Narrow scope but validates the cross-vendor pattern.

### B2. open-multi-agent-rs: Rust DAG Orchestrator
**Source:** Supernova1744/open-multi-agent-rs (⭐3)
Relevance: Low — Rust port of open-multi-agent with dependency-aware scheduling, shared memory, streaming, observability hooks. The **observability hooks** design could inspire taskfleet's instrumentation.

### B3. adg-parallels: VS Code Extension
**Source:** adamerso/adg-parallels (⭐5)
Relevance: Low — VS Code extension for multi-agent orchestration. The **parallel computing** topic tag suggests a different isolation model than git worktrees.

---

## 📘 Tier C — Academic Insights (arXiv Papers)

### C1. "A Two-Tier Perspective on Inference-Time Parallelism in Multi-Agent LLM Systems" ⭐ Highest arXiv score (6.80)
**Relevance:** ★★★★★ | **Actionability:** ★★☆☆☆

Studies two parallelism strategies: **inter-agent parallelism** (multiple agents, each handling a subtask) vs. **intra-agent parallelism** (parallel inference within one model). Finding: inter-agent parallelism scales better for complex tasks.

**Implication for taskfleet:** Taskfleet already does inter-agent parallelism. The paper validates this as the correct architecture. Consider adding intra-agent parallelism (e.g., running multiple acceptance gates concurrently).

### C2. ATM: "CID-Brokered Pre-Write Admission for Multi-Agent Code Co-Synthesis" (4.40)
**Relevance:** ★★★★☆

Introduces a **contention-aware admission controller** before any shared mutation in multi-agent code synthesis. Agents must acquire write locks on shared files before modifying them.

**Implication for taskfleet:** taskfleet's scope checking is advisory. Adding a **contention-aware lock** that prevents two parallel tasks from modifying overlapping files would prevent merge conflicts entirely (rather than recovering from them).

### C3. "Specification-first convergence: 189 files, 717k-line codebase" (2.69)
**Relevance:** ★★★★☆

Case study of a single AI coding agent refactoring 189 files with **no test oracle and no human review**, using a specification-first protocol. Key insight: the agent succeeded because it had a clear specification, not because of the model's capabilities.

**Implication for taskfleet:** The `acceptance_prose` field in taskfleet's task schema is underutilized. Better prose specifications → better agent performance.

### C4. "ForestBench: A Unified Graph Framework for Evaluating Multi-Agent Collaboration" (3.95)
**Relevance:** ★★★☆☆

Evaluates multi-agent systems via **execution traces** rather than just outcomes. Provides a common basis for comparison across methods.

**Implication for taskfleet:** The receipt ledger (from S2/VNX) would enable trace-based evaluation. Store not just pass/fail but the full execution trace per task.

### C5. "Beyond Global Replanning: Hierarchical Recovery for Cross-Device Agent Systems" (1.00)
**Relevance:** ★★★☆☆

Proposes **hierarchical recovery** instead of global replanning when a sub-agent fails. Only the failed subtree is replanned, not the entire task graph.

**Implication for taskfleet:** Currently, a failed task either retries from scratch (fresh branch) or is permanently failed. **Hierarchical retry** — retry only the failed portion of the work, not the entire task — would be more efficient for large tasks.

### C6. "Reinforcing Step-level Reasoning for Effective Self-Correction in LLMs" (1.69)
**Relevance:** ★★★☆☆

Shows that **step-level feedback** (rewarding each correct reasoning step, not just the final answer) dramatically improves self-correction in LLMs.

**Implication for taskfleet:** The error feedback injected into retry prompts is currently gate-level (the last failure). **Step-level feedback** — breaking the failure into individual steps and identifying which step went wrong — could improve retry success rates.

---

## 📊 Summary: Priority Ranking

| Rank | ID | Source | Action | Effort | Impact |
|------|----|--------|--------|--------|--------|
| **1** | S2 | VNX Orchestration | Receipt ledger + per-gate cost tracking | 3-4d | Observability + cost visibility |
| **2** | S1 | amux | Web dashboard (kanban + SSE) | 5-7d | Adoption blocker removed |
| **3** | A2 | omux | Cross-vendor verify gate | 1-2d | Verification quality 2× |
| **4** | S3 | rove | `taskfleet api` + attach | 2-3d | Debuggability + scriptability |
| **5** | A3 | 6 Patterns | Cross-model routing + two-stage gate | 2d | Cost optimization |
| **6** | A1 | groundcrew | External task sources (Linear/GitHub) | 2-3d | Workflow integration |
| **7** | A4 | agent-dispatch | Multi-repo support | 3-4d | Scope expansion |
| **8** | C2 | ATM paper | Content-aware write admission | 2-3d | Prevent merge conflicts |
| **9** | S1 | amux | Scope layers + inter-worker messaging | 3-4d | Coordination |
| **10** | C6 | Step-level paper | Step-level error feedback | 2-3d | Retry success rate |

**Total estimated effort for top 5:** 13-19 days
**Total estimated effort for all 10:** 25-35 days
