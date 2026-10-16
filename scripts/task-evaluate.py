#!/usr/bin/env python3
"""Measure real task-case recall from labelled required/follow-up paths.

Example: python3 scripts/task-evaluate.py --binary target/release/briefcase \
  --root . --scenarios task-scenarios.json --output task-results.json

Scenarios: {"scenarios": [{"id": "rename", "task": "rename ...", "target": "src/...",
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
    report = packet.get("case_id") or packet.get("id") or packet.get("report_id")
    if not report or not report.startswith(("task-case:", "task-brief:")):
        raise ValueError("No retained task-case or legacy task-brief ID for exhaustive section retrieval")
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


def evaluate_scenario(scenario, call):
    expected = set(scenario.get("required_paths", scenario.get("followup_paths", [])))
    if not expected:
        raise ValueError("Each scenario needs labelled required_paths or followup_paths")
    arguments = {"task": scenario["task"], "max_bytes": 1048576}
    for name in ("target", "duty", "view", "depth"):
        if name in scenario:
            arguments[name] = scenario[name]
    started = time.monotonic()
    packet = call("brief", arguments)
    outcome = packet.get("outcome")
    if outcome not in ("complete", "partial", "unknown"):
        raise ValueError("Brief outcome is missing or invalid")
    first_ms = (time.monotonic() - started) * 1000
    found = {item["path"] for role in ROLES
             for item in packet_section(packet, f"/slice/{role}", call)}
    gaps = {item.get("area") for item in packet_section(packet, "/not_covered", call)}
    missing = sorted(expected - found)
    forbidden = sorted(found.intersection(scenario.get("forbidden_paths", [])))
    missing_gaps = sorted(set(scenario.get("required_gap_areas", [])) - gaps)
    return {"id": scenario["id"], "status": "pass" if not (missing or forbidden or missing_gaps) else "miss",
            "report_id": packet.get("report_id") or packet.get("id"),
            "case_id": packet.get("case_id"), "view_id": packet.get("view_id"),
            "outcome": outcome, "false_complete": outcome == "complete" and bool(missing),
            "required_paths": len(expected), "recalled_paths": len(expected & found),
            "required_path_recall": len(expected & found) / len(expected),
            "missing_paths": missing, "forbidden_paths_returned": forbidden,
            "missing_gap_areas": missing_gaps, "first_packet_ms": round(first_ms, 3),
            "full_delivery_ms": round((time.monotonic() - started) * 1000, 3)}


def false_complete_metrics(results):
    """Errors are not scored as successes. Report both denominators explicitly."""
    scored = [r for r in results if r.get("status") != "error" and "false_complete" in r]
    complete = [r for r in scored if r.get("outcome") == "complete"]
    failures = sum(r["false_complete"] for r in scored)
    return {"false_complete_count": failures, "scored_scenarios": len(scored),
            "complete_outcomes": len(complete),
            "false_complete_rate": failures / len(scored) if scored else None,
            "false_complete_given_complete_rate": failures / len(complete) if complete else None,
            "false_complete_definition": "Complete outcome with at least one authored required path missing, divided by scored scenarios."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/briefcase"))
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--scenarios", "--cases", dest="scenarios", type=Path, required=True,
                        help="Authored scenarios JSON; --cases is a deprecated alias")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    authored = json.loads(args.scenarios.read_text())
    scenarios = authored.get("scenarios", authored.get("cases", []))
    if not scenarios:
        parser.error("At least one labelled scenario is required")
    binary, root = args.binary.resolve(), args.root.resolve()
    call = lambda operation, arguments: execute(binary, root, operation, arguments, args.timeout)
    results = []
    for scenario in scenarios:
        try:
            results.append(evaluate_scenario(scenario, call))
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
            results.append({"id": scenario.get("id"), "status": "error", "error": str(error)})
    timings = [r["first_packet_ms"] for r in results if "first_packet_ms" in r]
    report = {"method": "missed-context-task-cases-v1", "scenarios": results,
              "passed": sum(r["status"] == "pass" for r in results),
              "missed": sum(r["status"] == "miss" for r in results),
              "errors": sum(r["status"] == "error" for r in results),
              **false_complete_metrics(results),
              "median_first_packet_ms": statistics.median(timings) if timings else None,
              "interpretation": "Labelled task evidence recall, not a claim of safe patch completion. Cold/warm timing and scenario provenance must be reported separately."}
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded)
    print(encoded, end="")
    return 0 if report["passed"] == len(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
