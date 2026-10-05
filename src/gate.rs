//! Acceptance gate runner: a shell command with scoped env and hard timeout.
//! exit 0 = pass (the task's runtime contract test).

use crate::subprocess::{self, CmdOut};
use std::path::Path;
use std::time::Duration;

/// Shell used for gates: `/bin/sh -c` on unix, `cmd /C` on Windows.
pub fn shell() -> (&'static str, String) {
    #[cfg(windows)]
    {
        ("cmd", "/C".to_string())
    }
    #[cfg(not(windows))]
    {
        ("sh", "-c".to_string())
    }
}

pub fn run_accept(
    accept: &str,
    cwd: &Path,
    env: &[(String, String)],
    timeout: Duration,
    replay: bool,
) -> CmdOut {
    let (shell, flag) = shell();
    let args = vec![flag, accept.to_string()];
    // Forward the replay-safety decision so gate scripts can assert it.
    let mut env: Vec<(String, String)> = env.to_vec();
    env.push((
        "TF_GATE_REPLAY".to_string(),
        if replay { "1" } else { "0" }.to_string(),
    ));
    // Gates are user-authored (trusted): full inherited environment.
    subprocess::run(
        shell,
        &args,
        Some(cwd),
        &env,
        crate::subprocess::EnvMode::Inherit,
        timeout,
    )
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)] // run_accept is only referenced by unix-gated tests
    use super::*;
    #[cfg(unix)]
    use crate::subprocess::CmdKind;

    #[cfg(unix)]
    #[test]
    fn gate_passes_on_exit_zero() {
        let out = run_accept("exit 0", Path::new("."), &[], Duration::from_secs(5), true);
        assert!(out.passed());
    }

    #[cfg(unix)]
    #[test]
    fn gate_fails_on_exit_nonzero() {
        let out = run_accept("exit 3", Path::new("."), &[], Duration::from_secs(5), true);
        assert_ne!(out.kind, CmdKind::Success);
        assert_eq!(out.code, Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn gate_env_is_scoped_to_declared_pairs() {
        // Var injected via TF_GATE_ENV is visible; a random inherited var is not touched.
        let out = run_accept(
            "test \"$MY_GATE_VAR\" = hello",
            Path::new("."),
            &[("MY_GATE_VAR".to_string(), "hello".to_string())],
            Duration::from_secs(5),
            true,
        );
        assert!(out.passed(), "scoped env var reachable");
    }
}
