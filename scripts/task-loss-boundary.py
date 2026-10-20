#!/usr/bin/env python3
"""Compare reading policies on the exact binary, source and cases of a prior run.

Uses disposable DB copies. The original DB is read only. Retained inventories
are examined offline for scoring, never passed to the blind investigator.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

from run_evidence import RunEvidence, atomic_json, provenance
from task_investigation import normalize_mode, run_bounded_investigation
from task_loss_boundary import investigation_summary, retained_loss_report

spec = importlib.util.spec_from_file_location("task_evaluate", Path(__file__).with_name("task-evaluate.py"))
evaluation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluation)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True, help="Existing task-evaluate.py JSON with diagnostics and manifest measurements")
    parser.add_argument("--scenarios", type=Path, required=True)
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True, help="Must match the baseline's executable digest")
    parser.add_argument("--root", type=Path, required=True, help="Same evaluation worktree as the baseline")
    parser.add_argument("--work", type=Path, required=True, help="New directory retaining checkpoints and raw RPC evidence")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--modes", default="manifest-first,inventory-first,excerpt-first")
    args = parser.parse_args()
    baseline_bytes, scenario_bytes = args.baseline.read_bytes(), args.scenarios.read_bytes()
    baseline = json.loads(baseline_bytes)
    scenarios = json.loads(scenario_bytes)["scenarios"]
    binary, root, database, work = (p.resolve() for p in (args.binary, args.root, args.db, args.work))
    binary_digest = "sha256:" + hashlib.sha256(binary.read_bytes()).hexdigest()
    if binary_digest != baseline["inputs"]["provenance"]["binary_sha256"]:
        parser.error("Executable differs from frozen baseline")
    if hashlib.sha256(scenario_bytes).hexdigest() != baseline["inputs"]["scenario_sha256"]:
        parser.error("Scenarios differ from frozen baseline")
    if str(root) != baseline["inputs"]["root"]:
        parser.error("Evaluation root differs from frozen baseline")
    modes = [normalize_mode(mode) for mode in args.modes.split(",")]
    if not modes or len(modes) != len(set(modes)) or args.workers < 1:
        parser.error("Use distinct supported modes and a positive worker count")
    if work.exists():
        parser.error("--work must be a new directory")
    limits = baseline["investigation_limits"]
    budget = baseline["inputs"]["manifest_bytes"]
    if not budget or baseline["inputs"].get("validation_error") or baseline.get("errors"):
        parser.error("Baseline must have valid pinned inputs and manifest measurements without errors")
    # Offline reads happen before any protocol calls. This does not recapture
    # source or infer a relationship from path presence.
    loss = retained_loss_report(database, baseline, scenarios)
    work.mkdir(parents=True)
    evidence = RunEvidence(work / "rpc")
    evaluator_provenance = provenance(Path(__file__).resolve().parents[1], binary)
    scored = {row["id"]: row for row in baseline["scenarios"]}
    servers = []

    def make_call(copy):
        server = evaluation.Served(binary, root, args.timeout, db=copy, evidence=evidence)
        servers.append(server)
        return evaluation.PinnedCall(server, baseline["inputs"]["source_view"])

    # WorkerPool copies only when workers>1. Force copying even in serial so
    # queries can retain reports without writing to the frozen DB.
    import tempfile
    import subprocess
    # Keep copies on the source filesystem where reflinks can share pages;
    # large databases should not be materialized repeatedly on /tmp tmpfs.
    with tempfile.TemporaryDirectory(prefix="databases-", dir=work) as directory:
        copy = Path(directory) / "briefcase.db"
        for suffix in ("", "-wal"):
            source = Path(str(database) + suffix)
            if source.exists():
                subprocess.run(["cp", "--reflink=auto", str(source), str(copy) + suffix], check=True)
        pool = evaluation.WorkerPool(make_call(copy), args.workers, make_call, copy)
        try:
            def investigate(call, scenario, mode):
                expected = scored[scenario["id"]]

                def pinned(operation, arguments):
                    result = call(operation, arguments)
                    if operation == "agent_context" and any(result.get(k) != expected[k] for k in ("case_id", "view_id", "index_id")):
                        raise ValueError("Investigation differs from frozen baseline case/index")
                    return result

                safe_id = hashlib.sha256(scenario["id"].encode()).hexdigest()[:16]
                result = run_bounded_investigation(pinned, work / mode / safe_id, scenario, mode=mode,
                    budget=budget, **limits)
                print(json.dumps({"id": scenario["id"], "mode": mode, "calls": result["calls"],
                    "inspected_required": len(result["recovered_required_paths"]),
                    "beyond_manifest": len(result["recovered_beyond_manifest"])}), flush=True)
                return {"id": scenario["id"], **result}

            pairs = [(mode, scenario) for mode in modes for scenario in scenarios]
            rows = pool.map([(scenario, lambda call, scenario=scenario, mode=mode: investigate(call, scenario, mode))
                             for mode, scenario in pairs])
        finally:
            for server in servers:
                server.stop()
            pool.close()
    investigations = {mode: [] for mode in modes}
    for (mode, _), row in zip(pairs, rows):
        investigations[mode].append(row)
    report = {"method": "frozen-loss-boundary-v1", "baseline_sha256": hashlib.sha256(baseline_bytes).hexdigest(),
        "scenario_sha256": hashlib.sha256(scenario_bytes).hexdigest(), "binary_sha256": binary_digest,
        "source_view": baseline["inputs"]["source_view"], "budget": budget, "limits": limits,
        "evaluator_provenance": evaluator_provenance, "loss": loss, "investigations": investigations,
        "summary": {mode: investigation_summary(rows) for mode, rows in investigations.items()},
        "interpretation": "Inspected means pinned source bytes fetched by the blind driver, not a supported claim or a correct answer. Full-file client digest verification is separate."}
    if args.scenarios.read_bytes() != scenario_bytes or args.baseline.read_bytes() != baseline_bytes:
        raise ValueError("Evaluation inputs changed during the comparison")
    if "sha256:" + hashlib.sha256(binary.read_bytes()).hexdigest() != binary_digest:
        raise ValueError("Frozen executable changed during the comparison")
    atomic_json(args.output, report)
    print(json.dumps({"output": str(args.output), "loss": loss["first_loss_stage_counts"], "summary": report["summary"]}))
    return int(any(summary["errors"] for summary in report["summary"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
