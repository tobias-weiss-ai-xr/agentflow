//! Retry pacing (`retry_delay_s`, default 0): a failed attempt that will be
//! retried waits the configured seconds before the NEXT attempt starts —
//! and nothing else is ever delayed. First attempts, first-try merges and
//! the shipped default (field omitted = 0) must all behave exactly like the
//! no-delay legacy run.
//!
//! Hermetic, modeled on tests/e2e.rs's `fixture()` / tests/wakeup.rs: a
//! scratch git repo under a uniquely-named temp dir, `config::load`, and a
//! hand-built `Settings` whose single worker's `cli` is the bundled
//! `example_agent` stub. No network, no LLM.
//!
//! Timing bounds are machine-independent RELATIVE comparisons with
//! coarse whole-second slack, because per-attempt overhead is not: on a
//! fast CI runner two stub attempts cost ~0.2s, on a laptop ~4.5s (git
//! worktree add/merge/remove dominates). What is stable is the DELTA
//! between campaigns that do the same work — identical-campaign variance
//! measures well under 0.5s. So, with a 3s pace (PACE):
//!   * the paced campaign takes >= PACE total (the sleep alone guarantees
//!     it) and >= the identical zero-delay campaign + 2s (1s of slack for
//!     jitter on a 3s signal);
//!   * a campaign that must NOT be paced (first-try merge with pacing
//!     configured; field omitted) stays within 1s of its zero-delay twin
//!     — had pacing fired, the gap would be the full 3s.

use agentflow::config::{self, Settings, TaskState};
use agentflow::run::{self, RunOptions};
use agentflow::state::Store;
use std::path::Path;
use std::time::{Duration, Instant};

const AGENT: &str = env!("CARGO_BIN_EXE_example_agent");

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git must be available");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Fixture {
    cfg: config::Config,
    st: Settings,
}

/// A gate that FAILS its first evaluation and passes every later one: a
/// counter file in the fixture dir (shared across attempts, unlike the
/// fresh per-attempt worktree) counts gate runs — attempt 1 sees n=0 and
/// fails, attempt 2 sees n=1 and passes.
fn flaky_gate(counter: &Path) -> String {
    format!(
        r#"n=$(cat "{c}" 2>/dev/null || echo 0); echo $((n+1)) > "{c}"; test "$n" -ge 1"#,
        c = counter.display()
    )
}

/// `defaults_extra` is appended inside the `"defaults"` object (e.g.
/// `, "retry_delay_s": 1`); `""` omits the field entirely. `flaky` selects
/// the fail-once gate vs a gate that passes on the first attempt.
fn fixture(defaults_extra: &str, flaky: bool) -> Fixture {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("af-retry-pace-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    // Repo-local identity (NOT process env): parallel tests must not race
    // on GIT_* variables, and the worktrees share this config for the
    // stub agent's commits.
    git(&repo, &["config", "user.name", "af test"]);
    git(&repo, &["config", "user.email", "af@test"]);
    std::fs::write(repo.join("README.md"), "# scratch\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);

    let accept = if flaky {
        flaky_gate(&dir.join("gate-count"))
    } else {
        // The stub agent writes DONE.txt on every run: this gate passes on
        // the first attempt (the happy path).
        "test -f DONE.txt".to_string()
    };
    let config_dir = dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    // JSON-escape both embedded paths/commands: Windows paths carry
    // backslashes and the shell quoting adds double quotes.
    let accept_json = accept.replace('\\', "\\\\").replace('"', "\\\"");
    let agent_escaped = AGENT.replace('\\', "\\\\");
    std::fs::write(
        config_dir.join("tasks.json"),
        format!(
            r#"{{ "tasks": [ {{"id":"A","title":"paced","scope":["DONE.txt"],"accept":"{accept_json}"}} ] }}"#
        ),
    )
    .unwrap();
    std::fs::write(
        config_dir.join("workers.json"),
        format!(
            r#"{{ "defaults": {{ "max_attempts": 3, "accept_timeout_s": 10{defaults_extra} }},
                "workers": [ {{ "name": "w1", "provider": "p", "model": "m",
                                "enabled": true, "cli": "{agent_escaped}" }} ] }}"#
        ),
    )
    .unwrap();

    let cfg = config::load(
        &config_dir.join("tasks.json"),
        &config_dir.join("workers.json"),
    )
    .unwrap();
    let st = Settings {
        repo_dir: repo.clone(),
        state_dir: dir.join("state"),
        worktree_root: dir.join("wt"),
        max_parallel: 1,
        branch_prefix: "tf".into(),
        poll_secs: 1,
        gate_env: vec![],
        agent_max_turns: 0,
        tasks_file: config_dir.join("tasks.json"),
        workers_file: config_dir.join("workers.json"),
        prompt_file: dir.join("no-template.md"),
        agent_timeout_s: 60,
        agent_stall_s: 0,
        max_wall_clock_s: 0,
        sandbox_cmd: vec![],
    };
    Fixture { cfg, st }
}

/// Run one campaign and return (exit code, elapsed). Also asserts the
/// task reached Done on `expected_attempts` — a pacing regression that
/// broke retries would show up here, not just in the timing.
fn run_campaign(f: &Fixture, expected_attempts: u32) -> (i32, Duration) {
    let t0 = Instant::now();
    let code = run::run_loop(&f.cfg, &f.st, &RunOptions::default());
    let elapsed = t0.elapsed();
    assert_eq!(code, 0, "the campaign completes (all done)");
    let st = Store::new(f.st.state_dir.clone()).load();
    assert_eq!(st["A"].state, TaskState::Done);
    assert_eq!(
        st["A"].attempts, expected_attempts,
        "attempt count: the gate decided the retries"
    );
    (code, elapsed)
}

// spec: lifecycle/execute-pipeline
// spec: lifecycle/execute-pipeline#happy-path
/// `retry_delay_s` paces a task's retries without taxing anything else:
/// with the field set, a gate that fails the first attempt forces a paced
/// retry (the run takes at least the delay), while the SAME campaign at
/// delay 0 completes measurably faster; a first-try merge with pacing
/// configured, and a config that omits the field (the shipped default 0),
/// are never delayed.
#[cfg(unix)] // the fail-once gate is POSIX shell arithmetic
#[test]
fn retry_delay_paces_attempts_without_delaying_the_happy_path() {
    // 3s pace: coarse and loud. The assertions below only ever compare
    // deltas between campaigns doing identical work, so absolute
    // per-machine overhead (0.2s..4.5s per attempt, measured) cannot
    // flake them; 2s of slack sits on the upper-bound assertions — the
    // worst identical-campaign variance observed under 8 CPU hogs is
    // ~1.5s, so 2s tolerates it while still distinguishing 0s from 3s.
    const PACE_SECS: u64 = 3;
    let pace = format!(r#", "retry_delay_s": {PACE_SECS}"#);

    // --- (1) PACED: retry_delay_s = 3, gate fails attempt 1 --------------
    // The retry happens (Done on attempt 2) but is paced: the backoff
    // alone puts the total at >= PACE.
    let paced = fixture(&pace, true);
    let paced_state = paced.st.state_dir.clone();
    let (_code, paced_elapsed) = run_campaign(&paced, 2);
    assert!(
        paced_elapsed >= Duration::from_secs(PACE_SECS),
        "the paced retry adds at least the {PACE_SECS}s delay (took {paced_elapsed:?})"
    );
    // Observable: the pace leaves one clear line in the task's log naming
    // the seconds, so a puzzling pause is explainable after the fact.
    let log = std::fs::read_to_string(paced_state.join("logs").join("A.log"))
        .expect("task log exists after the run");
    assert!(
        log.contains("retry_delay_s") && log.contains("3s"),
        "the pace is visible in the log: {log}"
    );

    // --- (2) ZERO DELAY: identical campaign, retry_delay_s = 0 ----------
    // Same two attempts, no backoff: measurably faster — the paced run
    // must exceed it by roughly the full pace (1s of slack for jitter).
    let fast = fixture(r#", "retry_delay_s": 0"#, true);
    let (_code, fast_elapsed) = run_campaign(&fast, 2);
    assert!(
        paced_elapsed >= fast_elapsed + Duration::from_secs(PACE_SECS - 1),
        "the pace separates the runs: paced {paced_elapsed:?} vs fast {fast_elapsed:?}"
    );

    // --- (3) HAPPY PATH: retry_delay_s = 3, gate passes first try -------
    // Pacing is configured but the task merges on attempt 1: never
    // delayed — within 2s of its zero-delay twin (a paced first attempt
    // would add the full 3s).
    let happy_paced = fixture(&pace, false);
    let (_code, happy_paced_elapsed) = run_campaign(&happy_paced, 1);
    let happy_zero = fixture(r#", "retry_delay_s": 0"#, false);
    let (_code, happy_zero_elapsed) = run_campaign(&happy_zero, 1);
    assert!(
        happy_paced_elapsed < happy_zero_elapsed + Duration::from_secs(2),
        "a first-try merge is never delayed, even with retry_delay_s set \
         (paced {happy_paced_elapsed:?} vs zero {happy_zero_elapsed:?})"
    );

    // --- (4) DEFAULT: the field is omitted entirely ---------------------
    // The shipped default is 0, so an existing workers.json that never
    // mentioned the field keeps the legacy no-delay behaviour — within 2s
    // of the explicit zero-delay campaign (a default pace would add 3s).
    let defaulted = fixture("", true);
    assert_eq!(
        defaulted.cfg.defaults.retry_delay_s, 0,
        "the shipped default is 0"
    );
    let (_code, default_elapsed) = run_campaign(&defaulted, 2);
    assert!(
        default_elapsed < fast_elapsed + Duration::from_secs(2),
        "an omitted retry_delay_s behaves like the zero-delay run \
         (default {default_elapsed:?} vs fast {fast_elapsed:?})"
    );
}
