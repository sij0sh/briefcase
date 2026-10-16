#!/usr/bin/env python3
"""Measure real task-packet recall from labelled required/follow-up paths.

Example: python3 scripts/task-evaluate.py --binary target/release/briefcase \
  --root . --cases task-cases.json --output task-results.json

Cases: {"cases": [{"id": "rename", "task": "rename ...", "target": "src/...",
  "duty": "refactor", "required_paths": ["src/...", "tests/..."],
  "required_gap_areas": ["external_installations"]}]}.
No expected files are inferred by the evaluator. Unsupported operations,
timeouts, delivery omissions and malformed responses are failures, not zeros.
"""
import argparse
import json
from pathlib import Path
import statistics
import subprocess
import time

ROLES = ("impl", "test", "config", "script", "build", "install", "doc",
         "adr", "plan", "fixture", "generated")


def execute(binary, root, operation, arguments, timeout):
    result = subprocess.run(
        [str(binary), "--project-root", str(root), "--json", "call", operation,
         "--args", json.dumps(arguments, ensure_ascii=False)],
        cwd=root, capture_output=True, text=True, timeout=timeout,
    )
    if result.returncode:
        raise RuntimeError((result.stderr or result.stdout).strip()[:2000])
    value = json.loads(result.stdout)
    if not isinstance(value, dict):
        raise ValueError("Expected one JSON object")
    return value


def packet_section(packet, section, call):
    """Read the exact retained section; detect stalled/incorrect cursors."""
    parts = section.strip("/").split("/")
    current = packet
    for part in parts:
        current = current.get(part, {}) if isinstance(current, dict) else {}
    initial = current if isinstance(current, list) else []
    delivery = packet.get("coverage", {}).get("delivery", {}).get(section, {})
    if delivery and not delivery.get("omitted"):
        return initial
    report = packet.get("id") or packet.get("report_id")
    if not report or not report.startswith("task-brief:"):
        raise ValueError("No retained task-brief ID for exhaustive section retrieval")
    offset, collected = 0, []
    while True:
        page = call("page", {"report_id": report, "section": section,
                                 "offset": offset, "limit": 1000,
                                 "max_bytes": 1048576})
        if "items" not in page or "page" not in page:
            raise ValueError("Section page was omitted by its response budget")
        if page.get("historical_packet"):
            raise ValueError("Retractions changed during evaluation; rerun on stable inputs")
        items = page["items"]
        collected.extend(items)
        next_offset = page["page"].get("next_offset")
        if next_offset is None:
            if len(collected) != page["page"]["total"]:
                raise ValueError("Delivered section count differs from its retained total")
            return collected
        if next_offset != offset + len(items) or next_offset <= offset:
            raise ValueError("Non-contiguous or stalled section pagination")
        offset = next_offset


def evaluate_case(case, call):
    expected = set(case.get("required_paths", case.get("followup_paths", [])))
    if not expected:
        raise ValueError("Each case needs labelled required_paths or followup_paths")
    arguments = {"task": case["task"], "max_bytes": 1048576}
    for name in ("target", "duty", "view", "depth"):
        if name in case:
            arguments[name] = case[name]
    started = time.monotonic()
    packet = call("brief", arguments)
    first_ms = (time.monotonic() - started) * 1000
    found = {item["path"] for role in ROLES
             for item in packet_section(packet, f"/slice/{role}", call)}
    gaps = {item.get("area") for item in packet_section(packet, "/not_covered", call)}
    missing = sorted(expected - found)
    forbidden = sorted(found.intersection(case.get("forbidden_paths", [])))
    missing_gaps = sorted(set(case.get("required_gap_areas", [])) - gaps)
    return {"id": case["id"], "status": "pass" if not (missing or forbidden or missing_gaps) else "miss",
            "report_id": packet.get("id"), "view_id": packet.get("view_id"),
            "required_paths": len(expected), "recalled_paths": len(expected & found),
            "required_path_recall": len(expected & found) / len(expected),
            "missing_paths": missing, "forbidden_paths_returned": forbidden,
            "missing_gap_areas": missing_gaps, "first_packet_ms": round(first_ms, 3),
            "full_delivery_ms": round((time.monotonic() - started) * 1000, 3)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/briefcase"))
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    cases = json.loads(args.cases.read_text())["cases"]
    if not cases:
        parser.error("At least one labelled case is required")
    binary, root = args.binary.resolve(), args.root.resolve()
    call = lambda operation, arguments: execute(binary, root, operation, arguments, args.timeout)
    results = []
    for case in cases:
        try:
            results.append(evaluate_case(case, call))
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
            results.append({"id": case.get("id"), "status": "error", "error": str(error)})
    timings = [r["first_packet_ms"] for r in results if "first_packet_ms" in r]
    report = {"method": "missed-context-task-packets-v1", "cases": results,
              "passed": sum(r["status"] == "pass" for r in results),
              "missed": sum(r["status"] == "miss" for r in results),
              "errors": sum(r["status"] == "error" for r in results),
              "median_first_packet_ms": statistics.median(timings) if timings else None,
              "interpretation": "Labelled task evidence recall, not a claim of safe patch completion. Cold/warm timing and case provenance must be reported separately."}
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded)
    print(encoded, end="")
    return 0 if report["passed"] == len(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
