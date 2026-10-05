//! Contract tests for the acceptance-gate runner, [`agentflow::gate::run_accept`].
//!
//! `run_accept` is line-covered by its own unit tests, but WHAT CALLERS MAY
//! RELY ON lived nowhere in one readable place. This file pins it: exit-code
//! classification, the environment the gate observes, the working directory,
//! the hard timeout, large-output capture (the classic subprocess-pipe
//! deadlock), and missing-command handling.
//!
//! These assertions were read from `src/gate.rs` and `src/subprocess.rs`,
//! not guessed:
//!
//! * the gate is `sh -c <accept>` on unix (`cmd /C <accept>` on windows);
//! * the child INHERITS the full parent environment (`EnvMode::Inherit`),
//!   then the caller-supplied `env` pairs are applied on top, then
//!   `TF_GATE_REPLAY`;
//! * `TF_GATE_REPLAY` is the literal string `"1"` when `replay == true` and
//!   `"0"` when `replay == false`. The runner synthesizes NO `TF_TASK_ID` /
//!   `TF_ACCEPTANCE`: per-task values travel through the caller's `env`
//!   argument (populated from `Settings.gate_env` ← `TF_GATE_ENV`), so that
//!   is exactly what this file pins;
//! * exit 0 ⇒ `CmdKind::Success`; any other issue ⇒ `CmdKind::NonZero`
//!   (with `code`), `CmdKind::Timeout` on timeout, `CmdKind::Missing` when
//!   the shell/command cannot be spawned at all.
//!
//! Uses only std + the crate. Every test gets its own uniquely-named temp dir,
//! so the suite stays independent under cargo's parallel test runner.

#![cfg(unix)]

use agentflow::gate::run_accept;
use agentflow::subprocess::CmdKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// A fresh, uniquely-named temp directory (one per call).
fn fresh_dir(label: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "af-gate-contract-{label}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create unique temp dir");
    d
}

/// Build the caller-supplied env pairs the way `Settings.gate_env` does.
fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// One place to pin the gate runner's exit-code *and* environment contract:
/// exit 0 passes, a non-zero exit fails carrying the gate's output, and the
/// replay decision reaches the gate as the literal `"1"`/`"0"`.
// spec: lifecycle/subprocess-execution-contract
#[test]
fn gate_contract_exit_codes_and_env() {
    let d = fresh_dir("umbrella");

    // exit 0 = pass (`Success`, code 0).
    let ok = run_accept("exit 0", &d, &[], Duration::from_secs(5), true);
    assert!(ok.passed(), "exit 0 must pass");
    assert_eq!(ok.kind, CmdKind::Success);
    assert_eq!(ok.code, Some(0));

    // non-zero = fail, and the gate's captured output rides along.
    let bad = run_accept(
        "echo gate-said-this; exit 9",
        &d,
        &[],
        Duration::from_secs(5),
        true,
    );
    assert!(!bad.passed(), "exit 9 must fail");
    assert_eq!(bad.kind, CmdKind::NonZero);
    assert_eq!(bad.code, Some(9));
    assert!(
        bad.combined().contains("gate-said-this"),
        "failure carries captured output: {:?}",
        bad.combined()
    );

    // The replay flag is rendered verbatim as "1" / "0" (see src/gate.rs).
    let on = run_accept(
        "printf '%s' \"$TF_GATE_REPLAY\"",
        &d,
        &[],
        Duration::from_secs(5),
        true,
    );
    assert_eq!(on.stdout, "1", "replay == true ⇒ TF_GATE_REPLAY=\"1\"");
    let off = run_accept(
        "printf '%s' \"$TF_GATE_REPLAY\"",
        &d,
        &[],
        Duration::from_secs(5),
        false,
    );
    assert_eq!(off.stdout, "0", "replay == false ⇒ TF_GATE_REPLAY=\"0\"");
}

/// A gate that exits 0 passes; one that exits 1, 2 or 127 fails. The failure
/// carries the gate's captured stdout AND stderr.
// spec: lifecycle/execute-pipeline
#[test]
fn gate_exit_zero_passes_and_non_zero_fails() {
    let d = fresh_dir("exit-codes");

    let pass = run_accept("exit 0", &d, &[], Duration::from_secs(5), true);
    assert!(pass.passed(), "exit 0 is a pass");
    assert_eq!(pass.kind, CmdKind::Success);

    for code in [1, 2, 127] {
        let script =
            format!("echo stdout-marker-{code}; echo stderr-marker-{code} >&2; exit {code}");
        let out = run_accept(&script, &d, &[], Duration::from_secs(5), true);
        assert!(!out.passed(), "exit {code} must fail");
        assert_eq!(out.kind, CmdKind::NonZero, "exit {code} classification");
        assert_eq!(out.code, Some(code), "exit {code} captured verbatim");
        assert!(
            out.stdout.contains(&format!("stdout-marker-{code}")),
            "stdout captured for exit {code}: {:?}",
            out.stdout
        );
        assert!(
            out.stderr.contains(&format!("stderr-marker-{code}")),
            "stderr captured for exit {code}: {:?}",
            out.stderr
        );
    }
}

// spec: lifecycle/subprocess-execution-contract
#[test]
fn gate_receives_the_task_env_contract() {
    let d = fresh_dir("env");

    // The runner synthesizes ONLY `TF_GATE_REPLAY`; task id / acceptance
    // values are caller-supplied pairs (Settings.gate_env ← TF_GATE_ENV).
    // Model them explicitly and pin the exact strings the gate observes.
    let declared = pairs(&[
        ("AF_GATE_TASK_ID", "r5-contract-gate"),
        ("AF_GATE_ACCEPTANCE", "cargo test"),
    ]);
    let script = "printf 'replay=%s\\ntask=%s\\naccept=%s\\n' \
                  \"$TF_GATE_REPLAY\" \"$AF_GATE_TASK_ID\" \"$AF_GATE_ACCEPTANCE\"";

    let on = run_accept(script, &d, &declared, Duration::from_secs(5), true);
    assert!(on.passed(), "env-probe gate must pass: {:?}", on.combined());
    assert_eq!(
        on.stdout, "replay=1\ntask=r5-contract-gate\naccept=cargo test\n",
        "declared pairs + TF_GATE_REPLAY=\"1\" all reach the gate"
    );

    let off = run_accept(script, &d, &declared, Duration::from_secs(5), false);
    assert!(off.passed());
    assert_eq!(
        off.stdout, "replay=0\ntask=r5-contract-gate\naccept=cargo test\n",
        "the same pairs reach the gate with TF_GATE_REPLAY=\"0\""
    );

    // Inherit mode: the parent environment is not stripped.
    let inherited = run_accept("test -n \"$PATH\"", &d, &[], Duration::from_secs(5), true);
    assert!(
        inherited.passed(),
        "EnvMode::Inherit keeps the parent env (PATH must be visible)"
    );
}

/// The gate runs in the directory it was given, so a relative write lands
/// there — not in the orchestrator's current working directory.
// spec: lifecycle/execute-pipeline
#[test]
fn gate_runs_in_the_worktree_directory() {
    let d = fresh_dir("cwd");
    let name = format!("af_gate_landing_{}.txt", std::process::id());
    let script = format!("printf 'landed' > {name}");

    let out = run_accept(&script, &d, &[], Duration::from_secs(5), true);
    assert!(
        out.passed(),
        "relative write gate must pass: {:?}",
        out.combined()
    );

    let landed = d.join(&name);
    assert!(
        landed.exists(),
        "relative file must land in the cwd passed to run_accept ({})",
        landed.display()
    );
    assert_eq!(std::fs::read_to_string(&landed).unwrap(), "landed");

    // And it must NOT appear relative to the orchestrator's own cwd.
    let here = std::env::current_dir().unwrap().join(&name);
    assert!(
        !here.exists(),
        "relative file must not leak into the orchestrator's cwd ({})",
        here.display()
    );

    let _ = std::fs::remove_dir_all(&d);
}

/// A gate that sleeps far past the configured timeout is killed and reported
/// as a timeout FAILURE — and, critically, the call RETURNS promptly instead
/// of waiting out the sleep.
// spec: lifecycle/subprocess-execution-contract
#[test]
fn gate_timeout_is_reported_as_a_failure() {
    let d = fresh_dir("timeout");
    let start = Instant::now();
    let out = run_accept("sleep 60", &d, &[], Duration::from_millis(500), true);
    let elapsed = start.elapsed();

    assert!(!out.passed(), "a timed-out gate is a failure");
    assert_eq!(
        out.kind,
        CmdKind::Timeout,
        "timeout is classified as Timeout"
    );
    assert!(
        format!("{:?}", out.kind).to_lowercase().contains("timeout"),
        "failure classification names the timeout"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "run_accept must return after the timeout instead of waiting out the \
         60s sleep (took {elapsed:?})"
    );

    let _ = std::fs::remove_dir_all(&d);
}

/// A gate printing more than a pipe buffer must complete and have its output
/// fully captured — the classic subprocess-pipe deadlock. We emit > 256 KiB
/// (4096 lines × 65 bytes ≈ 260 KiB), far beyond a typical 64 KiB pipe.
// spec: lifecycle/subprocess-execution-contract
#[test]
fn gate_with_large_output_does_not_deadlock() {
    let d = fresh_dir("large-output");
    // `%064d` gives a 64-char line; +1 newline = 65 bytes per iteration.
    let script = "i=0; while [ $i -lt 4096 ]; do printf '%064d\\n' \"$i\"; i=$((i+1)); done";

    let start = Instant::now();
    let out = run_accept(script, &d, &[], Duration::from_secs(30), true);
    let elapsed = start.elapsed();

    assert!(
        out.passed(),
        "large-output gate must complete and pass: {:?}",
        out.combined()
    );
    assert!(
        out.stdout.len() >= 256 * 1024,
        "captured at least 256 KiB (got {} bytes)",
        out.stdout.len()
    );
    assert_eq!(
        out.stdout.lines().count(),
        4096,
        "every line is captured — no pipe deadlock, no truncation"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "must not stall on the pipe buffer (took {elapsed:?})"
    );

    let _ = std::fs::remove_dir_all(&d);
}

/// A gate naming a non-existent command yields a FAILURE result, never a
/// panic or a hang: `sh -c` reports command-not-found and exits 127.
#[test]
fn missing_gate_command_fails_without_panicking() {
    let d = fresh_dir("missing-command");
    let out = run_accept(
        "/af-no-such-binary-xyz --nope",
        &d,
        &[],
        Duration::from_secs(5),
        true,
    );

    assert!(
        !out.passed(),
        "a non-existent command is a failure, not a panic"
    );
    assert_eq!(out.kind, CmdKind::NonZero);
    assert_eq!(
        out.code,
        Some(127),
        "sh reports command-not-found as exit 127"
    );
    assert!(
        !out.stderr.is_empty(),
        "the shell's diagnostic is captured: {:?}",
        out.stderr
    );

    let _ = std::fs::remove_dir_all(&d);
}
