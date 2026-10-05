//! `gate_replay` flag: defaults to `true` when omitted, parses `false` when
//! declared — making acceptance-gate idempotency an explicit contract.

use std::path::PathBuf;

const WORKERS: &str = r#"{ "defaults": { "max_attempts": 1, "accept_timeout_s": 10 },
    "workers": [ { "name": "w1", "provider": "openai", "model": "gpt-4o", "enabled": true, "cli": "unused" } ] }"#;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("af-gate-replay-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, content).unwrap();
    p
}

// spec: lifecycle/gate-replay-contract
#[test]
fn gate_replay_defaults_to_true_and_parses() {
    // 1) Explicit `false` parses as false.
    let d = tmpdir("explicit-false");
    let tasks = write(
        &d,
        "tasks.json",
        r#"{ "tasks": [ { "id": "A", "title": "t", "accept": "true", "gate_replay": false } ] }"#,
    );
    let workers = write(&d, "workers.json", WORKERS);
    let cfg = agentflow::config::load(&tasks, &workers).unwrap();
    assert!(!cfg.by_id["A"].gate_replay);

    // 2) Omitting the field parses as true (opt-out flag).
    let d2 = tmpdir("omitted");
    let tasks2 = write(
        &d2,
        "tasks.json",
        r#"{ "tasks": [ { "id": "A", "title": "t", "accept": "true" } ] }"#,
    );
    let workers2 = write(&d2, "workers.json", WORKERS);
    let cfg2 = agentflow::config::load(&tasks2, &workers2).unwrap();
    assert!(cfg2.by_id["A"].gate_replay);
}
