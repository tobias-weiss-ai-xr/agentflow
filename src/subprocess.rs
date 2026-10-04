//! Subprocess helper: the single, uniform way `af` runs git, the agent CLI,
//! and acceptance gates (arc42 ch. 8.3). Captures output without pipe
//! deadlock, applies a hard timeout, kills on timeout, and classifies the
//! result by exit-code contract.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdKind {
    Success,
    NonZero,
    Killed,
    Timeout,
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

pub fn run(
    cmd: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
    env_mode: EnvMode,
    timeout: Duration,
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
        Err(_) => {
            return CmdOut {
                kind: CmdKind::Missing,
                code: None,
                stdout: String::new(),
                stderr: String::new(),
            }
        }
    };

    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let r1 = thread::spawn(move || {
        let mut s = String::new();
        let _ = so.read_to_string(&mut s);
        s
    });
    let r2 = thread::spawn(move || {
        let mut s = String::new();
        let _ = se.read_to_string(&mut s);
        s
    });

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
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    if status.is_some() && kind != CmdKind::Timeout {
        let code = status.as_ref().and_then(|s| s.code());
        kind = match code {
            Some(0) => CmdKind::Success,
            Some(_) => CmdKind::NonZero,
            None => CmdKind::Killed, // terminated by signal
        };
        let _ = 0; // keep `code` extracted below too
    }

    let stdout = r1.join().unwrap_or_default();
    let stderr = r2.join().unwrap_or_default();
    let code = status.as_ref().and_then(|s| s.code());

    CmdOut {
        kind,
        code,
        stdout,
        stderr,
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)] // referenced only by unix-gated tests
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exit_codes_are_classified() {
        let ok = run("sh", &["-c".into(), "exit 0".into()], None, &[], EnvMode::Inherit, Duration::from_secs(5));
        assert!(ok.passed());
        let bad = run("sh", &["-c".into(), "exit 7".into()], None, &[], EnvMode::Inherit, Duration::from_secs(5));
        assert_eq!(bad.kind, CmdKind::NonZero);
        assert_eq!(bad.code, Some(7));
        let missing = run("definitely-not-a-real-bin-xyz", &[], None, &[], EnvMode::Inherit, Duration::from_secs(1));
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
        assert_eq!(out.stdout.trim(), "visible", "allowlisted key passes through");
        let _ = std::env::remove_var("AF_TEST_ALLOWLIST_VAR");
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
}
