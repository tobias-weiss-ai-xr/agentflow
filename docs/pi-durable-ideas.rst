Pi Durable → AgentFlow: Durability Ideas
========================================

:Source: `Pi Durable <https://earendil.com/posts/pi-durable/>`_ (Earendil, 2026-10-01)
:Reference implementation: ``~/git/pi-durable/packages/durable``
   (``@earendil-works/pi-durable``; spec in ``docs/spec.md`` — the “Pico5 specification”)
:Target: AgentFlow (``~/git/agentflow``, 4.0k LOC Rust)
:Date: 2026-10-05
:Status: Proposal — not yet scheduled
:Companion: ``docs/moe-sovereign-ideas.md`` (same shape: source → target ideas analysis)

.. contents:: Contents
   :local:
   :depth: 2

1. Why pi-durable is relevant to agentflow
------------------------------------------

Pi Durable is a *durable agent harness*: “storage plus the machinery needed to
run one or more conversations with large language models in parallel”
(``packages/durable/README.md``). Its central claim is the one agentflow also
makes, one level down:

    every step of a run is a task that stores a checkpoint before it
    moves on. If the process dies, a new process opens the same storage,
    finds the unfinished tasks, and continues each one from its last
    checkpoint.

AgentFlow already advertises **“Self-healing — atomic JSON state; crash-safe
resume”**, but its durability granularity is coarse: an *attempt* is one opaque
span (``worktree → agent CLI → gate → merge``). Pi Durable commits a checkpoint
at **every phase boundary** and distinguishes **replay-safe** effects from
effects that must never be repeated. That gap is where the useful ideas live.

The two systems are otherwise different by design, and this document is explicit
about what *not* to import (see §7). Pi Durable is a ~20k-LOC TypeScript
framework for running conversations anywhere; agentflow is a thin Rust
orchestrator that drives ``git`` and an external agent CLI (ADR-1). AgentFlow has
no transcripts, no model calls, no Chord documents, and no forking conversations
— so the transcript-shaped half of Pi Durable is out of scope by construction.

What transfers cleanly is the **durability discipline**, not the runtime.

2. Parity: what agentflow already gets right
--------------------------------------------

.. list-table:: Durability parity
   :header-rows: 1
   :widths: 34 33 33

   * - Pi Durable mechanism
     - AgentFlow today
     - Verdict
   * - Durable task state machine per unit of work (``pi.generation`` /
       ``pi.tool``) with a checkpoint per phase (§5.1–5.2)
     - ``TaskStatus { state, attempts, last_error }`` in
       ``run-state.json``; one row per task (``src/state.rs``)
     - **Partial.** Coarse: state changes only at dispatch and reap, not at
       each phase of an attempt.
   * - Atomic commit; “nothing is shown before its commit is stored” (§1.2–1.3)
     - temp-file + ``rename`` for ``run-state.json`` (ADR-3); receipts are
       one file each
     - **Partial.** Each *file* is atomic; the status/receipt *pair* is not.
   * - Reopen resumes unfinished tasks; interrupted work is retried
     - Startup self-heal: ``Running`` → ``Ready``, ``last_error = "previous
       run interrupted"``; orphan worktrees healed (``src/run.rs``)
     - **Yes**, at attempt granularity.
   * - Exactly-once submissions via ``requestId``
     - Attempt counters persisted *before* spawn; receipts get a nanosecond
       suffix so fast retries never overwrite
     - **Partial.** No explicit idempotency key; no dedupe on reload.
   * - Storage backends with a shared **conformance suite** and benchmarks
       (§10–11)
     - Two bespoke JSON writers, each with its own ad-hoc unit tests
     - **Gap.**
   * - Cost/usage includes failed and aborted attempts; per-model, per-tool
       (``pi.usage``)
     - ``Receipt`` per attempt (worker, model, wall-clock, outcome, error);
       ``af cost`` shows per-task and per-worker trust
     - **Mostly yes.** Wall-clock only; ``tokens`` is an unpopulated
       ``Option<u64>``.
   * - Structured concurrency: ownership tree, abort flows down, owned work
       does not outlive its owner (§5.5)
     - ``deps`` DAG, critical-path priority, deadlock detection, contention
       avoidance — but no ownership edges and no abort
     - **Gap.** (See §6.3; probably out of scope for agentflow.)
   * - Observability as committed state: ``viewState`` / ``watch`` /
       ``taskGraph`` (§9)
     - ``af status [--json]``, ``af api status``, ``af api results``,
       ``af attach`` (log tail)
     - **Partial.** Point-in-time reads only; no event stream, no graph view.

3. Gap analysis
---------------

.. list-table:: Ranked by (value ÷ effort)
   :header-rows: 1
   :widths: 6 26 30 30 8

   * - #
     - Idea (pi-durable)
     - AgentFlow gap
     - Proposal
     - Tier
   * - 1
     - Effect sandwich: ``commit intent → effect → commit outcome`` (§5.2)
     - A crash after the agent committed but before the gate/merge re-runs the
       **agent** (the expensive, non-replayable step)
     - Attempt **phase journal**: ``spawned → agent_done → gate_passed →
       merged``; resume re-runs only replay-safe phases
     - 1
   * - 2
     - ``replay: "safe"`` on tools; unsafe effects are *reported*, never
       repeated (§5.2, README “Tools”)
     - No notion of replay safety at all
     - ``gate_replay`` (default ``true``) + documented contract that gates must
       be idempotent; non-replayable gates settle as ``interrupted``
     - 1
   * - 3
     - “One process owns a storage at a time” (README “Storage”)
     - Two concurrent ``af run`` invocations would clobber ``run-state.json``
     - Advisory lock file ``<state>/.lock`` (pid); second writer exits 2
     - 1
   * - 4
     - ``registerStorageConformance`` — one suite, every backend (§10–11)
     - Crash-safety is the headline feature but is covered by two hand-written
       unit tests
     - ``trait StateStore`` + a generic **conformance test suite** every
       backend must pass (torn write, concurrent append, ordering, atomicity)
     - 1
   * - 5
     - One Session commit is atomic across **all** records (§1.1)
     - ``store.save()`` then ``append_receipt()`` — a crash between them leaves
       ``Done`` with no receipt (or a receipt with stale status)
     - Single ``commit()`` that writes status + receipt(s); ordering chosen so
       the worst case is a *duplicate* receipt, which is dedupe-able
     - 1
   * - 6
     - Cancelling a wait cancels only the wait; aborted work is marked, not
       lost
     - ``SIGINT`` mid-campaign leaves ``Running`` rows and orphan worktrees;
       next run re-dispatches from scratch
     - Cooperative cancel: stop dispatching, mark in-flight attempts
       ``interrupted``, drain or kill with a bounded grace period
     - 2
   * - 7
     - Progress durably committed in bounded windows (100 ms default); a crash
       loses at most that window
     - Status changes only at reap; ``af status`` cannot show elapsed time or
       partial output for a running attempt
     - Periodic **heartbeat** checkpoint (last log offset + elapsed) into
       ``run-state.json``
     - 2
   * - 8
     - ``taskGraph`` / ``watch`` / agent events (§9)
     - No graph view, no event stream
     - ``af api graph --json`` (nodes: depth, state, deps, contention) and
       ``af api watch`` (NDJSON of dispatch/reap/merge)
     - 2
   * - 9
     - Compaction bounds what the model sees (§README “Compaction”)
     - Retry context concatenates every prior error; grows without bound across
       ``max_attempts``
     - Bounded, summarised retry context (last *N* errors + one “lessons” line)
     - 2
   * - 10
     - Usage counted for failed and aborted attempts too; per model/tool
     - ``tokens`` never populated; ``af cost`` is wall-clock only
     - Parse provider usage from the agent log when present; ``af cost
       --json``
     - 2
   * - 11
     - System prompt rebuilt from named **sections** before each request;
       changes are positional entries (§7.4, README “System Prompt”)
     - One monolithic ``prompt_file``; a foreign/incompatible template silently
       degraded every dispatch (the round-2 bug)
     - Composable prompt sections (project context + task section), each
       independently renderable and assertable
     - 3
   * - 12
     - Extensions stored **by name**, resolved against a registry (§7.1)
     - ``prompt_file`` / gates are referenced inline; refactoring a template
       breaks every config
     - Named **profiles** (prompt + gate + env) referenced from ``tasks.json``
     - 3
   * - 13
     - Ownership tree; abort flows down; owned work never outlives its owner
       (§5.5)
     - Flat ``deps`` DAG; no parent/child, no bottom-up abort
     - Optional ``parent`` + subtree scoping for ``--task``
     - 3
   * - 14
     - Storage backends behind a small interface
     - JSON files hard-wired
     - ``StateStore`` trait (arrives with #4); a SQLite backend only if a real
       need appears
     - 3

4. Tier 1 — high value, low risk
--------------------------------

These are the ideas that pay for themselves inside one campaign and are cheap
to verify with agentflow's own spec → contract → test pyramid. Each proposal
names the RED assertion that must fail before the change.

4.1 Attempt phase journal (the effect sandwich)
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

**Concept.** Pi Durable’s §5.2 *effect sandwich* is::

    commit intent phase
    perform external effect
    commit outcome or next phase

Reopening *in an intent phase* means the effect may have happened; the handler
either retries safely, polls an external handle, or records interruption.

**Today.** AgentFlow’s attempt has four effects in sequence — create worktree,
run the agent CLI, run the acceptance gate, merge to base — but records none of
the boundaries. A crash between “agent finished” and “gate passed” throws away
the agent’s work: the startup heal resets the task to ``Ready`` and the next
attempt re-invokes the LLM. With ``agent_timeout_s`` measured in thousands of
seconds and round-2 campaigns costing ~1000 s, that is real money.

**Proposal.** Persist a phase per ``(task, attempt)`` inside ``TaskStatus``
(or a sibling ``attempts/<id>-<n>.json``), committed with the same temp+rename
discipline::

    pub enum AttemptPhase {
        Spawned,      // intent recorded; worktree + agent about to run
        AgentDone,    // agent exited 0 and its commit exists on the branch
        GatePassed,   // acceptance command exited 0
        // absence of a record, or Merged state, means terminal
    }

On startup, for each ``Running`` row, read the phase:

* ``Spawned`` → the agent may or may not have finished. **Re-dispatch** (the
  agent is the non-replayable effect; mirrors Pi Durable reporting an
  interrupted unsafe tool call to the model).
* ``AgentDone`` → the worktree/branch still holds a valid commit. **Re-run the
  gate** (cheap, deterministic) instead of re-invoking the agent.
* ``GatePassed`` → **re-run the merge only**, idempotently.

**Contract test (RED first).**

``crash_after_agent_does_not_rerun_agent`` — drive ``run_loop`` with a stub
agent that increments a counter file and exits 0; kill the orchestrator between
``AgentDone`` and the gate (inject via a test-only hook or by exiting the
process in a child); restart; assert the counter is **1**, not 2, and the gate
ran. Requires ``AttemptPhase`` to be observable in state, which it is not
today → RED.

**Effort** M (~150 LOC + tests). **Risk** low: monotone phases, no new external
dependency.

4.2 Replay-safety on the acceptance gate
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

**Concept.** Pi Durable marks *read-only* tools ``replay: "safe"`` so a rerun
after a crash is fine; a ``deploy`` has no such mark and is never repeated.

**Today.** §4.1 makes gates re-runnable, so their safety must become explicit.
Most gates are already read-only (``cargo test``, ``pytest``, ``grep -q``), but
nothing stops a config from declaring a gate with side effects (a script that
writes artefacts, publishes, or sends mail).

**Proposal.** Add ``gate_replay: bool`` (default ``true``) to the task schema.
Document the contract in the task table: *“``accept`` MUST be idempotent;
declare ``gate_replay: false`` for a gate with side effects, and agentflow will
settle an interrupted such gate as failed rather than re-run it.”* This is a
config + doc change plus a small branch in the resume path.

**Contract test.** ``config_rejects_or_flags_non_idempotent_gate`` … concretely:
``gate_replay_false_gate_is_not_rerun_after_crash`` — same harness as 4.1 but
with ``gate_replay: false``; assert the gate counter is 1 and the task settles
``Failed`` with reason ``gate interrupted``.

**Effort** S. **Risk** low.

4.3 Single-writer state lock
~~~~~~~~~~~~~~~~~~~~~~~~~~~~

**Concept.** “One process owns a storage at a time; other clients attach to
that process.” (README “Storage”). The invariant is stated, not enforced.

**Today.** Two ``af run`` processes sharing ``TF_STATE_DIR`` both load
``run-state.json``, both dispatch, and both ``rename`` over each other — lost
updates, duplicate merges. ``merge_locks`` only guards within one process.

**Proposal.** At startup, create ``<state>/.lock`` with the pid, using
``O_CREAT|O_EXCL``; a stale lock (dead pid) is reclaimed with a warning; a live
lock makes ``af run`` exit 2 with a clear message. Reader commands
(``status``, ``cost``) stay lock-free.

**Contract test.** ``second_runner_refuses_live_state_dir`` — hold the lock,
run ``run_loop``, assert exit 2 and that no worktree was created. Today the
second run proceeds → RED.

**Effort** S. **Risk** low (reclaim-on-dead-pid avoids the classic stale-lock
footgun).

4.4 A storage conformance suite
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

**Concept.** Pi Durable ships ``registerStorageConformance`` so memory, SQLite,
and JSONL backends are held to one contract (§10–11) — the reason a new backend
is cheap and trustworthy.

**Today.** AgentFlow’s crash-safety rests on ``src/state.rs``, verified by two
tests (``torn_write_never_corrupts``, ``receipts_append_and_aggregate``). The
contract — *atomic replace*, *torn writes never corrupt*, *append-only receipts
never overwrite*, *ordering by timestamp* — lives in prose and in one
implementation.

**Proposal.** Extract ``trait StateStore { fn load(...); fn save(...); fn
append_receipt(...); fn load_receipts(...); }`` with the current JSON
implementation as ``JsonStateStore``, then write **one** generic conformance
suite parameterised over ``impl StateStore``:

* ``atomic_replace_never_observes_partial`` (write, then read under a torn temp)
* ``torn_write_never_corrupts``
* ``append_only_receipts_never_overwrite`` (same second, same attempt)
* ``receipts_order_by_timestamp``
* ``save_is_durable_across_reopen``

Every existing unit test becomes a conformance case, so the suite *is* the
regression net. A future SQLite backend then inherits the guarantees for free.

**Contract test.** Run the suite against the current backend (passes) and
against a deliberately non-atomic test double (must fail) to prove the suite
bites.

**Effort** M. **Risk** low; pure refactor with the existing tests as the safety
net.

4.5 Atomic status + receipt commit
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

**Concept.** “One Session commit is atomic across all record and document
writes” (§1.1).

**Today.** ``execute_task`` records the outcome (status + receipt) in two
independent writes. A crash between them yields ``state = done`` with no
receipt (routing loses a data point) or a receipt with stale status.

**Proposal.** A single ``Store::commit(&status, &[Receipt])`` that (a) writes
the receipt(s) first, then (b) atomically replaces the status map. Ordering
makes the only reachable inconsistency a *duplicate* receipt, which
``load_receipts`` can dedupe on ``(task, attempt, ts)`` — cheap and monotone.
Document the ordering as the invariant.

**Contract test.** ``crash_between_receipt_and_status_recovers_consistently`` —
inject a failure after step (a); assert reload yields a consistent view and that
a second run does not double-count the attempt.

**Effort** S–M. **Risk** low.

5. Tier 2 — medium value, more surface
--------------------------------------

5.1 Cooperative cancellation
~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Pi Durable: aborting a wait cancels *only that wait, never the work*; aborted
model requests “stay in the transcript, marked as aborted”. AgentFlow has no
documented ``SIGINT`` story: today a killed ``af run`` leaves ``Running`` rows
and orphan worktrees for the next startup to heal. Proposal: install a
``SIGINT``/``SIGTERM`` handler that (1) stops dispatching, (2) lets in-flight
attempts finish within a short grace window or kills them, (3) records
``last_error = "interrupted by operator"`` and re-marks tasks ``Ready`` so
attempts are not burned — mirroring “the partial answer stays, marked as
aborted”. Contract test: send ``SIGTERM`` mid-attempt, assert the task returns
to ``Ready`` and the attempt count did not increase. Effort M.

5.2 Heartbeat checkpoints
~~~~~~~~~~~~~~~~~~~~~~~~~

Pi Durable commits partials “at most every 100 ms by default, so a crash loses
at most that window”. AgentFlow streams the agent to ``logs/<id>.log`` (not
fsynced) and only changes state at reap, so ``af status`` cannot show elapsed
time and ``attach`` after a crash shows nothing structured. Proposal: a
heartbeat thread writes ``elapsed_s`` + log offset into the running task’s
status every ~10 s. ``af status`` gains an ``ELAPSED`` column;
``af api results`` reports partial progress. Contract test:
``heartbeat_updates_elapsed_while_attempt_runs``. Effort S–M.

5.3 Graph and event APIs
~~~~~~~~~~~~~~~~~~~~~~~~

``harness.taskGraph(context)`` and ``watch()`` expose live structure as
committed state. AgentFlow has ``af status --json`` (point-in-time) but no graph
and no stream. Proposal: ``af api graph --json`` emitting nodes
``{id, deps, depth, state, attempts, scope, repo}`` derived from
``scheduler::compute_depths`` + state, and ``af api watch`` emitting NDJSON
events (``dispatch``, ``reap``, ``merge``, ``gate``) from the existing run loop
— the CLI already has a machine-readable ``api`` namespace. Contract test:
``graph_json_includes_depth_and_state_for_every_task``. Effort M.

5.4 Bounded retry context (compaction by another name)
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Pi Durable’s compaction summarizes older entries so the request stays inside
the context window. AgentFlow’s **retry memory** appends every prior error to
the prompt, so a task on attempt 5 carries four failure strings and grows. It is
the same problem at a smaller scale. Proposal: cap the injected history to the
last *N* errors, plus one synthesised line (“earlier attempts failed on: …”),
reusing the existing ``last_error`` machinery. Contract test:
``retry_prompt_is_bounded_after_many_failures``. Effort S.

5.5 Cost beyond wall clock
~~~~~~~~~~~~~~~~~~~~~~~~~~

Pi Durable counts usage for failed and aborted attempts, per
``provider/model`` and per tool. AgentFlow’s ``Receipt.tokens`` is
``Option<u64>`` and never filled; ``af cost`` is wall-clock only. Proposal:
parse a usage footer from the agent log when the CLI emits one (best-effort,
provider-agnostic), populate ``tokens``, and add ``af cost --json``. Contract
test: ``receipt_records_tokens_when_agent_reports_usage``. Effort S–M.

6. Tier 3 — larger, defer until justified
-----------------------------------------

6.1 Composable prompt sections
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Pi Durable rebuilds the system prompt from named **sections** before every
request and records changes positionally. AgentFlow’s single ``prompt_file``
already bit us: a foreign template with mismatched placeholders silently
dropped scope and acceptance for every worker (round 2). Sections (e.g.
``project-context`` + ``task``) would make each piece independently renderable
and assertable, which is exactly the test we wished we had. Worth doing when the
prompt surface grows again; the §4.1–4.2 work does not depend on it.

6.2 Named profiles (extensions stored by name)
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Pi Durable stores extensions *by name* and resolves them against a registry, so
a stored name outlives the code. The analogue: let ``tasks.json`` reference a
named profile (prompt + gate + env + sandbox) instead of inlining
``accept``/``acceptance_prose``, so refactoring a template does not require
editing every task. Medium value; mostly ergonomics, and it interacts with the
round-2 gate-design lessons (self-contained gates, CI checks, name-in-title).

6.3 Ownership tree and bottom-up abort
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Pi Durable’s structured concurrency (§5.5) — immutable owner edges, abort flows
down, owned work never outlives its owner, ``waiting``/``completing`` holds — is
the most sophisticated idea in the spec. It would give agentflow subtree
scoping for ``--task`` and a principled cancel. But agentflow’s ``deps`` DAG plus
deadlock detection already covers the ordering need, and a full ownership tree
is a large change (new states, scheduler reconcile, migration) for a 4k-LOC
orchestrator. **Defer**, and revisit only if campaigns start needing
nested/owned work.

6.4 Pluggable storage backend
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

The ``StateStore`` trait from §4.4 makes a SQLite backend feasible, but agentflow
has no measured need: JSON is small, atomic, and inspectable — an asset. Keep
JSON; adopt the trait for testability, not for a new backend.

7. Explicit non-goals
---------------------

Imported from Pi Durable’s *shape*, not its discipline — deliberately rejected:

* **A runtime.** No JavaScript/TypeScript, no Cloudflare Durable Objects, no
  in-memory sandbox. AgentFlow stays Rust + subprocesses (ADR-1).
* **Conversations, transcripts, entries, forks.** AgentFlow has no chat; its
  unit of work is a task, not a turn.
* **Model/provider access.** Provider session affinity, prompt caching,
  streaming deltas, and retries live in the agent CLI, not in ``af``.
* **Chord documents and positional system-prompt entries.** There is no
  transcript to carry them.
* **Compaction of a context window.** AgentFlow’s “context” is a prompt
  template; §5.4 is the only, much smaller, analogue.
* **A definition/registry lifecycle with migrations.** Pi Durable’s
  ``defineDoc``/versions/migrations solve a problem agentflow does not have.

Keeping these out preserves the property that makes agentflow dogfoodable: an
agent can read all of it.

8. Suggested dogfooding campaign
--------------------------------

Tier 1 maps directly onto agentflow tasks (self-contained scopes, tests as
gates). Sketch — each ``accept`` includes the CI checks that round-2 learned to
require:

.. code-block:: json

   {
     "tasks": [
       { "id": "pd-lock",      "title": "Add single-writer state lock",
         "scope": ["src/state.rs", "src/run.rs", "tests/"],
         "accept": "cargo test --test cli lock && cargo fmt --check && cargo clippy --all-targets" },
       { "id": "pd-conformance","title": "Storage conformance suite for StateStore",
         "scope": ["src/state.rs", "tests/"],
         "accept": "cargo test conformance && cargo fmt --check && cargo clippy --all-targets" },
       { "id": "pd-journal",   "title": "Attempt phase journal (effect sandwich)",
         "scope": ["src/state.rs", "src/execute.rs", "src/run.rs", "tests/"],
         "deps": ["pd-conformance"],
         "accept": "cargo test --test e2e phase_journal && cargo fmt --check && cargo clippy --all-targets" },
       { "id": "pd-gate-replay","title": "Add gate_replay safety flag",
         "scope": ["src/config.rs", "src/gate.rs", "README.md", "tests/"],
         "deps": ["pd-journal"],
         "accept": "cargo test gate_replay && cargo fmt --check && cargo clippy --all-targets" }
     ]
   }

``pd-lock`` runs parallel with ``pd-conformance`` (disjoint scopes);
``pd-journal`` follows the storage refactor; ``pd-gate-replay`` follows the
journal. Verify each gate **fails before** the change (RED → GREEN), per the
round-2 gate checklist.

9. References
-------------

* Post: https://earendil.com/posts/pi-durable/
* Local checkout: ``~/git/pi-durable/packages/durable``
* Normative spec: ``docs/spec.md`` — §1 invariants, §5.1–5.2 tasks & effect
  sandwich, §5.5 structured concurrency, §9 observation, §10–11 storage.
* README: ``README.md`` — “Persist and Resume”, “Tools”, “Compaction”,
  “Usage and Cost”, “Storage”.
* Runnable examples: ``test/examples/00``–``31`` (esp. ``22``–``24``
  subagents/child tasks, ``31`` reload and restart).
* AgentFlow decision records: ADR-1 (thin orchestrator), ADR-3 (atomic state),
  ADR-9 (wall-clock truth), ADR-10 (sandboxing), ADR-12 (UCB1 routing) — see
  ``docs/arc42/09-architecture-decisions.md``.
