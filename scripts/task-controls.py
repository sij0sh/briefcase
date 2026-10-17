#!/usr/bin/env python3
"""Compare retained context with a clean source clone and run held-out ablations.

Both conditions use the same executable, scenario files, source bytes and
manifest budget. The control gets a fresh database and no imported history or
sessions. Results are measurements, not a claim of safe patch completion.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import time

SPEC = importlib.util.spec_from_file_location("task_evaluate", Path(__file__).with_name("task-evaluate.py"))
EVALUATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVALUATOR)


def file_digest(path):
    return "sha256:" + hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def inventory(call, view=None):
    arguments = {"task": "inspect retained source inventory", "target": ".", "max_files": 1, "max_bytes": 1048576}
    if view:
        arguments["view"] = view
    packet = call("brief", arguments)
    if not packet.get("view_id") or not packet.get("index_id"):
        raise ValueError("No pinned source/index available for the control")
    proof = call("explain", {"report_id": packet["index_id"], "ref": "/files", "max_bytes": 1048576})
    files = proof.get("entry")
    if not isinstance(files, dict) or not files:
        raise ValueError("Complete source inventory was unavailable or exceeded its response budget")
    for path, row in files.items():
        if not isinstance(row, dict) or row.get("path") != path or not isinstance(row.get("digest"), str):
            raise ValueError("Malformed source inventory")
    return packet["view_id"], files


def manifest(files):
    entries = [[path, row["digest"], row.get("bytes")] for path, row in sorted(files.items())]
    raw = json.dumps(entries, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    return {"sha256": hashlib.sha256(raw).hexdigest(), "files": len(entries), "entries": entries}


def source_profile(status, view):
    state = status.get("source_sync") or {}
    profile = state.get("profile") or {}
    if state.get("view_id") != view or profile.get("language") != "rust" or "profile_views" in profile:
        raise ValueError("Controls require a freshly synced single Rust profile; source/profile provenance is unavailable or differs")
    return profile


def checked_source(root, relative, digest):
    path = PurePosixPath(relative)
    if path.is_absolute() or ".." in path.parts or not path.parts or "\\" in relative or ".git" in path.parts:
        raise ValueError(f"Unsafe source path: {relative}")
    candidate = root.joinpath(*path.parts)
    current = root
    for part in path.parts:
        current = current / part
        if current.is_symlink():
            raise ValueError(f"Source symlink cannot be reconstructed as retained bytes: {relative}")
    if not candidate.is_file() or file_digest(candidate) != digest:
        raise ValueError(f"Working source differs from the retained capture: {relative}. Sync before running controls.")
    return candidate


def clean_clone(root, files, output):
    seed, clone = output / "seed", output / "clean" / "checkout"
    seed.mkdir()
    for path, row in sorted(files.items()):
        source = checked_source(root, path, row["digest"])
        destination = seed / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    git = shutil.which("git")
    if not git:
        raise FileNotFoundError("Git is required for the clean-clone control")
    empty_config, hooks = output / "git-config", output / "empty-hooks"
    empty_config.write_text("")
    hooks.mkdir()
    environment = {**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": str(empty_config),
                   "GIT_TERMINAL_PROMPT": "0"}

    def run(*arguments, cwd=seed):
        command = [git, "-c", f"core.hooksPath={hooks}", "-c", "commit.gpgsign=false", *arguments]
        result = subprocess.run(command, cwd=cwd, env=environment, capture_output=True, text=True, timeout=60)
        if result.returncode:
            raise RuntimeError(f"Git control setup failed: {result.stderr.strip()[:1000]}")

    run("init", "--quiet")
    run("add", "--force", "--all")
    run("-c", "user.name=Briefcase evaluation", "-c", "user.email=evaluation@localhost.invalid",
        "commit", "--quiet", "-m", "Controlled source snapshot")
    clone.parent.mkdir()
    run("clone", "--no-local", "--no-hardlinks", "--quiet", str(seed), str(clone), cwd=output)
    return clone


def run_evaluation(binary, root, db, view, scenarios, output, *, timeout, budget, harness, ablations):
    command = [sys.executable, "-B", str(Path(__file__).with_name("task-evaluate.py")),
               "--binary", str(binary), "--root", str(root), "--db", str(db), "--view", view,
               "--scenarios", str(scenarios), "--output", str(output), "--timeout", str(timeout),
               "--manifest-bytes", str(budget), "--harness", harness, "--diagnostics", "--ablations", ablations,
               "--verify-sources", "--budget-sweep", "3000,4000,6000,8000,12000"]
    print(f"Evaluating {output.parent.name}/{output.stem}", file=sys.stderr, flush=True)
    # Each RPC has its own evaluator deadline; no shell or background watcher runs.
    result = subprocess.run(command, capture_output=True, text=True)
    output.with_suffix(".stderr.txt").write_text(result.stderr)
    if not output.is_file():
        raise RuntimeError(f"Evaluator produced no report: {result.stderr.strip()[:1000]}")
    report = json.loads(output.read_text())
    if result.returncode not in (0, 1) or report.get("errors") or any(
            row.get("status") == "error" for rows in report.get("facet_ablations", {}).values() for row in rows):
        raise RuntimeError(f"Evaluation failed; inspect {output}")
    return report


def compare(normal, clean):
    control = {row["id"]: row for row in clean["scenarios"]}
    if set(control) != {row["id"] for row in normal["scenarios"]}:
        raise ValueError("Control scenario IDs differ")
    if normal["inputs"]["scenario_sha256"] != clean["inputs"]["scenario_sha256"]:
        raise ValueError("Scenario labels changed between retained and clean conditions")
    return [{"id": row["id"], "retained_recall": row["required_path_recall"],
             "clean_recall": control[row["id"]]["required_path_recall"],
             "retained_only_required_paths": sorted(set(control[row["id"]]["missing_paths"]) - set(row["missing_paths"])),
             "clean_only_required_paths": sorted(set(row["missing_paths"]) - set(control[row["id"]]["missing_paths"]))}
            for row in normal["scenarios"]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/briefcase"))
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--db", type=Path, required=True, help="Existing, freshly synced baseline database")
    parser.add_argument("--view", help="Optional retained source view")
    parser.add_argument("--original", type=Path, default=Path("evaluation/task-scenarios.json"))
    parser.add_argument("--heldout", type=Path, default=Path("evaluation/heldout-scenarios.json"))
    parser.add_argument("--output", type=Path, required=True, help="New results directory outside the measured source tree")
    parser.add_argument("--timeout", type=float, default=60, help="Per evaluation RPC seconds")
    parser.add_argument("--sync-timeout", type=int, default=600)
    parser.add_argument("--manifest-bytes", type=int, default=8000)
    parser.add_argument("--harness", choices=tuple(EVALUATOR.FRAMING), default="pi")
    parser.add_argument("--sync-arg", action="append", default=[], help="Additional profile option; use --sync-arg=--option")
    parser.add_argument("--control-ablations", action="store_true", help="Also ablate every facet in the clean clone")
    args = parser.parse_args()
    root, binary, db, output = (path.resolve() for path in (args.root, args.binary, args.db, args.output))
    if output.is_relative_to(root):
        parser.error("Control output must be outside the source tree")
    if output.exists() and any(output.iterdir()):
        parser.error("Use a new or empty output directory; existing results are never overwritten")
    output.mkdir(parents=True, exist_ok=True)
    report = {"method": "task-context-controls-v2", "status": "blocked", "started_at_unix": time.time(),
              "inputs": {"binary": str(binary), "root": str(root), "database": str(db),
                         "manifest_bytes": args.manifest_bytes, "harness": args.harness,
                         "verify_sources": True, "budget_sweep": [3000,4000,6000,8000,12000]},
              "limitations": ["This control isolates source-only context from retained Git/session evidence; it does not prove generalization from a small labelled corpus.",
                              "Native outcomes and cold-start timing can vary. Source equality is verified separately from producer completeness."]}
    try:
        if not binary.is_file():
            raise FileNotFoundError(f"Build the executable before controls: {binary}")
        if not db.is_file():
            raise FileNotFoundError(f"Sync the baseline database before controls: {db}")
        forbidden = ("--db", "--project-root", "--git", "--sessions", "--check", "--timeout")
        if any(argument.split("=")[0] in forbidden for argument in args.sync_arg):
            raise ValueError("Control sync arguments may select analysis options, not database/root/history/session domains")
        report["inputs"]["binary_sha256"] = file_digest(binary)
        scenarios = {name: path.resolve() for name, path in [("original", args.original), ("heldout", args.heldout)]}
        for path in scenarios.values():
            if not json.loads(path.read_text()).get("scenarios"):
                raise ValueError(f"No authored scenarios: {path}")
        normal_call = lambda operation, arguments: EVALUATOR.execute(binary, root, operation, arguments, args.timeout, db=db)
        initial_status = normal_call("status", {"summary": True, "max_bytes": 1048576})
        if initial_status.get("pending_or_interrupted"):
            raise ValueError("Finish the pending source capture before controls")
        view, files = inventory(normal_call, args.view)
        profile = source_profile(initial_status, view)
        source_manifest = manifest(files)
        for path, row in files.items():
            checked_source(root, path, row["digest"])
        write_json(output / "source-manifest.json", source_manifest)
        report["retained"] = {"view_id": view, "source_manifest_sha256": source_manifest["sha256"], "profile": profile}
        clone = clean_clone(root, files, output)
        control_db = output / "clean" / "briefcase.db"
        command = [str(binary), "--db", str(control_db), "--project-root", str(clone), "--json", "sync", "--project",
                   "--language", "rust", "--timeout", str(args.sync_timeout), *args.sync_arg]
        result = subprocess.run(command, cwd=clone, capture_output=True, text=True, timeout=args.sync_timeout + 30)
        (output / "clean" / "sync.stderr.txt").write_text(result.stderr)
        synced = json.loads(result.stdout)
        write_json(output / "clean" / "sync.json", synced)
        if result.returncode not in (0, 1) or not synced.get("view_id"):
            raise RuntimeError("Control source sync produced no usable retained view; inspect clean/sync.json")
        if synced.get("profile") != profile:
            raise ValueError("Control analysis profile differs from baseline; match its options with --sync-arg before comparing")
        control_call = lambda operation, arguments: EVALUATOR.execute(binary, clone, operation, arguments, args.timeout, db=control_db)
        control_view, control_files = inventory(control_call, synced["view_id"])
        if manifest(control_files)["sha256"] != source_manifest["sha256"]:
            raise ValueError("Clean clone captured different source bytes; no comparison is valid")
        status = control_call("status", {"summary": True, "max_bytes": 1048576})
        if status.get("inventory", {}).get("history_commits_reachable") != 0 or status.get("inventory", {}).get("session_publications") != 0:
            raise ValueError("Clean control contains retained history or sessions")
        report["clean"] = {"root": str(clone), "database": str(control_db), "view_id": control_view,
                           "source_manifest_sha256": source_manifest["sha256"], "sync_state": synced.get("sync"),
                           "ready": synced.get("ready"), "static_calls": synced.get("static_calls"), "profile": synced.get("profile"),
                           "retained_history_commits": 0, "session_publications": 0}
        comparisons = {}
        for name, source in scenarios.items():
            normal_output = output / "retained" / f"{name}.json"
            normal_output.parent.mkdir(exist_ok=True)
            normal = run_evaluation(binary, root, db, view, source, normal_output, timeout=args.timeout,
                                    budget=args.manifest_bytes, harness=args.harness, ablations="all")
            clean = run_evaluation(binary, clone, control_db, control_view, source, output / "clean" / f"{name}.json",
                                   timeout=args.timeout, budget=args.manifest_bytes, harness=args.harness,
                                   ablations="all" if args.control_ablations else "")
            comparisons[name] = compare(normal, clean)
        final_status = normal_call("status", {"summary": True, "max_bytes": 1048576})
        for key in ("current_view", "ledger_watermark", "retracted_count", "pending_or_interrupted"):
            if initial_status.get(key) != final_status.get(key):
                raise ValueError(f"Baseline database changed during controls: {key}")
        if source_profile(final_status, view) != profile:
            raise ValueError("Baseline analysis profile changed during controls")
        for path, row in files.items():
            checked_source(root, path, row["digest"])
        if file_digest(binary) != report["inputs"]["binary_sha256"]:
            raise ValueError("Executable changed during controls")
        report["comparisons"] = comparisons
        report["retained"]["ledger_watermark"] = initial_status.get("ledger_watermark")
        report["status"] = "measured"
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        report["error"] = str(error)
    report["finished_at_unix"] = time.time()
    write_json(output / "control-report.json", report)
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["status"] == "measured" else 2


if __name__ == "__main__":
    raise SystemExit(main())
