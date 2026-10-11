# Compliance Reporting

`scripts/compliance.py` runs the full test suite and writes timestamped,
machine-readable evidence of a passing build: git info, environment, every
test result, and overall duration.

## Running

```sh
make compliance
```

This is the single command to check overall project health before shipping.
It exits non-zero if any test fails, so it also works as a CI gate.

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
| `environment` | Python, platform, `cargo` and `rustc` versions |
| `tests` | summed passed/failed plus one entry per `test result:` line |
| `output` | full captured test output (stdout, with stderr merged) |

Self-check of the result parser (no tests run):

```sh
python3 scripts/compliance.py --selfcheck
```
