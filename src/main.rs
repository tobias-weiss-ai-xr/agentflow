//! The `af` binary: thin CLI over the agentflow library.

use agentflow::config::{self, Settings};
use agentflow::run::{self, RunOptions};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
af — parallel LLM task execution on isolated git worktrees

USAGE:
  af run       [--once] [--dry-run] [--worker NAME] [--task ID] [--poll SECS] [--tasks FILE] [--workers FILE]
  af status
  af api       status [--json] | results --task ID
  af attach    ID
  af cost      [--task ID]
  af --help | --version

ENV: TF_REPO_DIR, TF_STATE_DIR, TF_MAX_PARALLEL, TF_BRANCH_PREFIX, TF_POLL,
     TF_GATE_ENV, TF_TASKS_JSON, TF_WORKERS_JSON, TF_AGENT_TIMEOUT_S";

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
            println!("{}", run::status_board(&cfg, &st));
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
            println!("{}", run::cost(&cfg, &st, args.task.as_deref()));
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
}
