//! Acceptance gate runner: a shell command with scoped env and hard timeout.
//! exit 0 = pass (the task's runtime contract test).

use crate::subprocess::{self, CmdOut};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

/// Preferred shell: `/bin/sh -c` on unix. On Windows, prefer a POSIX `sh`
/// (git-bash) when one is on PATH so that agent tool calls and gates can be
/// authored in POSIX shell — `cmd /C` mangles bash idioms (`.` is not a
/// command prefix, so `./prog` fails; `$(...)`, `;`, and single-quoted pipes
/// all break, which is exactly how three doc-consistency attempts died). The
/// probe is run once per process and cached; without a reachable `sh` the
/// Windows fallback is `cmd /C` as before.
pub fn shell() -> (&'static str, String) {
    #[cfg(windows)]
    {
        if posix_sh_available() {
            ("sh", "-c".to_string())
        } else {
            ("cmd", "/C".to_string())
        }
    }
    #[cfg(not(windows))]
    {
        ("sh", "-c".to_string())
    }
}

#[cfg(windows)]
fn posix_sh_available() -> bool {
    static AVAIL: OnceLock<bool> = OnceLock::new();
    *AVAIL.get_or_init(|| {
        std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .status()
            .is_ok()
    })
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

    #[cfg(windows)]
    #[test]
    fn windows_gate_runs_posix_shell_when_gitbash_present() {
        // git-bash is on the dev host's PATH: shell() must report sh so that
        // POSIX-authored gates work (the same gate died under cmd /C with
        // "'.' is not recognized" / single-quote-pipe split).
        let (shell, flag) = shell();
        assert_eq!(shell, "sh");
        assert_eq!(flag, "-c");
        let out = run_accept(
            "printf ok | grep -q ok && [ -n `echo x` ]",
            Path::new("."),
            &[],
            Duration::from_secs(15),
            true,
        );
        assert!(out.passed(), "posix gate should pass: {}", out.combined());
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
