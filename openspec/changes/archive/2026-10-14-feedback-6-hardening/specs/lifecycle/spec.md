# lifecycle Delta — feedback-6-hardening

## ADDED Requirements

### Requirement: Dispatch rounds read the current task config

Each dispatch round of `af run` SHALL be planned against the current
tasks/workers files (mtime-gated hot reload), and a reload failure SHALL
degrade to the last good config with a single warning, never to a campaign
abort.

#### Scenario: hot reload refreshes ExecCtx for dispatch

- Given a campaign mid-run
- When the workers file changes (e.g. a worker is disabled) between rounds
- Then subsequent dispatch picks reflect the change without a restart

(Shared scenarios for failure handling live in the config delta.)

### Requirement: Cooperative stop drains in-flight attempts

`af run` SHALL write a pid file (`af.pid`) into the state dir and SHALL check
a stop flag file (`stop`) each poll round; when the flag is present the loop
SHALL stop dispatching new attempts, let in-flight attempts finish through
their normal reap path, and exit non-zero with the board. The `af stop`
subcommand SHALL signal the pid recorded in the pid file (best effort) and
create the flag file. A hard kill of the orchestrator SHALL remain safe via
the existing startup heal.

#### Scenario: stop flag halts new dispatches and drains

- Given a running campaign with one in-flight attempt
- When the stop flag file appears in the state dir
- Then no new task is dispatched
- And the loop exits only after the in-flight attempt reaps, printing the
  board with a stopped marker and exiting non-zero

#### Scenario: af stop signals the pid file owner

- Given `af.pid` in the state dir naming a live af process
- When the operator runs `af stop`
- Then the orchestrator process is terminated and the flag file is created

#### Scenario: crash log captures an unexpected panic

- Given a running campaign
- When the run loop panics
- Then the panic message and location are appended to `<state dir>/crash.log`
- And the default panic output is preserved
