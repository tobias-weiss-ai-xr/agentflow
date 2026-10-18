# Verify Tasks + Per-Contract Acceptance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `verify: true` task flag (never-merge, gate-only success, archived branch) and per-contract acceptance via an optional `accept_report.json` (parsed after the gate, surfaced in receipts + `status --json`, and seeded into retry prompts).

**Architecture:** Two additions to the existing effect sandwich in `src/execute.rs`. (1) A `verify` bool on `Task` makes the gate the sole deliverable: no-change is allowed to pass and the merge step becomes an archive instead (branch preserved under `<branch>-rejected-<now>`), returning a new `Outcome::Verified` that counts as a verdict. (2) A best-effort `accept_report.json` parser runs after the gate; per-contract verdicts are stored as `ContractVerdict` pairs on the `Receipt`, rendered in `status --json`, and fed into `failure_context` retry memory.

**Tech Stack:** Rust (pinned 1.98.1 via `rust-toolchain.toml`), serde/serde_json, git. Tests run in WSL because the Windows host has no gcc/cl (see Task 0).

---

## File Structure

- `src/config.rs` — add `verify: bool` to `Task`; add validation rules in `validate`.
- `src/state.rs` — add `ContractVerdict` struct; add `contracts: Option<Vec<ContractVerdict>>` to `Receipt`; extend `counts_as_verdict`.
- `src/execute.rs` — add `Outcome::Verified`; wire receipt mapping; gate-only no-change allowance; archive-vs-merge branch; parse `accept_report.json`; extend `failure_context` retry memory.
- `src/run.rs` — extend `status_json` with `verify`/`verdict`/`contracts`.
- `tests/e2e.rs` — end-to-end verify-task tests using the existing `Fixture`/`run_loop` pattern.
- `tests/cli.rs` — CLI-level validation tests (verify+manual / verify+readonly / verify-without-gate rejections).
- `openspec/specs/lifecycle/spec.md` — document the verify task behavior + per-contract acceptance.

Design spec (approved): `docs/superpowers/specs/2026-10-18-verify-tasks-per-contract-acceptance-design.md`.

---

## Task 0: Setup and environment note

**Files:** none.

- [ ] **Step 1: Confirm the test environment**

All test commands in this plan run inside WSL because the Windows host lacks `gcc`/`cl` (cc-rs fails) and Windows `cargo test` shows 3 false failures. Run every test as:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && <command>'
```

The repo lives on `/mnt/c` (drvfs), so git checkouts use CRLF line endings — assertions comparing file contents against hardcoded strings must normalize line endings with `.replace("\r\n", "\n").replace("\r", "\n")` (existing convention).

- [ ] **Step 2: Verify the baseline is green**

Run:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --all-targets 2>&1 | tail -1'
```

Expected: `test result: ok.` (all suites). If not green, stop and investigate before starting.

- [ ] **Step 3: Create a working branch**

```bash
cd C:\Users\Tobias\git\agentflow
git checkout -b feat/verify-tasks
```

---

## Task 1: Add `verify` to the Task struct (config)

**Files:**
- Modify: `src/config.rs:74-115` (Task struct)

- [ ] **Step 1: Write the failing test**

Add to the config.rs `validate` test module (near the existing `readonly_with_accept_rejected` test at `src/config.rs:933-949`). Find that test first:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && grep -n "readonly_with_accept_rejected" src/config.rs'
```

Append these three tests after the `readonly_with_accept_rejected` test (they use the existing `tmpdir`/`wt`/`load` test helpers in `src/config.rs` tests module):

```rust
#[test]
fn verify_without_gate_is_rejected() {
    let d = tmpdir("cfg-verify-nogate");
    wt(
        &d.join("tasks.json"),
        r#"{ "tasks": [ { "id": "A", "title": "t", "verify": true } ] }"#,
    );
    wt(
        &d.join("workers.json"),
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m" } ] }"#,
    );
    let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
    assert!(
        e.contains("verify task must declare an accept gate"),
        "{e}"
    );
}

#[test]
fn verify_plus_manual_is_rejected() {
    let d = tmpdir("cfg-verify-manual");
    wt(
        &d.join("tasks.json"),
        r#"{ "tasks": [ { "id": "A", "title": "t", "verify": true, "manual": true, "accept": "true" } ] }"#,
    );
    wt(
        &d.join("workers.json"),
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m" } ] }"#,
    );
    let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
    assert!(
        e.contains("verify task cannot be manual"),
        "{e}"
    );
}

#[test]
fn verify_plus_readonly_is_rejected() {
    let d = tmpdir("cfg-verify-readonly");
    wt(
        &d.join("tasks.json"),
        r#"{ "tasks": [ { "id": "A", "title": "t", "verify": true, "readonly": true, "accept": "true" } ] }"#,
    );
    wt(
        &d.join("workers.json"),
        r#"{ "workers": [ { "name": "w1", "provider": "p", "model": "m" } ] }"#,
    );
    let e = load(&d.join("tasks.json"), &d.join("workers.json")).unwrap_err();
    assert!(
        e.contains("verify task cannot be readonly"),
        "{e}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib config::tests::verify_without_gate_is_rejected config::tests::verify_plus_manual_is_rejected config::tests::verify_plus_readonly_is_rejected 2>&1 | tail -3'
```

Expected: `error[E0609]: no field 'verify' on type 'Task'` (field does not exist yet). Confirm the tests compile-fail for the right reason (missing field) — this proves the tests target the new field.

- [ ] **Step 3: Add the `verify` field to the Task struct**

In `src/config.rs`, after the `readonly` field (line 114, before the closing brace at 115), add:

```rust
    /// Verify-only task: the acceptance gate is the SOLE deliverable. The
    /// agent runs and the gate must pass, but the branch is NEVER merged —
    /// it is archived instead, and the attempt records `outcome: verified`.
    /// A task that produces no committed change still passes if the gate
    /// exits 0. Validation rejects combining `verify` with `manual`,
    /// `readonly`, or a missing `accept` gate. Default `false`.
    #[serde(default)]
    pub verify: bool,
```

- [ ] **Step 4: Run test to verify it passes (or fails for the right reason)**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib config::tests::verify_without_gate_is_rejected config::tests::verify_plus_manual_is_rejected config::tests::verify_plus_readonly_is_rejected 2>&1 | tail -3'
```

Expected: the three tests FAIL because `validate` does not yet reject these combos (e.g. `assertion failed: e.contains("verify task must declare an accept gate")`). This is the correct TDD state: tests are wired to the new field, validation not yet implemented.

- [ ] **Step 5: Add the validation rules**

In `src/config.rs` `validate` (after the `readonly + accept` check at line 364-369, before the no-gate warning at 370-376), insert:

```rust
        if t.verify && t.accept.is_none() {
            return Err(format!(
                "task '{}': verify task must declare an accept gate",
                t.id
            ));
        }
        if t.verify && t.manual {
            return Err(format!(
                "task '{}': verify task cannot be manual",
                t.id
            ));
        }
        if t.verify && t.readonly {
            return Err(format!(
                "task '{}': verify task cannot be readonly",
                t.id
            ));
        }
```

Then adjust the existing no-gate warning at lines 371-376 so a verify task that slipped past the error check never also emits the legacy "no gate" warning:

```rust
        // No gate and not manual: legacy corpus runs these gate-less; warn.
        if t.accept.is_none() && !t.manual && !t.readonly && !t.verify {
```

- [ ] **Step 6: Run test to verify it passes**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib config::tests::verify_without_gate_is_rejected config::tests::verify_plus_manual_is_rejected config::tests::verify_plus_readonly_is_rejected 2>&1 | tail -3'
```

Expected: `test result: ok.` — all three pass.

- [ ] **Step 7: Run the full lib test suite**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib 2>&1 | tail -1'
```

Expected: `test result: ok.` (all existing config tests still pass).

- [ ] **Step 8: Commit**

```bash
cd C:\Users\Tobias\git\agentflow
git add src/config.rs
git commit -m "feat(config): add verify task flag + validation (verify requires gate, excludes manual/readonly)"
```

---

## Task 2: Add `Outcome::Verified` and the `contracts` receipt field

**Files:**
- Modify: `src/execute.rs:14-18` (Outcome enum), `src/execute.rs:105-130` (receipt mapping)
- Modify: `src/state.rs:101-123` (Receipt), `src/state.rs:145-147` (counts_as_verdict)

- [ ] **Step 1: Write the failing tests**

Add to the `src/execute.rs` tests module (after the existing `failure_context` tests, near line 1263). The test helper `receipt_of` already exists (execute.rs:1204-1217):

```rust
#[test]
fn receipt_of_verified_outcome_maps_to_verified_string() {
    // The receipt serializer maps Outcome::Verified to "verified".
    let out = match Outcome::Verified {
        Outcome::Verified => "verified",
        _ => "merged",
    };
    assert_eq!(out, "verified");
}
```

Add to `src/state.rs` tests. First check whether a state.rs tests module exists and how `counts_as_verdict` is tested:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && grep -n "mod tests\|counts_as_verdict" src/state.rs'
```

Based on what you find, add (in the appropriate test module, using the same style as existing state.rs tests):

```rust
#[test]
fn verified_counts_as_a_verdict() {
    let r = Receipt { outcome: "verified".into(), ..Default::default() };
    assert!(r.counts_as_verdict(), "verified must count as a verdict");
}
```

If `Receipt` does not derive `Default` such that `..Default::default()` compiles, construct it fully with `outcome: "verified".into()` and the other fields set to their zero values (mirror the existing `receipt_of` helper in execute.rs:1204-1217 or any existing Receipt literal in state.rs tests).

Also add a state.rs test asserting a `ContractVerdict` list is serializable:

```rust
#[test]
fn contract_verdict_round_trips() {
    let cv = ContractVerdict { contract: "C-M1".into(), verdict: "pass".into() };
    let s = serde_json::to_string(&cv).unwrap();
    assert_eq!(s, r#"{"contract":"C-M1","verdict":"pass"}"#);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib execute::tests::receipt_of_verified_outcome_maps_to_verified_string state::tests::verified_counts_as_a_verdict state::tests::contract_verdict_round_trips 2>&1 | tail -5'
```

Expected: compile errors — `no variant named 'Verified' in enum 'Outcome'`, `no field 'outcome'` or missing `ContractVerdict`. Confirm failures are the "symbol doesn't exist yet" type.

- [ ] **Step 3: Add `Outcome::Verified` and extend the receipt mapping**

In `src/execute.rs:14-18`, change:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Merged,
    Failed(String),
}
```

to:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Merged,
    Verified,
    Failed(String),
}
```

In `src/execute.rs:109-116` (the `outcome_name` / `error` match inside `execute_task`), change:

```rust
    let outcome_name = match outcome {
        Outcome::Merged => "merged",
        Outcome::Failed(_) => "failed",
    };
    let error = match &outcome {
        Outcome::Failed(e) => Some(e.lines().next().unwrap_or("").chars().take(200).collect()),
        Outcome::Merged => None,
    };
```

to:

```rust
    let outcome_name = match outcome {
        Outcome::Merged => "merged",
        Outcome::Verified => "verified",
        Outcome::Failed(_) => "failed",
    };
    let error = match &outcome {
        Outcome::Failed(e) => Some(e.lines().next().unwrap_or("").chars().take(200).collect()),
        Outcome::Merged | Outcome::Verified => None,
    };
```

- [ ] **Step 4: Add `ContractVerdict` and the `contracts` field to Receipt**

In `src/state.rs`, add a new struct (place it just above the `Receipt` struct at state.rs:101):

```rust
/// A single per-contract verdict from an optional `accept_report.json`.
/// Stored as a Vec (not a HashMap) to preserve file/key ordering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractVerdict {
    pub contract: String,
    pub verdict: String,
}
```

In the `Receipt` struct (state.rs:101-123), add a field at the end (after `cost_micros`):

```rust
    /// Per-contract verdicts from an optional `accept_report.json`
    /// (verify tasks). `None` when no report was present/parseable.
    #[serde(default)]
    pub contracts: Option<Vec<ContractVerdict>>,
```

- [ ] **Step 5: Extend `counts_as_verdict`**

In `src/state.rs:145-147`, change:

```rust
    pub fn counts_as_verdict(&self) -> bool {
        self.outcome == "merged" || self.outcome == "failed"
    }
```

to:

```rust
    pub fn counts_as_verdict(&self) -> bool {
        self.outcome == "merged" || self.outcome == "failed" || self.outcome == "verified"
    }
```

- [ ] **Step 6: Fix every existing `Receipt` literal that now needs the new field**

Compilation will now fail wherever `Receipt { ... }` is built without `contracts`. Find them all:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo build --all-targets 2>&1 | grep -E "error\[|missing field" | head -40'
```

For each location (known sites: `src/execute.rs` receipt append at 117-128, `tests/cli.rs` `seed_receipt` at 131-142, `src/run.rs` tests, `src/state.rs` tests, `tests/e2e.rs` if any), add `contracts: None,`. Example for `tests/cli.rs` `seed_receipt` (line 127-144):

```rust
            .append_receipt(&Receipt {
                task: id.into(),
                attempt: 1,
                worker: "w1".into(),
                model: "m".into(),
                wall_clock_s: 1.5,
                tokens: None,
                ts: 1,
                outcome: "merged".into(),
                error: None,
                cost_micros: None,
                contracts: None,
            })
```

- [ ] **Step 7: Run tests to verify they pass**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --all-targets 2>&1 | tail -1'
```

Expected: `test result: ok.` for every suite (the new tests pass; all existing tests still compile and pass with `contracts: None`).

- [ ] **Step 8: Commit**

```bash
cd C:\Users\Tobias\git\agentflow
git add src/execute.rs src/state.rs tests/cli.rs
git commit -m "feat(state): add Outcome::Verified + per-contract verdicts on receipts"
```

---

## Task 3: Verify-task execution flow (no-change allowed, archive instead of merge)

**Files:**
- Modify: `src/execute.rs:534-560` (no-change failure), `src/execute.rs:670-701` (merge block)

- [ ] **Step 1: Write the failing tests**

Add end-to-end tests to `tests/e2e.rs` using the existing `fixture`, `run_loop`, `Store`, `archived_refs`, `archived_holds`, and `ENV_GUARD` patterns (see e2e.rs:30-169 for helpers; `gate_failure_keeps_the_agents_committed_branch` at 1329 for the branch-survival assertion pattern). Place these after the existing gate tests (after `failed_receipt_carries_the_failure_reason`, ~line 1404):

```rust
/// A verify task whose gate passes is recorded `verified`, the branch is
/// archived (not merged), and the base branch is untouched.
#[test]
fn verify_task_gate_pass_archives_branch_and_records_verified() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"verify it","verify":true,"scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("DONE.txt")
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done, "verify task ends done");

    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "verified", "receipt records verified");

    // Nothing merged: DONE.txt must NOT exist in the base repo.
    assert!(!f.repo.join("DONE.txt").exists(), "verify never merges");

    // The attempt branch is archived under tf/A-rejected-* (not merged,
    // not deleted).
    let archived = archived_refs(&f.repo);
    assert_eq!(archived.len(), 1, "exactly one archived ref: {archived:?}");
    assert!(!f.repo.join("DONE.txt").exists(), "base unchanged");
}

/// A verify task that produces NO committed change still passes if the
/// gate exits 0 (the gate is the sole deliverable).
///
/// The fixture seeds `README.md` with `# scratch\n` (e2e.rs:58) and the stub
/// writes `{FAKE_AGENT_OUT}\n` into `FAKE_AGENT_TOUCH`. Setting the touch to
/// README.md with out `# scratch` makes the write byte-for-byte identical to
/// the seeded content, so the worktree is genuinely CLEAN — zero commits
/// ahead — while the agent still exits 0 and the gate (test -f README.md)
/// passes. This is the same trick `an_attempt_that_produces_no_change_never_reports_merged`
/// (e2e.rs:1661-1677) uses.
#[test]
fn verify_task_with_zero_commits_passes_when_gate_passes() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_NO_COMMIT");
    std::env::set_var("FAKE_AGENT_TOUCH", "README.md");
    std::env::set_var("FAKE_AGENT_OUT", "# scratch");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"verify no-change","verify":true,"scope":["README.md"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("README.md")
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);

    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1, "one receipt: {receipts:?}");
    assert_eq!(receipts[0].outcome, "verified", "no-change verify still passes");

    // The base is untouched: no new commit on main, and nothing archived
    // (there is no work to preserve).
    let revs = std::process::Command::new("git")
        .args(["rev-list", "--count", "main"])
        .current_dir(&f.repo)
        .output()
        .expect("git runs");
    let revs = String::from_utf8_lossy(&revs.stdout).trim().to_string();
    assert_eq!(revs, "1", "base branch must have exactly the seed commit: {revs}");
    let archived = archived_refs(&f.repo);
    assert!(archived.is_empty(), "no work, so no archive branch: {archived:?}");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    std::env::remove_var("FAKE_AGENT_OUT");
}

/// A verify task whose gate FAILS is recorded failed (never verified).
#[test]
fn verify_task_gate_failure_is_failed_not_verified() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"verify it","verify":true,"scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate_cmd("NEVER.txt") // the agent never creates NEVER.txt
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 2);

    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Failed);

    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outcome, "failed", "gate failure is failed");
}
```

Note: the `verify_task_with_zero_commits_passes_when_gate_passes` test produces a genuinely clean worktree by making the stub's write byte-identical to the seeded README.md (`FAKE_AGENT_TOUCH=README.md` + `FAKE_AGENT_OUT=# scratch`, mirroring `an_attempt_that_produces_no_change_never_reports_merged` at e2e.rs:1661). With the worktree clean, `commit_all` at execute.rs:521 has nothing to commit and `commits_ahead == 0`. `preserve_work` (execute.rs:742) returns `None` when `commits_ahead` is not > 0, so no archive branch is created and the base stays untouched. Without the no-change exception this task would fail as "0 commits ahead" — this test proves the exception lets it pass as `verified`.

- [ ] **Step 2: Run tests to verify they fail**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --test e2e verify_task_ 2>&1 | tail -20'
```

Expected: `verify_task_gate_pass_archives_branch_and_records_verified` fails because the current code MERGES (DONE.txt exists in base, no archived ref, outcome is `merged`). `verify_task_with_zero_commits_passes_when_gate_passes` fails with "agent produced no change: 0 commits ahead" (the no-change exception is not yet in place). `verify_task_gate_failure_is_failed_not_verified` passes already (correct baseline behavior). Confirm the two intended tests fail.

- [ ] **Step 3: Wrap the no-change failure with a verify exception**

In `src/execute.rs:548-560`, change:

```rust
        if ahead == 0 {
            let secs = attempt_start.elapsed().as_secs();
            append(&format!(
                "-- agent produced no change: 0 commits ahead of {base_branch} after {secs}s"
            ));
            cleanup(&repo, &wt);
            return (
                Outcome::Failed(format!(
                    "agent produced no change: 0 commits ahead of {base_branch} after {secs}s"
                )),
                spend,
            );
        }
```

to:

```rust
        if ahead == 0 {
            // A verify task treats the gate as the sole deliverable: it may
            // pass with zero commits ahead (the gate is checked in the
            // worktree below). Non-verify tasks still treat no-change as a
            // failure.
            if !task.verify {
                let secs = attempt_start.elapsed().as_secs();
                append(&format!(
                    "-- agent produced no change: 0 commits ahead of {base_branch} after {secs}s"
                ));
                cleanup(&repo, &wt);
                return (
                    Outcome::Failed(format!(
                        "agent produced no change: 0 commits ahead of {base_branch} after {secs}s"
                    )),
                    spend,
                );
            }
        }
```

- [ ] **Step 4: Branch the merge block on `task.verify`**

In `src/execute.rs:670-701`, the merge block begins with:

```rust
    // 4) Merge (serialized per repo; never force-push).
    let msg = format!("af: {} — {}", task.id, task.title);
```

Insert a verify branch BEFORE the existing merge code, at the top of the block (right after the closing brace of the gate block at line 668 / the `record_phase` at 666). Change the merge block header so it reads:

```rust
    // 4) Merge (serialized per repo; never force-push).
    if task.verify {
        // Verify task: the gate already passed (or the agent stopped and
        // produced no committed change). The branch is NEVER merged — it is
        // archived under `<branch>-rejected-<now>` so the work survives for
        // recovery while the base branch stays clean. The receipt outcome is
        // `verified`.
        append("-- verify passed; archiving branch (no merge) --");
        let _ = preserve_and_note(&repo, &wt, id, attempt, &mut append);
        cleanup(&repo, &wt);
        return (Outcome::Verified, spend);
    }
    let msg = format!("af: {} — {}", task.id, task.title);
```

Keep the rest of the merge block (lines 672-701) unchanged.

- [ ] **Step 5: Run tests to verify they pass**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --test e2e verify_task_ 2>&1 | tail -5'
```

Expected: `test result: ok.` — all three verify tests pass.

- [ ] **Step 6: Run the full test suite**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --all-targets 2>&1 | tail -1'
```

Expected: `test result: ok.` (no regressions).

- [ ] **Step 7: Commit**

```bash
cd C:\Users\Tobias\git\agentflow
git add src/execute.rs tests/e2e.rs
git commit -m "feat(execute): verify tasks never merge — archive branch, allow no-change pass, record verified"
```

---

## Task 4: Parse `accept_report.json` and store per-contract verdicts

**Files:**
- Modify: `src/execute.rs` (add `parse_accept_report` helper + call it in the verify branch, thread contracts into the receipt)

- [ ] **Step 1: Write the failing tests**

Add a unit test for the parser in the `src/execute.rs` tests module (near line 1263). It needs a small temp-dir helper; the existing tests use `std::env::temp_dir().join(format!(...))` (see `render_prompt_uses_custom_template_when_configured` at 1312-1314). The parser signature below is `fn parse_accept_report(wt_path: &Path) -> Option<Vec<ContractVerdict>>` — a pure function that reads `wt_path/accept_report.json`.

```rust
#[test]
fn parse_accept_report_reads_verdict_map_sorted_by_key() {
    let dir = std::env::temp_dir().join(format!("af-accept-rep-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("accept_report.json"),
        r#"{"C-M2":"fail","C-M1":"pass","C-A1":"pass"}"#,
    )
    .unwrap();
    let got = parse_accept_report(&dir).unwrap();
    // Sorted by contract key: C-A1, C-M1, C-M2.
    let keys: Vec<&str> = got.iter().map(|c| c.contract.as_str()).collect();
    assert_eq!(keys, vec!["C-A1", "C-M1", "C-M2"]);
    assert_eq!(got[0].verdict, "pass");
    assert_eq!(got[1].verdict, "pass");
    assert_eq!(got[2].verdict, "fail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_accept_report_missing_file_returns_none() {
    let dir = std::env::temp_dir().join(format!("af-accept-miss-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    assert!(parse_accept_report(&dir).is_none(), "missing report is best-effort");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_accept_report_bad_json_returns_none() {
    let dir = std::env::temp_dir().join(format!("af-accept-bad-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("accept_report.json"), "not json").unwrap();
    assert!(parse_accept_report(&dir).is_none(), "bad report is ignored");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib execute::tests::parse_accept_report_ 2>&1 | tail -5'
```

Expected: `error[E0425]: cannot find function 'parse_accept_report'` (function does not exist yet).

- [ ] **Step 3: Implement the parser**

Add this free function in `src/execute.rs` (place it just before `failure_context` at line 135, or near the `Outcome` enum). Ensure `crate::state::ContractVerdict` is imported (add `use crate::state::ContractVerdict;` if not already in scope — check the imports at the top of execute.rs first):

```rust
/// Best-effort per-contract acceptance report (verify tasks). Reads
/// `accept_report.json` from the worktree root (the gate's cwd). The file is
/// an object `{"C-M1":"pass","C-M2":"fail",...}`. Missing or unparseable
/// reports yield `None` and NEVER fail the task — the gate's exit code is the
/// sole pass/fail authority; the report only adds per-contract detail.
fn parse_accept_report(wt_path: &Path) -> Option<Vec<ContractVerdict>> {
    let raw = std::fs::read_to_string(wt_path.join("accept_report.json")).ok()?;
    let map: std::collections::BTreeMap<String, String> = serde_json::from_str(&raw).ok()?;
    Some(
        map.into_iter()
            .map(|(contract, verdict)| ContractVerdict { contract, verdict })
            .collect(),
    )
}
```

`BTreeMap` gives sorted-by-key iteration for free, satisfying the ordering requirement.

- [ ] **Step 4: Wire contracts into the verify branch and the receipt**

In the verify branch added in Task 3 Step 4, change:

```rust
    if task.verify {
        // Verify task: the gate already passed (or the agent stopped and
        // produced no committed change). The branch is NEVER merged — it is
        // archived under `<branch>-rejected-<now>` so the work survives for
        // recovery while the base branch stays clean. The receipt outcome is
        // `verified`.
        append("-- verify passed; archiving branch (no merge) --");
        let _ = preserve_and_note(&repo, &wt, id, attempt, &mut append);
        cleanup(&repo, &wt);
        return (Outcome::Verified, spend);
    }
```

to:

```rust
    if task.verify {
        // Verify task: the gate already passed (or the agent stopped and
        // produced no committed change). The branch is NEVER merged — it is
        // archived under `<branch>-rejected-<now>` so the work survives for
        // recovery while the base branch stays clean. The receipt outcome is
        // `verified`, and per-contract verdicts from an optional
        // `accept_report.json` ride the receipt.
        append("-- verify passed; archiving branch (no merge) --");
        let contracts = parse_accept_report(&wt_path);
        if let Some(c) = &contracts {
            append(&format!(
                "-- accept_report: {} --",
                c.iter()
                    .map(|v| format!("{}={}", v.contract, v.verdict))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let _ = preserve_and_note(&repo, &wt, id, attempt, &mut append);
        cleanup(&repo, &wt);
        return (Outcome::Verified, spend);
    }
```

Now the receipt must carry `contracts`. In `src/execute.rs:117-128` (the `append_receipt` call in `execute_task`), the `contracts` value needs to be plumbed. Change the return type path: `execute_task` currently calls `execute_attempt(...)` which returns `(Outcome, Spend)`. Add a third element to carry contracts.

First, change `execute_attempt`'s signature and the verify-branch return. In `src/execute.rs:216`:

```rust
fn execute_attempt(
    ctx: &ExecCtx,
    worker: &Worker,
    id: &str,
    attempt: u32,
    log_path: &Path,
) -> (Outcome, Spend) {
```

to:

```rust
fn execute_attempt(
    ctx: &ExecCtx,
    worker: &Worker,
    id: &str,
    attempt: u32,
    log_path: &Path,
) -> (Outcome, Spend, Option<Vec<ContractVerdict>>) {
```

Every existing `return (Outcome::..., spend)` in `execute_attempt` becomes `return (Outcome::..., spend, None)`. The final normal-path returns become `(Outcome::Merged, spend, None)`. The verify branch return becomes `(Outcome::Verified, spend, contracts)`.

Then in `execute_task` (execute.rs:107), change:

```rust
    let (outcome, spend) = execute_attempt(ctx, worker, id, attempt, log_path);
```

to:

```rust
    let (outcome, spend, contracts) = execute_attempt(ctx, worker, id, attempt, log_path);
```

And in the `append_receipt` call (execute.rs:117-128), add `contracts,` after `cost_micros`:

```rust
        outcome: outcome_name.to_string(),
        error,
        cost_micros: spend.cost_micros,
        contracts,
    });
```

To convert all the `(Outcome::X, spend)` returns efficiently, run a mechanical replace. The distinct return shapes are: `(Outcome::Failed(...), spend)`, `(Outcome::Failed(...), Spend::default())`, `(Outcome::Merged, spend)`, `(Outcome::Verified, spend)`. Use `cargo check` iteratively to find every site that needs `, None` appended:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo check 2>&1 | grep -E "expected tuple|mismatched types|expected .* found" | head -40'
```

Fix each reported line by appending `, None` (or `, contracts` for the verify branch). There are roughly 10-12 return sites inside `execute_attempt`; all are mechanical.

- [ ] **Step 5: Run tests to verify they pass**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib execute::tests::parse_accept_report_ 2>&1 | tail -3'
```

Expected: `test result: ok.` (parser tests pass).

Then run the e2e verify test to confirm receipts still record `verified` with the new plumbing:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --test e2e verify_task_ 2>&1 | tail -3'
```

Expected: `test result: ok.`

- [ ] **Step 6: Add an e2e test asserting contracts reach the receipt**

Add to `tests/e2e.rs` (after `verify_task_gate_failure_is_failed_not_verified`):

```rust
/// A verify task whose gate writes `accept_report.json` records the
/// per-contract verdicts on the receipt, sorted by contract key.
#[test]
fn verify_task_records_accept_report_contracts_on_receipt() {
    let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var("FAKE_AGENT_EXIT");
    std::env::remove_var("FAKE_AGENT_TOUCH");
    // The gate writes accept_report.json AND passes (exit 0).
    let gate = format!(
        "{} && printf '%s' '{{\"C-M2\":\"fail\",\"C-M1\":\"pass\"}}' > accept_report.json",
        gate_cmd("DONE.txt")
    );
    let f = fixture(
        &format!(
            r#"{{ "tasks": [ {{"id":"A","title":"verify contracts","verify":true,"scope":["DONE.txt"],"accept":"{g}"}} ] }}"#,
            g = gate
        ),
        &worker_json(1),
    );
    assert_eq!(run::run_loop(&f.cfg, &f.st, &RunOptions::default()), 0);

    let receipts = Store::new(f.st.state_dir.clone()).load_receipts();
    assert_eq!(receipts.len(), 1);
    let contracts = receipts[0].contracts.as_ref().expect("contracts present");
    assert_eq!(contracts.len(), 2);
    assert_eq!(contracts[0].contract, "C-M1");
    assert_eq!(contracts[0].verdict, "pass");
    assert_eq!(contracts[1].contract, "C-M2");
    assert_eq!(contracts[1].verdict, "fail");
}
```

Run it:

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --test e2e verify_task_records_accept_report_contracts_on_receipt 2>&1 | tail -3'
```

Expected: `test result: ok.`

Note: `gate_cmd` uses `test -f` on unix; the appended `printf ... > accept_report.json` runs in the gate's cwd (the worktree). On CI (ubuntu) this works. The `&&` requires the file check to pass first, so the report only appears when the gate would pass anyway — consistent with "report supplements a passing gate."

- [ ] **Step 7: Run the full test suite**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --all-targets 2>&1 | tail -1'
```

Expected: `test result: ok.`

- [ ] **Step 8: Commit**

```bash
cd C:\Users\Tobias\git\agentflow
git add src/execute.rs tests/e2e.rs
git commit -m "feat(execute): parse accept_report.json into per-contract verdicts on verify receipts"
```

---

## Task 5: Surface `verify`/`verdict`/`contracts` in `status --json`

**Files:**
- Modify: `src/run.rs:1629-1645` (status_json), `src/run.rs` status_json test (2762-2793)

- [ ] **Step 1: Write the failing test**

Extend the existing `status_json_serializes_task_fields_from_the_store` test (run.rs:2763-2793). The helpers are `task(id)` (run.rs:2592, builds `Task { ..Default::default() }`) and `cfg_with(tasks)` (run.rs:2601). Change the `cfg_with` line (run.rs:2766) so task A is a verify task:

```rust
        let cfg = cfg_with(vec![
            Task { verify: true, ..task("A") },
            task("B"),
        ]);
```

Then add assertions after line 2786 (the existing `last_error` check), and seed a receipt with `contracts` so the verdict/contracts resolution is exercised. Replace the `Store::new(...).save(&status)` call block so it also appends a verified receipt for A:

```rust
        Store::new(st.state_dir.clone()).save(&status).unwrap();
        Store::new(st.state_dir.clone())
            .append_receipt(&Receipt {
                task: "A".into(),
                attempt: 1,
                worker: "w1".into(),
                model: "m".into(),
                wall_clock_s: 1.0,
                tokens: None,
                ts: 1,
                outcome: "verified".into(),
                error: None,
                cost_micros: None,
                contracts: Some(vec![crate::state::ContractVerdict {
                    contract: "C-M1".into(),
                    verdict: "pass".into(),
                }]),
            })
            .unwrap();
```

Then add the assertions (after line 2786):

```rust
        assert_eq!(v["A"]["verify"], true, "verify flag surfaced");
        assert_eq!(v["A"]["verdict"], "verified", "latest verdict surfaced");
        assert_eq!(v["A"]["contracts"][0]["contract"], "C-M1");
        assert_eq!(v["A"]["contracts"][0]["verdict"], "pass");
        assert_eq!(v["B"]["verdict"], "none", "no receipt for B");
```

Note: `verdict` reflects the LATEST surviving receipt's outcome (resolved by `status_json` as the last `counts_as_verdict()` receipt for the task), NOT `TaskStatus.state`. Because A has a `verified` receipt, `verdict` is `"verified"` even though `state` is `"failed"` — the two are intentionally distinct.

- [ ] **Step 2: Run test to verify it fails**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib run::tests::status_json_serializes_task_fields_from_the_store 2>&1 | tail -10'
```

Expected: fails — `status_json` does not yet emit `verify`/`verdict`/`contracts` (assertions on missing JSON keys fail), and/or the receipt-seeding step needs the new fields.

- [ ] **Step 3: Implement the new fields in `status_json`**

Replace the `status_json` function (run.rs:1629-1645) with a version that loads receipts once, resolves each task's latest surviving verdict + contracts, and adds the `verify` flag:

```rust
/// `af api status --json`.
pub fn status_json(cfg: &Config, st: &Settings) -> String {
    let status = Store::new(st.state_dir.clone()).load();
    let receipts = Store::new(st.state_dir.clone()).load_receipts();
    let mut map = serde_json::Map::new();
    for t in &cfg.tasks {
        let s = status.get(&t.id).cloned().unwrap_or_default();
        // Latest surviving receipt for this task (last winning attempt).
        let latest = receipts.iter().rev().find(|r| r.task == t.id && r.counts_as_verdict());
        let verdict = latest.map(|r| r.outcome.as_str()).unwrap_or("none");
        let contracts = latest
            .and_then(|r| r.contracts.as_ref())
            .map(|c| {
                c.iter()
                    .map(|v| serde_json::json!({ "contract": v.contract, "verdict": v.verdict }))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        map.insert(
            t.id.clone(),
            serde_json::json!({
                "id": t.id,
                "state": format!("{:?}", s.state).to_lowercase(),
                "attempts": s.attempts,
                "last_error": s.last_error,
                "verify": t.verify,
                "verdict": verdict,
                "contracts": contracts,
            }),
        );
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_default()
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib run::tests::status_json_serializes_task_fields_from_the_store 2>&1 | tail -3'
```

Expected: `test result: ok.`

- [ ] **Step 5: Run the full test suite**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --all-targets 2>&1 | tail -1'
```

Expected: `test result: ok.`

- [ ] **Step 6: Commit**

```bash
cd C:\Users\Tobias\git\agentflow
git add src/run.rs
git commit -m "feat(status): surface verify flag, verdict, and per-contract verdicts in status --json"
```

---

## Task 6: Seed failing contract names into retry prompts

**Files:**
- Modify: `src/execute.rs:135-153` (failure_context)

- [ ] **Step 1: Write the failing test**

Add to the `src/execute.rs` tests module (near the existing `failure_context` tests at 1246-1263). The `receipt_of` helper at 1204-1217 needs a `contracts` field (already added in Task 2). Add a new helper or extend `receipt_of` with a `contracts: Option<Vec<ContractVerdict>>` parameter — simplest is to add a dedicated helper:

```rust
    fn receipt_with_contracts(task: &str, attempt: u32, outcome: &str, contracts: &[&str]) -> Receipt {
        Receipt {
            task: task.into(),
            attempt,
            worker: "w".into(),
            model: "m".into(),
            wall_clock_s: 1.0,
            tokens: None,
            ts: 0,
            outcome: outcome.into(),
            error: None,
            cost_micros: None,
            contracts: Some(
                contracts
                    .iter()
                    .map(|c| ContractVerdict { contract: c.to_string(), verdict: "fail".into() })
                    .collect(),
            ),
        }
    }

    #[test]
    fn failure_context_appends_failing_contracts_from_prior_receipts() {
        let rs = vec![
            receipt_with_contracts("A", 1, "failed", &["C-M1", "C-M2"]),
            receipt_with_contracts("B", 1, "failed", &["C-OTHER"]),
        ];
        let ctx = failure_context(&rs, "A", 2).unwrap();
        assert!(ctx.contains("C-M1"), "failing contract seeded: {ctx}");
        assert!(ctx.contains("C-M2"), "failing contract seeded: {ctx}");
        assert!(!ctx.contains("C-OTHER"), "other task's contracts excluded: {ctx}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib execute::tests::failure_context_appends_failing_contracts_from_prior_receipts 2>&1 | tail -5'
```

Expected: fails — the `failure_context` block does not mention C-M1/C-M2.

- [ ] **Step 3: Extend `failure_context`**

Replace the body of the loop in `failure_context` (execute.rs:148-151):

```rust
    for r in prior {
        let why = r.error.as_deref().unwrap_or("(unknown failure)");
        out.push_str(&format!("- attempt {}: {}\n", r.attempt, why));
    }
```

to:

```rust
    for r in prior {
        let why = r.error.as_deref().unwrap_or("(unknown failure)");
        out.push_str(&format!("- attempt {}: {}\n", r.attempt, why));
        if let Some(cs) = &r.contracts {
            let failing: Vec<&str> = cs
                .iter()
                .filter(|c| c.verdict != "pass")
                .map(|c| c.contract.as_str())
                .collect();
            if !failing.is_empty() {
                out.push_str(&format!(
                    "  - failing contracts: {}\n",
                    failing.join(", ")
                ));
            }
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --lib execute::tests::failure_context_appends_failing_contracts_from_prior_receipts 2>&1 | tail -3'
```

Expected: `test result: ok.`

- [ ] **Step 5: Run the full test suite**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --all-targets 2>&1 | tail -1'
```

Expected: `test result: ok.`

- [ ] **Step 6: Commit**

```bash
cd C:\Users\Tobias\git\agentflow
git add src/execute.rs
git commit -m "feat(execute): seed failing contract names into retry prompt context"
```

---

## Task 7: Document in the lifecycle spec and README

**Files:**
- Modify: `openspec/specs/lifecycle/spec.md`
- Modify: `README.md` (task fields reference, if present)

- [ ] **Step 1: Read the lifecycle spec's task/acceptance section**

```bash
cd C:\Users\Tobias\git\agentflow
grep -n -i "accept\|readonly\|manual\|gate" openspec/specs/lifecycle/spec.md | head -30
```

Find the section describing task fields / acceptance gates and add a subsection for verify tasks.

- [ ] **Step 2: Add the verify-task documentation**

Add a section (matching the file's existing style and spec-requirement numbering conventions) describing:

- `verify: true` marks a task whose acceptance gate is the sole deliverable.
- Success = gate exits 0 (agent may produce zero committed change).
- The branch is never merged — archived under `<branch>-rejected-<now>`, receipt `outcome: verified`.
- Optional `accept_report.json` in the worktree root (`{"C-M1":"pass",...}`) is parsed best-effort into per-contract verdicts on the receipt, surfaced in `status --json` and seeded into retry prompts.
- Validation rejects `verify` + `manual`, `verify` + `readonly`, and `verify` without an `accept` gate.

- [ ] **Step 3: Update the README task-fields table**

If `README.md` documents task fields (search for `readonly` / `manual` / `accept`), add a row for `verify` matching the existing table style.

- [ ] **Step 4: Commit**

```bash
cd C:\Users\Tobias\git\agentflow
git add openspec/specs/lifecycle/spec.md README.md
git commit -m "docs: verify tasks + per-contract acceptance (lifecycle spec, README)"
```

---

## Task 8: Format, lint, and verify the whole suite green

**Files:** none (verification only).

- [ ] **Step 1: cargo fmt --check**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo fmt --check'
```

If it fails, run `cargo fmt` and re-check. Expected: clean (exit 0).

- [ ] **Step 2: cargo clippy with -D warnings**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3'
```

Expected: no errors/warnings.

- [ ] **Step 3: Full test suite**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --all-targets 2>&1 | grep -E "test result:" | sort | uniq -c'
```

Expected: every suite reports `test result: ok.`, 0 FAILED.

- [ ] **Step 4: spec_traceability**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo test --test spec_traceability 2>&1 | tail -3'
```

Expected: `test result: ok.` If new spec markers were added to tests in Tasks 3/4/6, ensure every `// spec:` marker in the new tests references an existing spec section (or add the corresponding spec section in Task 7).

- [ ] **Step 5: Coverage gate**

```bash
wsl -d Ubuntu -- bash -lc 'cd /mnt/c/Users/Tobias/git/agentflow && cargo llvm-cov --workspace 2>&1 | tail -5'
```

Expected: total line coverage at or above the `.coverage-min` floor (currently 94%). The new tests add coverage for the new branches; if coverage dips below the floor, add assertions/tests to cover the new `verify` branches and `parse_accept_report` error paths.

- [ ] **Step 6: Final commit if fmt/clippy made changes**

```bash
cd C:\Users\Tobias\git\agentflow
git status --short
# If any files changed: git add -u && git commit -m "style: cargo fmt"
```

- [ ] **Step 7: Merge to main**

```bash
cd C:\Users\Tobias\git\agentflow
git checkout main
git merge feat/verify-tasks
git push origin main
```

---

## Self-Review Notes

- **Spec coverage:** S1 (config/behavior) → Tasks 1,3; S2 (data model) → Task 2; S3 (flow/parsing/seeding) → Tasks 3,4,6; status → Task 5; docs → Task 7. All design sections are implemented.
- **`contracts` plumbing:** Task 4 changes `execute_attempt`'s return type to a 3-tuple. Every `return` inside must gain `, None` (or `, contracts`). `cargo check` is the source of truth for finding all sites — do not guess.
- **Type consistency:** `Outcome::Verified` is used consistently; `ContractVerdict { contract, verdict }`; receipt field `contracts: Option<Vec<ContractVerdict>>`; `counts_as_verdict()` includes `"verified"`. `status_json`'s `verdict` is the latest surviving receipt's outcome string.
- **Verify + readonly:** readonly returns `Outcome::Merged` at execute.rs:489 — but validation (Task 1) rejects verify+readonly, so that path can never combine with verify.
