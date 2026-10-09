//! Quick start examples from the README — executable documentation.
//!
//! This test embeds the two JSON config examples from the README 'Quick start'
//! section verbatim and asserts they load and validate cleanly. This makes the
//! quickstart executable documentation: any config-schema change that breaks
//! the README fails CI.
//!
//! Expected: exactly 1 task, 1 enabled worker, exactly one warning about the
//! missing cost basis.

use agentflow::config::load;
use std::path::PathBuf;

/// The tasks.json example from the README 'Quick start' section (verbatim).
const README_TASKS_JSON: &str = r#"{ "tasks": [ {
    "id": "hello",
    "title": "Add a hello module",
    "scope": ["src/**"],
    "accept": "test -f src/hello.rs",
    "acceptance_prose": "src/hello.rs exists and declares pub fn hello()."
} ] }"#;

/// The workers.json example from the README 'Quick start' section (verbatim).
const README_WORKERS_JSON: &str = r#"{ "defaults": { "accept_timeout_s": 600, "max_attempts": 2 },
  "workers": [ {
    "name": "glm",
    "provider": "zai",
    "model": "glm-5.2",
    "api_base": "https://api.z.ai/api/paas/v4",
    "api_key_env": "ZAI_API_KEY",
    "enabled": true,
    "cli": "builtin",
    "max_turns": 48
} ] }"#;

/// Fresh, uniquely-named temp directory (one per call), emptied if a stale
/// one exists. Unique across tests via pid.
fn fresh_dir(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "af-quickstart-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create unique temp dir");
    d
}

/// Write `content` to `path` (creating parents) and return the path.
fn write_file(path: &std::path::Path, content: &str) -> PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, content).expect("write fixture");
    path.to_path_buf()
}

#[test]
fn quickstart_examples_load_and_validate() {
    let d = fresh_dir("examples");

    let tasks_path = write_file(&d.join("tasks.json"), README_TASKS_JSON);
    let workers_path = write_file(&d.join("workers.json"), README_WORKERS_JSON);

    let cfg = load(&tasks_path, &workers_path).expect("README examples should load cleanly");

    // Exactly 1 task
    assert_eq!(cfg.tasks.len(), 1, "should have exactly 1 task");
    assert_eq!(cfg.tasks[0].id, "hello");
    assert_eq!(cfg.tasks[0].title, "Add a hello module");
    assert_eq!(cfg.tasks[0].scope, vec!["src/**"]);
    assert_eq!(cfg.tasks[0].accept, Some("test -f src/hello.rs".to_string()));
    assert_eq!(
        cfg.tasks[0].acceptance_prose,
        Some("src/hello.rs exists and declares pub fn hello().".to_string())
    );

    // Exactly 1 enabled worker
    assert_eq!(cfg.workers.len(), 1, "should have exactly 1 worker");
    assert_eq!(cfg.workers[0].name, "glm");
    assert_eq!(cfg.workers[0].provider, "zai");
    assert_eq!(cfg.workers[0].model, "glm-5.2");
    assert!(cfg.workers[0].enabled, "worker should be enabled");
    assert_eq!(cfg.workers[0].cli, "builtin");
    assert_eq!(cfg.workers[0].max_turns, Some(48));

    // Exactly one warning about the missing cost basis
    assert_eq!(cfg.warnings.len(), 1, "should have exactly 1 warning");
    assert!(
        cfg.warnings[0].contains("no cost basis"),
        "warning should mention missing cost basis: {}",
        cfg.warnings[0]
    );
}
