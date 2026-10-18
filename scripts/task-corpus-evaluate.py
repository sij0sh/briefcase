#!/usr/bin/env python3
"""Blind harness evaluation over the generated task corpus.

Example: python3 scripts/task-corpus-evaluate.py --binary target/release/briefcase \
  --output corpus-run --variants 4 [--harness-command 'my-agent --task "$BRIEFCASE_TASK"']

The reference driver sees only the task text. Answer keys stay outside every
repository and are read by the evaluator only after the driver stops. The
funnel (candidate -> selected -> digest-verified source) is reported per
required path; outcomes are verified only by running the fixture's check after
an external harness edits a copy. Without --harness-command outcomes are
"not_run", never inferred from retrieved evidence. Errors are failures.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time

from run_evidence import RunEvidence, atomic_json
from task_corpus import FAMILIES, corpus
from task_investigation import Investigation

GIT = ["git", "-c", "user.name=Corpus", "-c", "user.email=corpus@local.invalid", "-c", "commit.gpgsign=false",
       "-c", "core.hooksPath=/dev/null"]


def materialize(directory, files):
    for path, text in files.items():
        (directory / path).parent.mkdir(parents=True, exist_ok=True)
        (directory / path).write_text(text)
    for command in (["git", "init", "-q"], ["git", "add", "."], GIT + ["commit", "-qm", "fixture"]):
        subprocess.run(command, cwd=directory, check=True, capture_output=True)


def check(script, repository, timeout=60):
    result = subprocess.run([sys.executable, "-B", "-I", str(script), str(repository)], capture_output=True, text=True, timeout=timeout)
    return {"passed": result.returncode == 0, "stderr": result.stderr.strip()[-400:]}


def evaluate(fixture, args, output):
    started = time.monotonic()
    fid, key = fixture["id"], fixture["answer_key"]
    repository, keys = output / "repos" / fid, output / "keys"
    materialize(repository, fixture["files"])
    keys.mkdir(parents=True, exist_ok=True)
    script = keys / f"{fid}.check.py"
    script.write_text(key["check"])
    # The outcome oracle must discriminate before it can verify anything.
    oracle = output / "oracle" / fid
    shutil.copytree(repository, oracle, ignore=shutil.ignore_patterns(".git"))
    for path, text in key["patch"].items():
        (oracle / path).write_text(text)
    before, after = check(script, repository), check(script, oracle)
    record = {"id": fid, "family": fixture["family"], "split": fixture["split"], "task": fixture["task"],
              "oracle": {"valid": not before["passed"] and after["passed"], "before": before, "after_reference": after}}
    db = output / "db" / f"{fid}.db"
    db.parent.mkdir(parents=True, exist_ok=True)
    run = RunEvidence(output / "raw" / fid)
    captured = run.run([str(args.binary), "--db", str(db), "--project-root", str(repository), "sync"], cwd=repository, timeout=args.timeout)
    if captured.returncode:
        raise RuntimeError("sync failed: " + (captured.stderr or captured.stdout)[:400])
    usage = {"calls": 0, "response_bytes": 0}

    def call(operation, arguments):
        result = run.run([str(args.binary), "--db", str(db), "--project-root", str(repository), "--json", "call", operation,
                          "--args", json.dumps(arguments, ensure_ascii=False)], cwd=repository, timeout=args.timeout)
        usage["calls"] += 1; usage["response_bytes"] += len(result.stdout.encode())
        if result.returncode:
            raise RuntimeError((result.stderr or result.stdout).strip()[:400])
        return json.loads(result.stdout)

    # Blind: the driver receives the task only. No target, duty or path hint.
    workflow = Investigation(call, output / "investigations" / fid)
    workflow.start({"task": fixture["task"]}, args.max_bytes)
    steps = 0
    while steps < args.steps and workflow.step() is not None:
        steps += 1
    driver_calls = usage["calls"]
    record["driver"] = {"steps": steps, "calls": driver_calls, "response_bytes": usage["response_bytes"],
                        "pending_actions": len(workflow.state["queue"]), "context_readiness": workflow.state["context_readiness"],
                        "seconds": round(time.monotonic() - started, 3)}
    # Scoring only: enumerate the case's candidates after the driver stopped.
    candidates, offset = {}, 0
    while offset is not None:
        page = call("page", {"report_id": workflow.state["case_id"], "section": "/candidates", "offset": offset,
                             "limit": 1000, "max_bytes": 1048576})
        candidates.update({row["path"]: row for row in page["items"]})
        offset = page["page"].get("next_offset")
    sources = workflow.state["sources"]
    record["required"] = [{"path": path, "candidate": path in candidates, "rank": candidates.get(path, {}).get("rank"),
                           "selected": bool(candidates.get(path, {}).get("selected")),
                           "verified_source": sources.get(path, {}).get("state") == "digest_verified"}
                          for path in key["required_paths"]]
    record["outcome"] = {"state": "not_run"}
    if args.harness_command:
        work = output / "work" / fid
        shutil.copytree(repository, work)
        env = {**os.environ, "BRIEFCASE_TASK": fixture["task"], "BRIEFCASE_DB": str(db),
               "BRIEFCASE_INVESTIGATION": str(output / "investigations" / fid)}
        harness = subprocess.run(shlex.split(args.harness_command), cwd=work, env=env, capture_output=True, text=True, timeout=args.harness_timeout)
        verified = check(script, work)
        record["outcome"] = {"state": "verified_pass" if verified["passed"] else "verified_fail",
                             "harness_exit": harness.returncode, "check": verified}
    return record


def summarize(records):
    groups = {}
    for record in records:
        for name in ("all", "split:" + record["split"], "family:" + record["family"]):
            groups.setdefault(name, []).append(record)
    summary = {}
    for name, group in sorted(groups.items()):
        scored = [r for r in group if "error" not in r]
        required = [row for r in scored for row in r["required"]]
        outcomes = [r["outcome"]["state"] for r in scored]
        summary[name] = {
            "fixtures": len(group), "errors": len(group) - len(scored),
            "oracle_valid": sum(r["oracle"]["valid"] for r in scored), "required_paths": len(required),
            # Errored fixtures stay in the denominator: a crash is not a skipped case.
            "fixtures_all_required_verified": sum(all(x["verified_source"] for x in r["required"]) for r in scored),
            "candidate_recall": sum(x["candidate"] for x in required), "selected_recall": sum(x["selected"] for x in required),
            "verified_source_recall": sum(x["verified_source"] for x in required),
            "mean_driver_calls": round(sum(r["driver"]["calls"] for r in scored) / len(scored), 1) if scored else None,
            "outcomes": {state: outcomes.count(state) for state in sorted(set(outcomes))}}
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/briefcase"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--variants", type=int, default=4)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--families", default=",".join(FAMILIES))
    parser.add_argument("--split", choices=("all", "dev", "held_out"), default="all")
    parser.add_argument("--steps", type=int, default=400, help="Driver actions per fixture; remaining work is reported")
    parser.add_argument("--max-bytes", type=int, default=8000)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--harness-command", help="Run in a copy of each repository; outcome is the fixture check afterwards")
    parser.add_argument("--harness-timeout", type=float, default=900)
    args = parser.parse_args()
    args.binary, output = args.binary.resolve(), args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Use a new output directory")
    families = [f for f in args.families.split(",") if f]
    if not families or any(f not in FAMILIES for f in families):
        parser.error("Unknown family; choose from " + ",".join(FAMILIES))
    fixtures = [f for f in corpus(args.variants, args.seed, families) if args.split in ("all", f["split"])]
    records = []
    for fixture in fixtures:
        try:
            records.append(evaluate(fixture, args, output))
        except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
            records.append({"id": fixture["id"], "family": fixture["family"], "split": fixture["split"], "error": str(error)[:800]})
        print(json.dumps({k: records[-1].get(k) for k in ("id", "split", "error")}), file=sys.stderr)
    report = {"schema_version": "1.0", "kind": "task_corpus_evaluation", "seed": args.seed, "variants": args.variants,
              "binary": str(args.binary), "driver": "task_investigation.Investigation (blind: task text only)",
              "interpretation": "Recall counts are evidence access, not task success. Outcomes require --harness-command and the fixture check.",
              "summary": summarize(records), "fixtures": records}
    atomic_json(output / "report.json", report)
    print(json.dumps({"report": str(output / "report.json"), "summary": report["summary"]["all"]}))
    return 1 if any("error" in r for r in records) else 0


if __name__ == "__main__":
    raise SystemExit(main())
