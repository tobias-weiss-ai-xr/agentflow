# Compliance Reporting

`scripts/compliance.py` runs the project's static checks and full test suite,
then writes timestamped, machine-readable evidence of the run: git info,
environment, every test result, and overall duration.

## Running

```sh
make compliance
```

This is the single command to check overall project health before shipping.
It exits non-zero if any test fails or a static check is dirty, so it also
works as a CI gate.

## Reports

Reports are written to `target/compliance/` (gitignored, so generated
evidence never pollutes the tree) with a UTC timestamp in the filename:

- `compliance-<timestamp>.json` — full machine-readable report
- `compliance-<timestamp>.md` — human-readable summary

To write reports elsewhere:

```sh
python3 scripts/compliance.py /some/other/dir
```

## What's in a report

| Field | Meaning |
|-------|---------|
| `generated` | UTC timestamp of the run |
| `duration_seconds` | wall time of the whole run |
| `result` | `PASS` / `FAIL` |
| `command` | the test command executed (`cargo test`) |
| `git` | commit SHA, branch, `git describe`, and dirty-file list |
| `environment` | Python, platform, uname, and git / cargo / rustc versions |
| `static_checks` | result of `cargo fmt --check` (`clean` / `dirty`) |
| `tests` | summed passed/failed, the names of any failing tests, and one entry per `test result:` line |
| `output` | full captured test output (stdout, with stderr merged) |

A run only reports `PASS` when the static check is clean, no test result line
is `FAILED`, and the test process exits 0.

## Self-check

Sanity-check the result parser (no tests run):

```sh
python3 scripts/compliance.py --selfcheck
```
