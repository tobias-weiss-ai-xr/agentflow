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
//! - `FAKE_AGENT_ENV` / `FAKE_AGENT_ENV_NAMES`: sandbox env probe (see below)
//! - `FAKE_AGENT_TOUCH_FROM_MODEL`: when set (and `FAKE_AGENT_TOUCH` unset),
//!   write `{model}.txt` instead — lets two workers with distinct `--model`s
//!   produce distinct merged artifacts in a parallel-dispatch test.
//! - `FAKE_AGENT_SLEEP_MS`: optional fixed delay before doing work, so E2E
//!   tests can hold a task in the `running` state long enough to observe that
//!   several tasks are genuinely in flight at the same instant.
//! - `FAKE_AGENT_HANG_MS`: optional delay BEFORE any output at all (capped
//!   like `FAKE_AGENT_SLEEP_MS`), so a test can present a genuinely stalled
//!   agent — silent on stdout/stderr far longer than a stall window.
//!
//! Like a real agent, it commits its work to the current branch so the
//! orchestrator's merge actually carries the changes to the base branch.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Optional `--model` value (worker selection): lets a test give each
    // worker its own output file so two merged tasks leave two artifacts.
    let model = args
        .windows(2)
        .find(|w| w[0] == "--model")
        .and_then(|w| w.get(1))
        .cloned();
    let exit: i32 = std::env::var("FAKE_AGENT_EXIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Output file: explicit `FAKE_AGENT_TOUCH` wins, else (opt-in) the model
    // name, else the historical default.
    let touch = std::env::var("FAKE_AGENT_TOUCH")
        .ok()
        .or_else(|| {
            if std::env::var("FAKE_AGENT_TOUCH_FROM_MODEL").is_ok() {
                model.map(|m| format!("{m}.txt"))
            } else {
                None
            }
        })
        .unwrap_or_else(|| "DONE.txt".to_string());
    let out = std::env::var("FAKE_AGENT_OUT")
        .unwrap_or_else(|_| "example-agent: task complete".to_string());

    if exit != 0 {
        eprintln!("example_agent: failing with exit {exit}");
        return ExitCode::from(exit.clamp(0, 255) as u8);
    }
    // Watchdog-test knob: hang BEFORE writing any output — the stub stays
    // silent on stdout/stderr (an "agent stopped producing output" stall),
    // bounded exactly like FAKE_AGENT_SLEEP_MS so no test can ask for an
    // unbounded sleep.
    if let Some(ms) = std::env::var("FAKE_AGENT_HANG_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(2000)));
    }
    // Optional fixed delay so a test can observe several tasks running at
    // once (parallel-dispatch E2E proof). Bounded and deterministically short.
    if let Some(ms) = std::env::var("FAKE_AGENT_SLEEP_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms.min(2000)));
    }
    // Env probe (sandbox tests): dump `FAKE_AGENT_ENV_NAMES` to `FAKE_AGENT_ENV`
    // as `NAME=value` / `NAME=<unset>` lines so tests can assert what the agent
    // child actually received.
    if let Ok(probe) = std::env::var("FAKE_AGENT_ENV") {
        let names = std::env::var("FAKE_AGENT_ENV_NAMES").unwrap_or_default();
        let mut s = String::new();
        for n in names.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match std::env::var(n) {
                Ok(v) => s.push_str(&format!("{n}={v}\n")),
                Err(_) => s.push_str(&format!("{n}=<unset>\n")),
            }
        }
        let _ = std::fs::write(&probe, s);
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
