# Competitive Feature Matrix

Comparing taskfleet against the most similar projects.

## Legend
- ✅ = Supported
- ⚠️ = Partial
- ❌ = Not supported

## Core Features

| Feature | taskfleet | shogun | pocock | boi | vortex | oh-my-agent | omnigent |
|---|---|---|---|---|---|---|---|
| **Stars** | — | 1,412 | 26 | 0 | 0 | 1,226 | 8,818 |
| **Language** | Bash | Shell | Shell | Go | Go | TS | TS |
| **Parallel dispatch** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Git worktree isolation** | ✅ | ❌ (tmux) | ✅ | ✅ | ✅ | ❌ | ❌ |
| **DAG dependencies** | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ | ❌ |
| **Acceptance gates** | ✅ (shell) | ❌ | ✅ (TDD) | ✅ | ✅ (5-tier) | ✅ | ❌ |
| **Error classification** | ✅ (9 cats) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Retry with feedback** | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ |
| **Merge conflict recovery** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

## Provider / Harness Support

| Feature | taskfleet | shogun | pocock | boi | vortex | oh-my-agent | omnigent |
|---|---|---|---|---|---|---|---|
| **Provider agnostic** | ✅ | ❌ | ❌ | ❌ | ✅ | ✅ | ✅ |
| **Multi-harness** | ✅ (2) | ❌ | ❌ | ❌ | ✅ (3) | ✅ (10+) | ✅ (5+) |
| **Local model support** | ✅ (vllm) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Worker health check** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

## Robustness & Reliability

| Feature | taskfleet | shogun | pocock | boi | vortex | oh-my-agent | omnigent |
|---|---|---|---|---|---|---|---|
| **Stale process cleanup** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Stale worktree recovery** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Deadlock detection** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **No-op detection** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Event sourcing** | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ | ❌ |

## Testing Quality

| Feature | taskfleet | shogun | pocock | boi | vortex | oh-my-agent | omnigent |
|---|---|---|---|---|---|---|---|
| **Mutation testing** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Chaos testing** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Property-based** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Fuzz testing** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Stress testing** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Test count** | 159 | ❓ | ❓ | ❓ | ❓ | ❓ | ❓ |

## Key Takeaways

**Where taskfleet wins:** Testing quality (unique), error classification + retry feedback, merge conflict recovery, 6-level robustness stack, local model support, zero dependencies.

**Where to improve:** Web UI, cost tracking (first-mover opportunity), HITL gates, streaming output, notifications, PR-based merge.
