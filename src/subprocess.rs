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

pub fn run(
    cmd: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
    timeout: Duration,
) -> CmdOut {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    for (k, v) in env {
        c.env(k, v);
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
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exit_codes_are_classified() {
        let ok = run("sh", &["-c".into(), "exit 0".into()], None, &[], Duration::from_secs(5));
        assert!(ok.passed());
        let bad = run("sh", &["-c".into(), "exit 7".into()], None, &[], Duration::from_secs(5));
        assert_eq!(bad.kind, CmdKind::NonZero);
        assert_eq!(bad.code, Some(7));
        let missing = run("definitely-not-a-real-bin-xyz", &[], None, &[], Duration::from_secs(1));
        assert_eq!(missing.kind, CmdKind::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills() {
        let start = Instant::now();
        let out = run(
            "sh",
            &["-c".into(), "sleep 30".into()],
            None,
            &[],
            Duration::from_millis(300),
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
            Duration::from_secs(5),
        );
        assert!(out.stdout.contains("hi"));
        assert!(out.stderr.contains("err"));
    }
}
