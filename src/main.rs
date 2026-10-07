//! The `af` binary: thin CLI over the agentflow library.

use agentflow::config::{self, Settings};
use agentflow::run::{self, RunOptions};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
af — parallel LLM task execution on isolated git worktrees

USAGE:
  af run       [--once] [--dry-run] [--worker NAME] [--task ID] [--poll SECS] [--tasks FILE] [--workers FILE]
  af status    [--json]
  af api       status [--json] | results --task ID
  af attach    ID
  af cost      [--task ID] [--last] [--since DATE|UNIX_TS]
  af clean     [--dry-run]
  af recover   --task ID [--dry-run]
  af validate  [--worker NAME] [--tasks FILE] [--workers FILE]
  af --help | --version

COST REPORT WINDOWS (af cost):
  --last                aggregate only the most recent receipt per task:
                        the one with the greatest ts, ties broken by the
                        greater attempt number — retries are not
                        double-counted. ATTEMPTS is 1 per task.
  --since DATE|UNIX_TS  aggregate only receipts with ts >= the instant;
                        DATE is YYYY-MM-DD (UTC midnight) or a bare unix
                        timestamp. Applied before --last, so
                        `--last --since D` = the latest attempt per task
                        since D.
  The windows compose with each other and with --task ID (which narrows
  the table rows); the TOTAL line and the per-worker trust block are
  always computed over the selected receipts only. Plain `af cost`
  totals every attempt of every task.

ENV: TF_REPO_DIR, TF_STATE_DIR, TF_MAX_PARALLEL, TF_BRANCH_PREFIX, TF_POLL,
     TF_GATE_ENV, TF_TASKS_JSON, TF_WORKERS_JSON, TF_AGENT_TIMEOUT_S";

#[derive(Debug)]
struct Args {
    cmd: String,
    once: bool,
    dry_run: bool,
    worker: Option<String>,
    task: Option<String>,
    poll: Option<u64>,
    json: bool,
    tasks_file: Option<PathBuf>,
    workers_file: Option<PathBuf>,
    repos_file: Option<PathBuf>,
    last: bool,
    since: Option<u64>,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        cmd: String::new(),
        once: false,
        dry_run: false,
        worker: None,
        task: None,
        poll: None,
        json: false,
        tasks_file: None,
        workers_file: None,
        repos_file: None,
        last: false,
        since: None,
    };
    let mut it = argv.iter();
    a.cmd = it.next().cloned().unwrap_or_default();
    // `api` collapses into its subcommand: "status" | "results"
    if a.cmd == "api" {
        let sub = it
            .next()
            .ok_or("af api requires a subcommand: status | results")?;
        a.cmd = format!("api-{sub}");
    }
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--once" => a.once = true,
            "--dry-run" => a.dry_run = true,
            "--json" => a.json = true,
            "--worker" => a.worker = Some(it.next().ok_or("--worker needs a value")?.clone()),
            "--task" => a.task = Some(it.next().ok_or("--task needs a value")?.clone()),
            "--poll" => {
                a.poll = Some(
                    it.next()
                        .ok_or("--poll needs a value")?
                        .parse()
                        .map_err(|_| "--poll must be an integer")?,
                )
            }
            "--tasks" => {
                a.tasks_file = Some(PathBuf::from(it.next().ok_or("--tasks needs a value")?))
            }
            "--workers" => {
                a.workers_file = Some(PathBuf::from(it.next().ok_or("--workers needs a value")?))
            }
            "--repos" => {
                a.repos_file = Some(PathBuf::from(it.next().ok_or("--repos needs a value")?))
            }
            "--last" => a.last = true,
            "--since" => {
                let v = it.next().ok_or("--since needs a value")?;
                a.since = Some(run::parse_since(v).map_err(|e| format!("--since: {e}"))?);
            }
            other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
            other => {
                // bare positional (e.g. `af attach <id>`)
                if a.task.is_none() {
                    a.task = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument: {other}"));
                }
            }
        }
    }
    Ok(a)
}

fn load_cfg(args: &Args) -> Result<(config::Config, Settings), String> {
    let st = Settings::from_env();
    let tasks = args
        .tasks_file
        .clone()
        .unwrap_or_else(|| st.tasks_file.clone());
    let workers = args
        .workers_file
        .clone()
        .unwrap_or_else(|| st.workers_file.clone());
    let cfg = config::load(&tasks, &workers)?;
    // Multi-repo (ADR-11): --repos > TF_REPOS_JSON > <tasks dir>/repos.json;
    // missing file = single-repo mode.
    let repos_path = args
        .repos_file
        .clone()
        .or_else(|| std::env::var("TF_REPOS_JSON").ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            tasks
                .parent()
                .map(|p| p.join("repos.json"))
                .unwrap_or_else(|| PathBuf::from("repos.json"))
        });
    let mut cfg = cfg;
    cfg.repos = config::load_repos(&repos_path)?;
    let repo_warns = cfg.repo_warnings(&st.repo_dir);
    cfg.warnings.extend(repo_warns); // printed by the common loop below
    for w in &cfg.warnings {
        eprintln!("warning: {w}");
    }
    Ok((cfg, st))
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() || argv[0] == "--help" || argv[0] == "-h" {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if argv[0] == "--version" || argv[0] == "-V" {
        println!("af {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let (cfg, st) = match load_cfg(&args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(2);
        }
    };

    let code = match args.cmd.as_str() {
        "run" => {
            let opts = RunOptions {
                once: args.once,
                dry_run: args.dry_run,
                worker_filter: args.worker.clone(),
                task_filter: args.task.clone(),
                poll_secs: args.poll,
            };
            run::run_loop(&cfg, &st, &opts)
        }
        "status" => {
            if args.json {
                println!("{}", run::status_json(&cfg, &st));
            } else {
                println!("{}", run::status_board(&cfg, &st));
            }
            0
        }
        "api-status" => {
            if args.json {
                println!("{}", run::status_json(&cfg, &st));
            } else {
                println!("{}", run::status_board(&cfg, &st));
            }
            0
        }
        "api-results" => {
            let id = match &args.task {
                Some(t) => t.clone(),
                None => {
                    eprintln!("error: af api results requires --task ID");
                    return ExitCode::from(2);
                }
            };
            print!("{}", run::results(&cfg, &st, &id));
            0
        }
        "attach" => {
            let id = match &args.task {
                Some(t) => t.clone(),
                None => {
                    eprintln!("error: af attach requires a task id (af attach <id>)");
                    return ExitCode::from(2);
                }
            };
            run::attach(&st, &id)
        }
        "cost" => {
            let filter = run::CostFilter {
                task: args.task.clone(),
                last: args.last,
                since: args.since,
            };
            println!("{}", run::cost(&cfg, &st, &filter));
            0
        }
        "clean" => run::clean(&cfg, &st, args.dry_run),
        "recover" => match &args.task {
            Some(id) => run::recover(&cfg, &st, id, args.dry_run),
            None => {
                eprintln!("error: af recover requires --task ID");
                2
            }
        },
        "validate" => {
            // Pre-flight only: load_cfg already loaded + validated the config
            // (hard errors exit 2 before dispatch) and printed its warnings.
            // Nothing is dispatched here — no scheduler, no agents, no state.
            if let Some(name) = &args.worker {
                if !cfg.workers.iter().any(|w| &w.name == name && w.enabled) {
                    eprintln!(
                        "error: unknown or disabled worker '{name}' (known workers: {})",
                        cfg.workers
                            .iter()
                            .map(|w| w.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    return ExitCode::from(2);
                }
            }
            let enabled = cfg.workers.iter().filter(|w| w.enabled).count();
            println!(
                "config OK: {} task(s), {} worker(s), {} enabled, {} warning(s)",
                cfg.tasks.len(),
                cfg.workers.len(),
                enabled,
                cfg.warnings.len()
            );
            0
        }
        other => {
            eprintln!("error: unknown command '{other}'\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    ExitCode::from(code.clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_run_flags() {
        let argv = vec![
            "run".to_string(),
            "--once".to_string(),
            "--worker".to_string(),
            "w1".to_string(),
            "--poll".to_string(),
            "3".to_string(),
        ];
        let a = parse(&argv).unwrap();
        assert_eq!(a.cmd, "run");
        assert!(a.once);
        assert_eq!(a.worker.as_deref(), Some("w1"));
        assert_eq!(a.poll, Some(3));
    }

    #[test]
    fn parse_api_subcommands() {
        let a = parse(&[
            "api".to_string(),
            "status".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        assert_eq!(a.cmd, "api-status");
        assert!(a.json);
        let a = parse(&[
            "api".to_string(),
            "results".to_string(),
            "--task".to_string(),
            "X".to_string(),
        ])
        .unwrap();
        assert_eq!(a.cmd, "api-results");
        assert_eq!(a.task.as_deref(), Some("X"));
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        assert!(parse(&["run".to_string(), "--nope".to_string()]).is_err());
    }

    #[test]
    fn parse_cost_window_flags() {
        let a = parse(&[
            "cost".to_string(),
            "--last".to_string(),
            "--since".to_string(),
            "2024-01-01".to_string(),
        ])
        .unwrap();
        assert_eq!(a.cmd, "cost");
        assert!(a.last);
        assert_eq!(a.since, Some(1_704_067_200), "YYYY-MM-DD is UTC midnight");

        let a = parse(&[
            "cost".to_string(),
            "--since".to_string(),
            "1700000000".to_string(),
        ])
        .unwrap();
        assert!(!a.last);
        assert_eq!(a.since, Some(1_700_000_000), "bare unix timestamp");
    }

    #[test]
    fn parse_rejects_bad_since_value() {
        // A missing value and an unparseable value are errors naming the
        // flag — surfaced as exit 2 by main(), never a panic.
        assert!(parse(&["cost".to_string(), "--since".to_string()]).is_err());
        let err = parse(&[
            "cost".to_string(),
            "--since".to_string(),
            "not-a-date".to_string(),
        ])
        .unwrap_err();
        assert!(err.starts_with("--since:"), "{err}");
        assert!(err.contains("not-a-date"), "{err}");
    }
}
