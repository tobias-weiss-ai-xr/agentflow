//! example_agent — a stub OpenAI-compatible agent CLI.
//!
//! Used by agentflow's own test suite (and by CI) as a deterministic stand-in
//! for `pi`/opencode/etc. Accepts the same argument shape as the real thing
//! (`--provider X --model Y -p @file`), ignores it, and behaves according to
//! a few environment variables:
//!
//! - `FAKE_AGENT_EXIT`: exit code (default 0)
//! - `FAKE_AGENT_TOUCH`: file to write into the current directory (default `DONE.txt`)
//! - `FAKE_AGENT_OUT`:   content to write (default a summary line)
//!
//! Like a real agent, it commits its work to the current branch so the
//! orchestrator's merge actually carries the changes to the base branch.

use std::process::ExitCode;

fn main() -> ExitCode {
    let _ = std::env::args().skip(1).collect::<Vec<_>>(); // accept any args
    let exit: i32 = std::env::var("FAKE_AGENT_EXIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let touch = std::env::var("FAKE_AGENT_TOUCH").unwrap_or_else(|_| "DONE.txt".to_string());
    let out = std::env::var("FAKE_AGENT_OUT")
        .unwrap_or_else(|_| "example-agent: task complete".to_string());

    if exit != 0 {
        eprintln!("example_agent: failing with exit {exit}");
        return ExitCode::from(exit.clamp(0, 255) as u8);
    }
    let _ = std::fs::write(&touch, format!("{out}\n"));
    println!("{out}");

    // Commit the work so merges carry it (ignore commit failures — the
    // acceptance gate is the real check).
    let _ = std::process::Command::new("git")
        .args(["add", "-A"])
        .status();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "example_agent: task work"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    ExitCode::SUCCESS
}
