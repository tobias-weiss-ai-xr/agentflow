//! Subprocess helper: the single, uniform way `af` runs git, the agent CLI,
//! and acceptance gates (arc42 ch. 8.3). Captures output without pipe
//! deadlock, applies a hard timeout, kills on timeout, and classifies the
//! result by exit-code contract.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdKind {
    Success,
    NonZero,
    Killed,
    Timeout,
    /// The stall watchdog fired (see [`run_with_stall`]): the child
    /// produced no output for the configured window and was killed
    /// before the total `timeout` expired.
    Stalled,
    /// Binary not found on PATH.
    Missing,
}

#[derive(Debug, Clone)]
pub struct CmdOut {
    pub kind: CmdKind,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOut {
    pub fn passed(&self) -> bool {
        self.kind == CmdKind::Success
    }
    pub fn combined(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else {
            format!("{}\n{}", self.stdout.trim_end(), self.stderr)
        }
    }
}

/// Environment policy for a spawned child (sandbox layer 1).
#[derive(Debug, Clone)]
pub enum EnvMode {
    /// Inherit the parent environment. Trusted callers only: af's own git
    /// operations and user-authored acceptance gates.
    Inherit,
    /// Start EMPTY; take the named keys from the parent env. Used for the
    /// untrusted agent CLI. `env` pairs are always applied on top.
    Allowlist(Vec<String>),
}

/// Run a child to completion with a hard total timeout. Signature and
/// behaviour are pinned (callers in gate.rs, worktree.rs, run.rs, tests) —
/// see [`run_with_stall`] for the full contract.
pub fn run(
    cmd: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
    env_mode: EnvMode,
    timeout: Duration,
) -> CmdOut {
    run_with_stall(cmd, args, cwd, env, env_mode, timeout, None)
}

/// [`run`] plus an OPTIONAL stall watchdog. `stall: None` is exactly the
/// legacy behaviour (bounded only by `timeout`). `stall: Some(window)`
/// additionally kills the child when it has produced NO output (stdout
/// nor stderr) for `window`, classifying the result [`CmdKind::Stalled`]
/// — a hung agent stops burning the clock (and paid tokens) instead of
/// sitting out the whole `timeout`.
///
/// The captured stdout/stderr are the FULL output the child wrote on both
/// paths; after a timeout or stall kill the buffers are snapshotted with
/// a 200ms grace instead of joined (orphaned grandchildren can hold the
/// pipe write-ends forever).
pub fn run_with_stall(
    cmd: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
    env_mode: EnvMode,
    timeout: Duration,
    stall: Option<Duration>,
) -> CmdOut {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    match env_mode {
        EnvMode::Inherit => {
            for (k, v) in env {
                c.env(k, v);
            }
        }
        EnvMode::Allowlist(keys) => {
            c.env_clear();
            for k in &keys {
                if let Ok(v) = std::env::var(k) {
                    c.env(k, v);
                }
            }
            for (k, v) in env {
                c.env(k, v);
            }
        }
    }
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());

    let mut child = match c.spawn() {
        Ok(ch) => ch,
        // Missing covers both a truly absent binary and transient spawn
        // failures (e.g. fork under memory pressure on CI runners); the OS
        // error text lands in stderr so diagnostics can tell them apart.
        Err(e) => {
            return CmdOut {
                kind: CmdKind::Missing,
                code: None,
                stdout: String::new(),
                stderr: e.to_string(),
            }
        }
    };

    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let buf1 = Arc::new(Mutex::new(String::new()));
    let buf2 = Arc::new(Mutex::new(String::new()));

    // Stall-watchdog substrate: a shared time anchor (created BEFORE the
    // readers are spawned) plus one "last output" timestamp per pipe,
    // published after every non-empty chunk. The main loop computes
    // `idle = anchor.elapsed() - max(last_stdout, last_stderr)`.
    let anchor = Instant::now();
    let last_stdout = Arc::new(AtomicU64::new(0));
    let last_stderr = Arc::new(AtomicU64::new(0));

    let r1 = {
        let b1 = buf1.clone();
        let last = last_stdout.clone();
        thread::spawn(move || {
            read_incrementally(&mut so, &b1, &last, anchor);
        })
    };
    let r2 = {
        let b2 = buf2.clone();
        let last = last_stderr.clone();
        thread::spawn(move || {
            read_incrementally(&mut se, &b2, &last, anchor);
        })
    };

    let start = Instant::now();
    let mut status = None;
    let mut kind = CmdKind::NonZero;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                status = Some(st);
                break;
            }
            Ok(None) => {}
            Err(e) => {
                // Rare OS error while waiting; treat as non-zero and stop polling.
                eprintln!("subprocess wait error: {e}");
                break;
            }
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            kind = CmdKind::Timeout;
            // ponytail: post-kill output is best-effort (200ms grace) —
            // orphaned grandchildren can hold the pipes; kill the process
            // tree via a wrapper command if that ever matters.
            break;
        }
        // Stall watchdog: no output on either pipe for `window` ⇒ kill
        // NOW, long before the total `timeout` would. A silent child is
        // indistinguishable from a working one to the exit poll above —
        // this is the only activity signal.
        if let Some(window) = stall {
            let now_ms = anchor.elapsed().as_millis() as u64;
            let last_ms = last_stdout
                .load(Ordering::SeqCst)
                .max(last_stderr.load(Ordering::SeqCst));
            if now_ms.saturating_sub(last_ms) >= window.as_millis() as u64 {
                let _ = child.kill();
                let _ = child.wait();
                kind = CmdKind::Stalled;
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    if status.is_some() && kind != CmdKind::Timeout && kind != CmdKind::Stalled {
        let code = status.as_ref().and_then(|s| s.code());
        kind = match code {
            Some(0) => CmdKind::Success,
            Some(_) => CmdKind::NonZero,
            None => CmdKind::Killed, // terminated by signal
        }
    }

    // On a timeout OR stall kill, orphaned grandchildren may hold the pipe
    // write-ends — joining the readers would block until THEY exit.
    // Snapshot instead (same 200ms grace as the timeout path).
    let best_effort = matches!(kind, CmdKind::Timeout | CmdKind::Stalled);
    let (stdout, stderr) = if best_effort {
        thread::sleep(Duration::from_millis(200));
        (buf1.lock().unwrap().clone(), buf2.lock().unwrap().clone())
    } else {
        let _ = r1.join();
        let _ = r2.join();
        (buf1.lock().unwrap().clone(), buf2.lock().unwrap().clone())
    };
    let code = status.as_ref().and_then(|s| s.code());

    CmdOut {
        kind,
        code,
        stdout,
        stderr,
    }
}

/// Read one pipe incrementally: a fixed-buffer `Read` loop that keeps
/// DRAINING the pipe (that is what prevents a child writing more than a
/// pipe buffer from blocking), accumulates the full output into the shared
/// buffer at EOF, and publishes a "last output" timestamp after every
/// non-empty chunk for the stall watchdog. `Ok(0)` is EOF; an `Err` (pipe
/// torn down) ends the loop with whatever was captured.
fn read_incrementally<R: Read>(
    pipe: &mut R,
    shared: &Mutex<String>,
    last_output: &AtomicU64,
    anchor: Instant,
) {
    let mut s = String::new();
    let mut carry: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => {
                let before = s.len();
                push_chunk(&mut s, &mut carry, &chunk[..n]);
                // Mirror the new text into the shared buffer AS IT ARRIVES:
                // after a timeout/stall kill, orphaned grandchildren can
                // hold the pipe write-ends so this reader may never reach
                // EOF — the grace-period snapshot must still see every
                // chunk already drained.
                *shared.lock().unwrap() += &s[before..];
                last_output.store(anchor.elapsed().as_millis() as u64, Ordering::SeqCst);
            }
            Err(_) => break, // read error — keep what we have
        }
    }
    // A dangling incomplete UTF-8 sequence at EOF degrades to U+FFFD
    // rather than being dropped.
    if !carry.is_empty() {
        s.push('\u{FFFD}');
    }
    *shared.lock().unwrap() = s;
}

/// Append one raw chunk to the accumulated output String, carrying any
/// incomplete trailing UTF-8 sequence across chunk boundaries (a `read`
/// may split a multi-byte character mid-sequence). Truly invalid bytes
/// become U+FFFD — the same lossy policy as `String::from_utf8_lossy`.
fn push_chunk(out: &mut String, carry: &mut Vec<u8>, chunk: &[u8]) {
    carry.extend_from_slice(chunk);
    loop {
        match std::str::from_utf8(carry) {
            Ok(s) => {
                out.push_str(s);
                carry.clear();
                return;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.push_str(&String::from_utf8_lossy(&carry[..valid]));
                match e.error_len() {
                    // Incomplete trailing sequence: it may complete with
                    // bytes from the NEXT chunk — keep it in the carry.
                    None => {
                        carry.drain(..valid);
                        return;
                    }
                    // Genuinely invalid bytes: replace and continue with
                    // the rest of the chunk.
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        carry.drain(..valid + bad);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)] // referenced only by unix-gated tests
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exit_codes_are_classified() {
        let ok = run(
            "sh",
            &["-c".into(), "exit 0".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(5),
        );
        assert!(ok.passed());
        let bad = run(
            "sh",
            &["-c".into(), "exit 7".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(5),
        );
        assert_eq!(bad.kind, CmdKind::NonZero);
        assert_eq!(bad.code, Some(7));
        let missing = run(
            "definitely-not-a-real-bin-xyz",
            &[],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(1),
        );
        assert_eq!(missing.kind, CmdKind::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn allowlist_env_strips_foreign_keys() {
        // SAFETY: single-threaded test section over a unique var name.
        std::env::set_var("AF_TEST_ALLOWLIST_VAR", "visible");
        let out = run(
            "sh",
            &["-c".into(), "echo ${AF_TEST_ALLOWLIST_VAR:-unset}".into()],
            None,
            &[],
            EnvMode::Allowlist(vec!["PATH".into()]),
            Duration::from_secs(5),
        );
        assert_eq!(out.stdout.trim(), "unset", "foreign key must be stripped");

        let out = run(
            "sh",
            &["-c".into(), "echo ${AF_TEST_ALLOWLIST_VAR:-unset}".into()],
            None,
            &[],
            EnvMode::Allowlist(vec!["PATH".into(), "AF_TEST_ALLOWLIST_VAR".into()]),
            Duration::from_secs(5),
        );
        assert_eq!(
            out.stdout.trim(),
            "visible",
            "allowlisted key passes through"
        );
        std::env::remove_var("AF_TEST_ALLOWLIST_VAR");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills() {
        let start = Instant::now();
        let out = run(
            "sh",
            &["-c".into(), "sleep 60".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(2),
        );
        assert_eq!(out.kind, CmdKind::Timeout);
        assert!(start.elapsed() < Duration::from_secs(10), "killed on time");
    }

    #[cfg(unix)]
    #[test]
    fn captures_stdout_and_stderr() {
        let out = run(
            "sh",
            &["-c".into(), "echo hi; echo err >&2".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(5),
        );
        assert!(out.stdout.contains("hi"));
        assert!(out.stderr.contains("err"));
    }

    #[cfg(unix)]
    #[test]
    fn stall_watchdog_kills_a_silent_child_long_before_the_timeout() {
        let start = Instant::now();
        let out = run_with_stall(
            "sh",
            &["-c".into(), "sleep 60".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(30), // total timeout — must NOT be reached
            Some(Duration::from_millis(300)),
        );
        assert_eq!(out.kind, CmdKind::Stalled, "a silent child is Stalled");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "killed at the ~300ms stall window, not the 30s timeout"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stall_window_does_not_disturb_a_talkative_child() {
        // Prints every 100ms — never idle for the 1s window — exits on its own.
        let out = run_with_stall(
            "sh",
            &[
                "-c".into(),
                "i=0; while [ $i -lt 5 ]; do echo tick; i=$((i+1)); sleep 0.1; done".into(),
            ],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(30),
            Some(Duration::from_secs(1)),
        );
        assert!(
            out.passed(),
            "healthy child completes: {:?}",
            out.combined()
        );
        assert_eq!(out.kind, CmdKind::Success);
        assert_eq!(
            out.stdout.matches("tick").count(),
            5,
            "full output captured"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stalled_child_keeps_the_output_it_wrote_before_going_silent() {
        let out = run_with_stall(
            "sh",
            &["-c".into(), "echo before-hang; sleep 60".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(30),
            Some(Duration::from_millis(400)),
        );
        assert_eq!(out.kind, CmdKind::Stalled);
        assert!(
            out.stdout.contains("before-hang"),
            "output before the stall is captured: {:?}",
            out.stdout
        );
    }

    #[cfg(unix)]
    #[test]
    fn stall_none_is_the_legacy_timeout_only_behaviour() {
        // A silent child with no stall window runs until the TOTAL timeout.
        let out = run_with_stall(
            "sh",
            &["-c".into(), "sleep 60".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_millis(300),
            None,
        );
        assert_eq!(
            out.kind,
            CmdKind::Timeout,
            "stall=None disables the watchdog"
        );
    }

    #[cfg(unix)]
    #[test]
    fn incremental_reads_capture_output_split_across_chunk_boundaries() {
        // A multi-byte char split by the fixed read buffer is reassembled;
        // a dangling incomplete sequence at EOF degrades to U+FFFD.
        let out = run(
            "sh",
            &["-c".into(), "printf 'ok\\303'".into()],
            None,
            &[],
            EnvMode::Inherit,
            Duration::from_secs(5),
        );
        assert!(out.passed());
        assert_eq!(
            out.stdout, "ok\u{FFFD}",
            "orphan lead byte is lossy, not lost"
        );
    }

    #[test]
    fn push_chunk_reassembles_split_multi_byte_characters() {
        let mut out = String::new();
        let mut carry = Vec::new();
        // 'A' + the first half of 'é' (0xC3 0xA9).
        push_chunk(&mut out, &mut carry, &[0x41, 0xC3]);
        assert_eq!(out, "A");
        assert_eq!(carry, vec![0xC3], "incomplete sequence carried");
        push_chunk(&mut out, &mut carry, &[0xA9]);
        assert_eq!(out, "Aé");
        assert!(carry.is_empty(), "carry drained once the char completes");
    }

    #[test]
    fn push_chunk_replaces_invalid_bytes_and_keeps_going() {
        let mut out = String::new();
        let mut carry = Vec::new();
        // 0xC3 0x28 is not a valid sequence ('(' is not a continuation byte);
        // the rest of the chunk ('y') must still come through.
        push_chunk(&mut out, &mut carry, &[0xC3, 0x28, b'y']);
        assert_eq!(out, "\u{FFFD}(y");
        assert!(carry.is_empty());
    }
}
