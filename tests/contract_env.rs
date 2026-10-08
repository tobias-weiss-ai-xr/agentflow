//! Contract tests pinning the agent child-process environment as a real
//! SECURITY boundary (sandbox spec: "Agent environment allowlist" and
//! "Git hygiene for agent children").
//!
//! Until now the boundary was only covered indirectly: e2e drives it
//! through the whole run loop with a fake agent, and unit tests assert on
//! the allow VECTOR — neither proves what a real child process actually
//! observes. These tests spawn real children through the exact path
//! production takes (`execute::agent_env` → `subprocess::run` with
//! `EnvMode::Allowlist`), have them print the environment they woke up
//! with (`sh -c env`, the platform shell the module's own tests use; a
//! direct `env` exec for the exact-set proof), and assert on what the
//! children truly saw — not on what the policy claims.
//!
//! # Race avoidance for the env mutations
//!
//! `std::env::set_var` mutates PROCESS-global state while cargo's test
//! harness runs tests in parallel threads. Four rules make these tests
//! race-free (this is the documentation the canary assertion requires):
//!
//! 1. cargo runs each integration-test FILE as its own process, so
//!    mutations here can never reach another test binary (e2e.rs, cli.rs,
//!    … are separate processes with separate environments);
//! 2. inside this binary both tests serialize on `env_lock()` — a static
//!    mutex held for the whole test body — so no sibling test in this
//!    binary spawns a child or inspects the env while a mutation is live;
//! 3. every mutated name is either unique to this file (`AF_CONTRACT_*`)
//!    or saved-and-restored by `EnvRestore` (the foreign provider key, the
//!    git identity vars), so the ambient environment is left byte-identical
//!    on exit — even when an assertion panics mid-test;
//! 4. all assertions read the CHILD's snapshot (captured stdout), never
//!    the parent env, so a concurrent parent-env reader elsewhere cannot
//!    flake these assertions.
//!
//! Unix-only, like the subprocess module's own tests (CI is Linux; the
//! `env`/`sh` binaries are the unix platform tools).

#![cfg(unix)]

use agentflow::config::Worker;
use agentflow::execute::agent_env;
use agentflow::subprocess::{run, CmdKind, EnvMode};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// Serializes env-mutating test bodies within THIS binary (module docs,
/// race-avoidance rule 2).
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Saves an env var's current value and restores it (present OR absent) on
/// drop, so the ambient environment survives the test byte-identical even
/// when an assertion fails mid-test (module docs, rule 3).
struct EnvRestore {
    key: String,
    prior: Option<String>,
}

impl EnvRestore {
    fn set(key: &str, value: &str) -> Self {
        // Racy only if another thread touched this key concurrently; the
        // name is unique to this file (or restored below) and this test
        // holds env_lock() — see the module docs.
        let prior = std::env::var(key).ok();
        std::env::set_var(key, value);
        EnvRestore {
            key: key.to_string(),
            prior,
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.prior.take() {
            Some(v) => std::env::set_var(&self.key, v),
            None => std::env::remove_var(&self.key),
        }
    }
}

/// A unique temp dir per test (never shared; cleaned up best-effort).
fn temp_dir(tag: &str) -> PathBuf {
    let nano = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "af-contract-env-{}-{tag}-{nano}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Parse `env` output (KEY=VALUE lines) into pairs. Lines without '=' are
/// ignored (never expected, but never silently fatal either).
fn parse_env_dump(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|l| {
            l.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// The exact key set a child may show: every allowlisted name that is
/// actually present in the parent (`EnvMode::Allowlist` relays present
/// keys only — both sides use `std::env::var`, so they agree by
/// construction), plus the hygiene pairs `agent_env` force-sets.
fn expected_keys(allow: &[String], pairs: &[(String, String)]) -> HashSet<String> {
    let mut keys: HashSet<String> = allow
        .iter()
        .filter(|k| std::env::var(k).is_ok())
        .cloned()
        .collect();
    keys.extend(pairs.iter().map(|(k, _)| k.clone()));
    keys
}

fn worker_with_key_env(key_env: Option<&str>) -> Worker {
    Worker {
        name: "zai".into(),
        provider: "zai".into(),
        model: "glm-5.2".into(),
        api_key_env: key_env.map(str::to_string),
        output: "text".into(),
        args: Vec::new(),
        params_b: None,
        price_per_mtok_usd: None,
        ..Default::default()
    }
}

/// The environment contract (sandbox layer 1): an agent child spawned with
/// the production policy sees ONLY its own worker's provider key plus the
/// system/git-identity base and the git hygiene pairs — nothing else from
/// the orchestrator environment — and a non-zero child exit is captured as
/// a classified result, never raised.
// spec: sandbox/agent-environment-allowlist
// spec: sandbox/git-hygiene-for-agent-children
#[test]
fn subprocess_env_is_allowlisted() {
    let _lock = env_lock();

    // --- Arrange an ambient environment full of things that must NOT leak.
    // Every mutation goes through EnvRestore (module docs, rule 3).

    // (1) Sentinel canary: proof the child is not simply inheriting the
    //     parent env (env_clear, not overlay). It must be set in this
    //     process because EnvMode::Allowlist reads the REAL parent env;
    //     the mutation is race-free per the module docs.
    let _canary = EnvRestore::set("AF_CONTRACT_CANARY", "leak");
    // (2) A DIFFERENT provider's key, as really found in orchestrator envs
    //     on dev machines: must NOT reach a worker that does not own it.
    //     Saved/restored in case the host has a real one.
    let _foreign = EnvRestore::set("ANTHROPIC_API_KEY", "sk-foreign-must-not-leak");
    // (2) The dispatched worker's own key: MUST reach the child, intact.
    let _own = EnvRestore::set("AF_CONTRACT_WORKER_API_KEY", "sk-own-worker-key-1");
    // (3) Commit identity: agent_env does NOT fix these values — its
    //     AGENT_ENV_BASE relays the parent's GIT_AUTHOR_*/GIT_COMMITTER_*
    //     when present — so set deterministic ones to pin the pass-through.
    let _an = EnvRestore::set("GIT_AUTHOR_NAME", "af-contract-test");
    let _ae = EnvRestore::set("GIT_AUTHOR_EMAIL", "af-contract@example.invalid");
    let _cn = EnvRestore::set("GIT_COMMITTER_NAME", "af-contract-test");
    let _ce = EnvRestore::set("GIT_COMMITTER_EMAIL", "af-contract@example.invalid");

    // Passthrough lookup pinned "unset" so the BASE contract is asserted
    // regardless of the host's TF_AGENT_ENV_PASSTHROUGH (that escape hatch
    // is covered by the e2e suite).
    let (pairs_static, allow) = agent_env(
        &worker_with_key_env(Some("AF_CONTRACT_WORKER_API_KEY")),
        &|_| None,
    );
    // Convert to owned for subprocess compatibility
    let pairs: Vec<(String, String)> = pairs_static
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let dir = temp_dir("allowlisted");

    // --- One shell spawn pins the exit contract alongside the env dump:
    // dump the environment as KEY=VALUE lines, write to stderr (to pin
    // combined()), then exit 7.
    let out = run(
        "sh",
        &["-c".into(), "env; echo noise-on-stderr >&2; exit 7".into()],
        Some(&dir),
        &pairs,
        EnvMode::Allowlist(allow.clone()),
        Duration::from_secs(30),
    );

    // (4) Exit contract: a non-zero child is CAPTURED as data, not raised —
    // run() returned instead of panicking, kind/code are exact, and the
    // output is still intact.
    assert!(!out.passed(), "a non-zero exit must never count as passed");
    assert_eq!(out.kind, CmdKind::NonZero);
    assert_eq!(out.code, Some(7));
    assert!(
        out.stdout.contains("PATH="),
        "env dump captured despite the non-zero exit: {}",
        out.combined()
    );
    // combined() = stdout then stderr when stderr is non-empty (this is
    // what execute.rs/gate.rs log).
    let combined = out.combined();
    let dump_at = combined.find("PATH=").expect("combined() carries stdout");
    let noise_at = combined
        .find("noise-on-stderr")
        .expect("combined() carries stderr");
    assert!(
        dump_at < noise_at,
        "combined() is stdout first, then stderr"
    );

    let seen = parse_env_dump(&out.stdout);

    // (1) The canary does not leak — the child got a FRESH environment,
    // not a copy of the parent's.
    assert!(
        seen.iter().all(|(k, _)| k != "AF_CONTRACT_CANARY"),
        "canary leaked: the child inherited the parent env instead of an \
         allowlisted one"
    );

    // (2) The worker's own key is present with its value intact, and a
    // different provider's key is withheld.
    assert!(
        seen.contains(&(
            "AF_CONTRACT_WORKER_API_KEY".to_string(),
            "sk-own-worker-key-1".to_string()
        )),
        "the worker's own provider key must be relayed with its value intact"
    );
    assert!(
        seen.iter().all(|(k, _)| k != "ANTHROPIC_API_KEY"),
        "a DIFFERENT provider's key leaked into this worker's child env"
    );

    // (3) Git hygiene, exactly as agent_env sets it: GIT_TERMINAL_PROMPT=0
    // kills credential prompts; the GIT_CONFIG_COUNT/KEY_0/VALUE_0 triple
    // blanks credential.helper so stored helpers cannot be used.
    assert!(seen.contains(&("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())));
    assert!(seen.contains(&("GIT_CONFIG_COUNT".to_string(), "1".to_string())));
    assert!(seen.contains(&(
        "GIT_CONFIG_KEY_0".to_string(),
        "credential.helper".to_string()
    )));
    assert!(seen.contains(&("GIT_CONFIG_VALUE_0".to_string(), String::new())));
    // (3b) Commit identity is relayed from the parent (not invented).
    assert!(seen.contains(&(
        "GIT_AUTHOR_NAME".to_string(),
        "af-contract-test".to_string()
    )));
    assert!(seen.contains(&(
        "GIT_AUTHOR_EMAIL".to_string(),
        "af-contract@example.invalid".to_string()
    )));
    assert!(seen.contains(&(
        "GIT_COMMITTER_NAME".to_string(),
        "af-contract-test".to_string()
    )));
    assert!(seen.contains(&(
        "GIT_COMMITTER_EMAIL".to_string(),
        "af-contract@example.invalid".to_string()
    )));

    // --- Bounded env: NOTHING outside the policy is visible. This proof
    // execs `env` DIRECTLY (no shell) because /bin/sh injects its own vars
    // (PWD, SHLVL, _) into children — shell artifacts that would blur the
    // exact-set assertion. A direct env(1) prints exactly the environ the
    // Command built, so set equality holds in BOTH directions.
    let direct = run(
        "env",
        &[],
        Some(&dir),
        &pairs,
        EnvMode::Allowlist(allow.clone()),
        Duration::from_secs(30),
    );
    assert!(direct.passed(), "direct env dump: {}", direct.combined());
    let direct_seen: HashSet<String> = parse_env_dump(&direct.stdout)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        direct_seen,
        expected_keys(&allow, &pairs),
        "the child env must be EXACTLY the allowlisted keys present in the \
         parent plus the git hygiene pairs — nothing more, nothing less"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The empty-key contract: a worker with NO `api_key_env` must not smuggle
/// ANY credential into its child — no key slot in the allowlist, and no
/// key-shaped variable in what a real child actually observes.
// spec: sandbox/agent-environment-allowlist
#[test]
fn subprocess_env_without_worker_key_smuggles_nothing() {
    let _lock = env_lock();

    // Plausible provider keys really present in orchestrator envs — a
    // key-less worker must not inherit any of them through the base
    // allowlist. Saved/restored in case the host has real ones.
    let _f1 = EnvRestore::set("OPENAI_API_KEY", "sk-openai-must-not-smuggle");
    let _f2 = EnvRestore::set("ANTHROPIC_API_KEY", "sk-ant-must-not-smuggle");
    let _f3 = EnvRestore::set("GEMINI_API_KEY", "sk-gem-must-not-smuggle");
    // And a plain non-key secret, as a second canary.
    let _canary = EnvRestore::set("AF_CONTRACT_CANARY", "leak");

    let (pairs_static, allow) = agent_env(&worker_with_key_env(None), &|_| None);
    // Convert to owned for subprocess compatibility
    let pairs: Vec<(String, String)> = pairs_static
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    // The policy itself has no key slot when the worker declares none.
    assert!(
        allow.iter().all(|k| !k.contains("API_KEY")),
        "no api_key_env => the allowlist must contain no key variable: {allow:?}"
    );

    let dir = temp_dir("no-key");
    // Direct env(1) exec: exactly the environ the Command built (no shell
    // noise), so the bounded-set proof below is exact.
    let out = run(
        "env",
        &[],
        Some(&dir),
        &pairs,
        EnvMode::Allowlist(allow.clone()),
        Duration::from_secs(30),
    );
    assert!(out.passed(), "env dump must succeed: {}", out.combined());

    let seen = parse_env_dump(&out.stdout);

    // No credential-shaped variable at all. (GIT_CONFIG_KEY_0 is git
    // config plumbing — it NAMES the "credential.helper" setting — not a
    // credential, and contains no "API_KEY".)
    assert!(
        seen.iter().all(|(k, _)| {
            !k.contains("API_KEY") && !k.contains("SECRET") && !k.contains("TOKEN")
        }),
        "credential-shaped variable smuggled into a key-less worker's child: {seen:?}"
    );
    // Explicitly: the usual provider key names.
    for name in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "ZAI_API_KEY",
        "OPENROUTER_API_KEY",
        "DEEPSEEK_API_KEY",
        "GROQ_API_KEY",
        "XAI_API_KEY",
        "MISTRAL_API_KEY",
    ] {
        assert!(
            seen.iter().all(|(k, _)| k != name),
            "{name} leaked into a key-less worker's child env"
        );
    }
    // The canary is withheld here too.
    assert!(
        seen.iter().all(|(k, _)| k != "AF_CONTRACT_CANARY"),
        "canary leaked into a key-less worker's child env"
    );
    // Git hygiene applies to key-less workers as well (a key-less agent
    // still commits, so it still gets the non-interactive git contract).
    assert!(seen.contains(&("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())));
    assert!(seen.contains(&(
        "GIT_CONFIG_KEY_0".to_string(),
        "credential.helper".to_string()
    )));
    // Bounded exactly like the main contract: system base present in the
    // parent, plus hygiene pairs, and NOTHING else.
    let seen_keys: HashSet<String> = seen.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(
        seen_keys,
        expected_keys(&allow, &pairs),
        "a key-less worker's child env must be EXACTLY the base allowlist \
         plus git hygiene — nothing smuggled in"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
