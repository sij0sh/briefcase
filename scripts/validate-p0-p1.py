#!/usr/bin/env python3
"""Capture P0/P1 build/test evidence. Missing dependencies are explicit blockers."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys

from run_evidence import RunEvidence, atomic_json, digest, implementation, provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New directory outside the implementation tree")
    parser.add_argument("--baseline-db", type=Path, help="Freshly synced historical baseline database, when the original corpus is available")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    if output.is_relative_to(root):
        parser.error("Validation evidence must be outside the measured implementation")
    if output.exists() and any(output.iterdir()):
        parser.error("Use a new output directory")
    run = RunEvidence(output / "raw")
    binary = root / "target/release/briefcase"
    before = implementation(root)
    report = {"scope": "P0/P1 implementation validation", "status": "partial_validation", "checks": [],
              "inputs": provenance(root, binary), "historical_corpus": "not_supplied"}

    def check(name, command, *, timeout=60, cwd=root):
        if not shutil.which(str(command[0])) and not Path(command[0]).is_file():
            row = {"id": name, "status": "blocked", "command": [str(x) for x in command], "reason": "Executable unavailable: " + str(command[0])}
        else:
            try:
                result = run.run(command, cwd=cwd, timeout=timeout)
                row = {"id": name, "status": "passed" if result.returncode == 0 else "failed", "exit_code": result.returncode,
                       "command": [str(x) for x in command]}
            except (OSError, subprocess.TimeoutExpired) as error:
                row = {"id": name, "status": "failed", "error": str(error), "command": [str(x) for x in command]}
        report["checks"].append(row)
        return row

    build = check("release-build", ["cargo", "build", "--locked", "--release"], timeout=600)
    if build["status"] == "passed" and implementation(root) == before:
        atomic_json(output / "build-receipt.json", {"implementation_sha256": before["sha256"],
                    "binary_sha256": digest(binary.read_bytes()), "build": build, "toolchain": report["inputs"]["toolchain"]})
    check("python", [sys.executable, "-B", "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"])
    check("schemas-and-tools", [sys.executable, "-B", "-m", "unittest", "discover", "-s", "tools", "-p", "test_*.py"])
    check("owned-schemas", [sys.executable, "-B", "tools/check_package.py", "--schemas-only"])
    check("adapter", ["node", "--test", "--test-reporter=spec", "tests/briefcase-task-adapter.test.mjs"])
    check("rust-context", ["cargo", "test", "--locked", "--lib", "--test", "context", "--test", "stage2", "--test", "stage3", "--test", "stage4", "--test", "p0p1"], timeout=600)
    if build["status"] == "passed":
        # Installed adapter tests select the debug binary by default.
        if check("debug-binary", ["cargo", "build", "--locked", "--bins"], timeout=600)["status"] == "passed":
            check("installed-pi", ["node", "--test", "tests/briefcase-pi.test.mjs"], timeout=180)
        check("real-protocol", [sys.executable, "-B", "scripts/p0-p1-smoke.py", "--binary", str(binary), "--output", str(output / "protocol")], timeout=600)
    else:
        report["checks"].append({"id": "real-protocol", "status": "blocked", "reason": "A freshly built executable is required"})
        report["checks"].append({"id": "installed-pi", "status": "blocked", "reason": "The compiled installer and adapter are required"})
    missing = [p for p in ("evaluation/fixtures/index.json", "evaluation/sources", "evaluation/task-scenarios.json", "evaluation/heldout-scenarios.json") if not (root / p).exists()]
    if not missing and args.baseline_db and args.baseline_db.is_file() and build["status"] == "passed":
        report["historical_corpus"] = "supplied"
        check("historical-baseline", [sys.executable, "-B", "scripts/task-controls.py", "--binary", str(binary),
              "--root", str(root), "--db", str(args.baseline_db.resolve()), "--output", str(output / "historical"), "--control-ablations"], timeout=3600)
    else:
        report["checks"].append({"id": "historical-baseline", "status": "blocked", "missing_assets": missing,
                                 "reason": "Supply the historical corpus and --baseline-db, then build, before reproducing historical recall"})
    if implementation(root) != before:
        report["checks"].append({"id": "stable-inputs", "status": "failed", "reason": "Implementation changed during validation"})
    if any(c["status"] == "failed" for c in report["checks"]):
        report["status"] = "failed"
    elif all(c["status"] == "passed" for c in report["checks"]):
        report["status"] = "passed"
    report["summary"] = {state: sum(c["status"] == state for c in report["checks"]) for state in ("passed", "failed", "blocked")}
    atomic_json(output / "validation.json", report)
    print(json.dumps({k: v for k, v in report.items() if k != "inputs"}, indent=2))
    return 1 if report["status"] == "failed" else 0 if report["status"] == "passed" else 2


if __name__ == "__main__":
    raise SystemExit(main())
