#!/usr/bin/env python3
"""Audit exact contract/configuration/state-key joins for missed required paths
of a frozen task evaluation. Read-only against the database and source root."""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from task_key_audit import audit  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True, help="task-evaluate.py JSON whose cases are retained in --db")
    parser.add_argument("--scenarios", type=Path, required=True)
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True, help="Source tree the baseline captured")
    parser.add_argument("--only", default="", help="Comma-separated scenario ids")
    parser.add_argument("--specificity", type=int, default=8, help="Largest repository file count for a key to count as specific")
    parser.add_argument("--output", type=Path, required=True)
    a = parser.parse_args()
    scenarios = json.loads(a.scenarios.read_text())["scenarios"]
    if a.only:
        wanted = set(a.only.split(","))
        scenarios = [s for s in scenarios if s["id"] in wanted]
    report = audit(a.db, json.loads(a.baseline.read_text()), scenarios, a.root, a.specificity)
    a.output.write_text(json.dumps(report, indent=1) + "\n")
    print(json.dumps(report["verdict_counts"]))


if __name__ == "__main__":
    main()
