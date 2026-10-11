#!/usr/bin/env python3
"""Run all tests and emit timestamped compliance reports (JSON + Markdown).

Reports include git info, environment, every test result, and duration.
Written to <repo>/target/compliance/ (gitignored) unless a directory is
given as argv[1]. Exit code mirrors the test run (0 = all passed).

Usage:
  scripts/compliance.py                 # run all tests, write reports
  scripts/compliance.py --selfcheck     # sanity-check the parser, no tests
"""
import json
import os
import platform
import re
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone

RESULT_RE = re.compile(r"test result: (ok|FAILED)\.?\s+(\d+) passed; (\d+) failed")


def run(cmd, cwd=None):
    """Run a command, return (returncode, stdout, stderr)."""
    proc = subprocess.run(cmd, capture_output=True, text=True, cwd=cwd)
    return proc.returncode, proc.stdout.strip(), proc.stderr.strip()


def parse_results(stdout):
    """Sum passed/failed across every 'test result' line (one per test binary)."""
    passed = failed = 0
    lines = []
    for line in stdout.splitlines():
        m = RESULT_RE.search(line)
        if m:
            p, f = int(m.group(2)), int(m.group(3))
            passed += p
            failed += f
            lines.append({"result": m.group(1), "passed": p, "failed": f, "line": line.strip()})
    return passed, failed, lines


def main():
    # Survive being piped to `head` (e.g. `make compliance | head -20`):
    # a closed stdout must not kill the run with SIGPIPE.
    try:
        signal.signal(signal.SIGPIPE, signal.SIG_IGN)
    except (AttributeError, ValueError):  # non-POSIX / non-main-thread
        pass

    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

    if "--selfcheck" in sys.argv:
        sample = (
            "running 3 tests\n"
            "test result: ok. 340 passed; 1 failed; 0 ignored; finished in 0.63s\n"
            "test result: FAILED. 2 passed; 1 failed; finished in 0.10s\n"
        )
        passed, failed, _ = parse_results(sample)
        assert passed == 342 and failed == 2, (passed, failed)
        print("compliance: parser self-check OK")
        return 0

    out_dir = sys.argv[1] if len(sys.argv) > 1 else os.path.join(repo_root, "target", "compliance")
    os.makedirs(out_dir, exist_ok=True)

    generated = datetime.now(timezone.utc).isoformat(timespec="seconds")
    started = time.monotonic()

    # Git info
    git = {}
    for key, args in [
        ("commit", ["git", "rev-parse", "HEAD"]),
        ("branch", ["git", "branch", "--show-current"]),
        ("describe", ["git", "describe", "--tags", "--always"]),
    ]:
        code, out, err = run(args, cwd=repo_root)
        git[key] = out if code == 0 else err
    code, out, _ = run(["git", "status", "--porcelain"], cwd=repo_root)
    git["dirty_files"] = out.splitlines() if code == 0 else []

    # Environment
    env = {
        "python": platform.python_version(),
        "platform": platform.platform(),
        "cargo": run(["cargo", "--version"])[1],
        "rustc": run(["rustc", "--version"])[1],
    }

    # Run all tests once, capturing output for the report
    print("compliance: running `cargo test` ...")
    proc = subprocess.Popen(["cargo", "test"], cwd=repo_root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    tout, _ = proc.communicate()
    duration = round(time.monotonic() - started, 2)

    passed, failed, results = parse_results(tout)
    ok = proc.returncode == 0 and failed == 0

    report = {
        "generated": generated,
        "duration_seconds": duration,
        "result": "PASS" if ok else "FAIL",
        "command": "cargo test",
        "git": git,
        "environment": env,
        "tests": {"passed": passed, "failed": failed, "results": results},
        "output": {"stdout": tout},
    }

    base = f"compliance-{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')}"
    with open(os.path.join(out_dir, base + ".json"), "w") as fh:
        json.dump(report, fh, indent=2)
    with open(os.path.join(out_dir, base + ".md"), "w") as fh:
        fh.write(render_markdown(report))

    try:
        print(f"compliance: {report['result']} — {passed} passed, {failed} failed, "
              f"{duration}s — reports in {out_dir}")
    except BrokenPipeError:
        pass
    return 0 if ok else 1


def render_markdown(r):
    lines = [
        f"# Compliance Report — {r['result']}",
        "",
        f"- Generated: `{r['generated']}`",
        f"- Command: `{r['command']}`",
        f"- Duration: {r['duration_seconds']}s",
        f"- Tests: **{r['tests']['passed']} passed, {r['tests']['failed']} failed**",
        "",
        "## Git",
        "",
    ]
    for k, v in r["git"].items():
        if k == "dirty_files":
            lines.append(f"- dirty files: {len(v)}{' — ' + ', '.join(v) if v else ''}")
        else:
            lines.append(f"- {k}: `{v}`")
    lines += ["", "## Environment", ""]
    for k, v in r["environment"].items():
        lines.append(f"- {k}: `{v}`")
    lines += ["", "## Test results", ""]
    for res in r["tests"]["results"]:
        lines.append(f"- {res['result']:6} {res['passed']} passed, {res['failed']} failed")
    if not r["tests"]["results"]:
        lines.append("_No `test result` lines parsed._")
    lines.append("")
    return "\n".join(lines)


if __name__ == "__main__":
    sys.exit(main())
