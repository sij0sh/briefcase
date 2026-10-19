#!/usr/bin/env python3
"""Phase 4 outcome trials under the frozen protocol (evaluation/outcome-protocol.json).

  validate   check every repair fixture is meaningful and reversible
  run        run matched conditions on one task set and score them
  summarize  merge result files, compute paired contrasts and the exit gate

Examples:
  python3 scripts/task-outcomes.py validate --work ../work/fixtures
  python3 scripts/task-outcomes.py run --set repairs --work ../work/repairs --output repairs.json
  python3 scripts/task-outcomes.py run --set t3 --root ../t3/scratch --db ../t3/db/briefcase.db \
      --work ../work/t3 --output t3.json
  python3 scripts/task-outcomes.py summarize t3.json repairs.json --output summary.json

The agent tier runs only with --agent-command (paid model calls must be
authorized explicitly). The command reads one JSON bundle on stdin, runs in a
writable copy of the repair project (or an empty directory for T3), and
prints one JSON object.
"""
import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import task_outcomes as outcomes

ROOT = Path(__file__).resolve().parents[1]


def briefcase_call(evaluator, binary, root, db, timeout):
    raw = lambda operation, arguments: evaluator.execute(binary, root, operation, arguments, timeout, db=db)
    state = raw("status", {"summary": True, "max_bytes": 1048576})
    if state.get("pending_or_interrupted") is True:
        raise ValueError("Source capture is pending; finish it before outcome trials")
    return evaluator.PinnedCall(raw, state.get("current_view")), state.get("current_view")


def prepare_fixture(binary, fixture_dir, fixture, work):
    """Before-state source root, an rg root with the transcripts, and one scanned database."""
    work = Path(work)
    root = outcomes.materialize(fixture_dir, fixture, work / "root")
    rg_sessions = outcomes.materialize(fixture_dir, fixture, work / "rg-sessions", sessions=True) if fixture.get("sessions") else None
    db = work / "briefcase.db"
    for stale in work.glob("briefcase.db*"):
        stale.unlink()
    base = [str(binary), "--db", str(db), "--project-root", str(root), "--json"]
    scan = [*base, "scan", "--language", fixture.get("language", "python")]
    if fixture.get("source_roots"):
        scan += ["--python-source-root", fixture["source_roots"]]
    if fixture.get("policy"):
        scan += ["--policy", str(root / fixture["policy"])]
    run(scan + [str(root)], root)
    for relative in fixture.get("sessions", []):
        transcript = work / "sessions" / Path(relative).name
        transcript.parent.mkdir(parents=True, exist_ok=True)
        transcript.write_text(outcomes.session_text(fixture_dir, relative, root))
        run([*base, "sessions", "import", "--profile", "pi-v1", "--repository-root", str(root), str(transcript)], root)
    return root, rg_sessions, db


def run(command, cwd):
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=600)
    if result.returncode:
        raise RuntimeError(f"{' '.join(command[:6])}...: {(result.stderr or result.stdout).strip()[:1000]}")
    return result.stdout


def trial_rows(task, conditions, protocol, evaluator, *, call, root, rg_sessions_root, work, repeats, set_name):
    rows = []
    budgets = protocol["budgets"]
    for condition in conditions:
        spec = protocol["conditions"][condition]
        sessions = spec["sessions"]
        if sessions and not task.get("sessions"):
            continue
        for repeat in range(repeats):
            directory = Path(work) / "runs" / task["id"] / condition / str(repeat)
            if spec["kind"] == "rg_read":
                delivered = outcomes.deliver_rg(task, rg_sessions_root if sessions else root, budgets)
                audited = []
            else:
                delivered = outcomes.deliver_briefcase(call, task, directory,
                                                       outcomes.condition_exclusions(condition, protocol, evaluator), budgets)
                audited = delivered["retained_findings"]
            scored = outcomes.score_claims(task["claims"], delivered, sessions, root)
            everything = outcomes.score_claims(task["claims"], delivered, True, root)
            violations = outcomes.critical_audit(audited)
            kinds = {}
            for finding in delivered["findings"]:
                kinds[finding.get("kind")] = kinds.get(finding.get("kind"), 0) + 1
            rows.append({"set": set_name, "scenario": task["id"], "family": task.get("family"), "condition": condition,
                         "repeat": repeat, "sessions_task": bool(task.get("sessions")),
                         "supportability": scored["supportability"], "supported": scored["supported"],
                         "claims": scored["claims"], "misses": scored["misses"],
                         "supportability_all": everything["supportability"],
                         "unsupported_critical_claims": len(violations), "critical_violations": violations,
                         "findings_delivered": kinds, "findings_status": delivered.get("findings_status"),
                         "delivered_files": sorted(delivered["files"]), "cost": delivered["cost"],
                         "_delivered": delivered})
    return rows


def determinism(rows):
    groups = {}
    for row in rows:
        groups.setdefault((row["scenario"], row["condition"]), []).append(row)
    unstable = []
    for (scenario, condition), group in groups.items():
        signatures = {json.dumps([r["supported"], r["delivered_files"]]) for r in group}
        if len(signatures) > 1:
            unstable.append({"scenario": scenario, "condition": condition})
    return unstable


def agent_rows(task, conditions, protocol, args, delivered_by_condition, fixture_dir=None, fixture=None):
    rows = []
    for condition in conditions:
        delivered = delivered_by_condition.get(condition)
        if delivered is None:
            continue
        for repeat in range(args.agent_repeats):
            with tempfile.TemporaryDirectory(dir=args.work) as scratch:
                scratch = Path(scratch)
                if fixture is not None:
                    workdir = outcomes.materialize(fixture_dir, fixture, scratch / "work")
                    pristine = outcomes.materialize(fixture_dir, fixture, scratch / "pristine")
                else:
                    workdir, pristine = scratch, None
                bundle = outcomes.agent_bundle(task, condition, delivered)
                answer = outcomes.run_agent(args.agent_command, bundle, workdir, args.agent_timeout)
                row = {"set": args.set, "scenario": task["id"], "condition": condition, "repeat": repeat,
                       "status": answer["status"], "seconds": answer["seconds"]}
                if answer["status"] == "ok":
                    row.update(outcomes.score_agent(task, answer["answer"], delivered, task.get("forbidden_patterns", []),
                                                    workdir=workdir if fixture else None, pristine=pristine,
                                                    fixture=fixture))
                else:
                    row["error"] = answer["error"]
                rows.append(row)
    return rows


def command_validate(args):
    reports = []
    for fixture_dir, fixture in outcomes.load_fixtures(args.fixtures):
        reports.append(outcomes.validate_fixture(fixture_dir, fixture, Path(args.work) / fixture["id"]))
    print(json.dumps({"fixtures": reports, "ok": all(r["ok"] for r in reports)}, indent=2))
    return 0 if reports and all(r["ok"] for r in reports) else 1


def command_run(args):
    protocol = outcomes.load_protocol(args.protocol)
    evaluator = outcomes.load_evaluator()
    binary = args.binary.resolve()
    conditions = [c for c in (args.conditions.split(",") if args.conditions else protocol["conditions"])]
    unknown = set(conditions) - set(protocol["conditions"])
    if unknown:
        raise SystemExit(f"Unknown conditions: {sorted(unknown)}")
    repeats = args.repeats or protocol["tiers"]["deterministic"]["repeats"]
    work = Path(args.work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    rows, agents, views = [], [], {}
    if args.set == "t3":
        claims = {s["id"]: s for s in json.loads(args.claims.read_text())["scenarios"]}
        scenarios = json.loads(args.scenarios.read_text())["scenarios"]
        root = args.root.resolve()
        call, view = briefcase_call(evaluator, binary, root, args.db.resolve() if args.db else None, args.timeout)
        views["t3"] = view
        for scenario in scenarios:
            if args.only and scenario["id"] not in args.only.split(","):
                continue
            task = {**{k: scenario[k] for k in ("id", "task", "target", "duty") if k in scenario},
                    "family": claims[scenario["id"]]["family"], "claims": claims[scenario["id"]]["claims"],
                    "forbidden_patterns": claims[scenario["id"]]["forbidden_patterns"]}
            produced = trial_rows(task, conditions, protocol, evaluator, call=call, root=root, rg_sessions_root=None,
                                  work=work, repeats=repeats, set_name="t3")
            rows += produced
            if args.agent_command:
                agents += agent_rows(task, conditions, protocol, args,
                                     {r["condition"]: r["_delivered"] for r in produced if r["repeat"] == 0})
            print(f"{scenario['id']}: " + ", ".join(f"{r['condition']}={r['supportability']}" for r in produced if r["repeat"] == 0),
                  file=sys.stderr)
    else:
        for fixture_dir, fixture in outcomes.load_fixtures(args.fixtures):
            if args.only and fixture["id"] not in args.only.split(","):
                continue
            fixture_work = work / fixture["id"]
            root, rg_sessions, db = prepare_fixture(binary, fixture_dir, fixture, fixture_work)
            call, view = briefcase_call(evaluator, binary, root, db, args.timeout)
            views[fixture["id"]] = view
            task = {k: fixture[k] for k in ("id", "task", "target", "family", "claims", "forbidden_patterns", "sessions")}
            produced = trial_rows(task, conditions, protocol, evaluator, call=call, root=root,
                                  rg_sessions_root=rg_sessions, work=fixture_work, repeats=repeats, set_name="repairs")
            rows += produced
            if args.agent_command:
                agents += agent_rows(task, conditions, protocol, args,
                                     {r["condition"]: r["_delivered"] for r in produced if r["repeat"] == 0},
                                     fixture_dir=fixture_dir, fixture=fixture)
            print(f"{fixture['id']}: " + ", ".join(f"{r['condition']}={r['supportability_all']}" for r in produced if r["repeat"] == 0),
                  file=sys.stderr)
    for row in rows:
        row.pop("_delivered", None)
    result = {"schema_version": "1.0", "kind": "outcome_results", "protocol": protocol["revision"], "set": args.set,
              "binary": str(binary), "views": views, "conditions": conditions, "repeats": repeats,
              "nondeterministic": determinism(rows), "rows": rows, "agent_rows": agents}
    result["summary"] = summarize([result], protocol)
    text = json.dumps(result, indent=2)
    if args.output:
        args.output.write_text(text + "\n")
    print(json.dumps(result["summary"], indent=2))
    return 0


def condition_table(rows, metric):
    table = {}
    for row in rows:
        entry = table.setdefault(row["condition"], {"rows": 0, "values": [], "critical": 0,
                                                     "calls": 0, "bytes": 0, "seconds": 0.0, "files": 0})
        entry["rows"] += 1
        if row.get(metric) is not None:
            entry["values"].append(row[metric])
        entry["critical"] += row.get("unsupported_critical_claims", 0)
        for name in ("calls", "bytes", "seconds", "files"):
            entry[name] += row["cost"][name]
    return {name: {"mean_" + metric: round(sum(e["values"]) / len(e["values"]), 4) if e["values"] else None,
                   "unsupported_critical_claims": e["critical"],
                   **{"mean_" + k: round(e[k] / e["rows"], 2) for k in ("calls", "bytes", "seconds", "files")}}
            for name, e in table.items()}


def task_rows(rows, tasks):
    selected = []
    for row in rows:
        for spec in tasks:
            name, _, qualifier = spec.partition(":")
            if row["set"] == name and (qualifier != "sessions" or row["sessions_task"]):
                selected.append(row)
                break
    return selected


def summarize(results, protocol):
    rows = [row for result in results for row in result["rows"]]
    agents = [row for result in results for row in result.get("agent_rows", [])]
    statistics = protocol["statistics"]
    params = {"resamples": 10000, "seed": 20261018}
    contrasts = {}
    primary = protocol["contrasts"]["primary"]
    selected = task_rows(rows, primary["tasks"])
    contrasts["primary"] = outcomes.contrast(selected, primary["treatment"], primary["control"], params)
    contrasts["secondary"] = []
    for spec in protocol["contrasts"]["secondary"]:
        if spec["tier"] == "agent":
            pool = task_rows(agents, spec["tasks"]) if agents else []
            for metric in spec["metric"].split("+"):
                if pool:
                    contrasts["secondary"].append(outcomes.contrast(
                        [{**r, metric: float(r.get(metric, False)), "cost": {}} for r in pool if r.get("status") == "ok"],
                        spec["treatment"], spec["control"], params, metric))
                else:
                    contrasts["secondary"].append({"treatment": spec["treatment"], "control": spec["control"],
                                                   "metric": metric, "status": "not_run",
                                                   "reason": "agent tier requires an authorized --agent-command"})
            continue
        metric = "supportability_all" if any(t.endswith(":sessions") for t in spec["tasks"]) else "supportability"
        contrasts["secondary"].append(outcomes.contrast(task_rows(rows, spec["tasks"]), spec["treatment"],
                                                        spec["control"], params, metric))
    misses = {}
    for row in rows:
        if protocol["conditions"][row["condition"]]["kind"] != "briefcase" or row["repeat"]:
            continue
        for claim, miss in row["misses"].items():
            if miss:
                misses.setdefault(row["condition"], {}).setdefault(miss["class"], []).append(f"{row['scenario']}:{claim}")
    return {"protocol": protocol["revision"], "statistics": statistics["method"],
            "sets": sorted({row["set"] for row in rows}),
            "by_condition": condition_table(rows, "supportability"),
            "by_condition_all_claims": condition_table(task_rows(rows, ["repairs:sessions"]), "supportability_all"),
            "contrasts": contrasts, "gate": outcomes.gate(contrasts["primary"], protocol),
            "gate_scope": "deterministic tier only" + ("" if agents else "; agent tier not run"),
            "miss_classes": {c: {k: len(v) for k, v in m.items()} for c, m in misses.items()},
            "miss_detail": misses,
            "nondeterministic": [n for result in results for n in result.get("nondeterministic", [])]}


def command_summarize(args):
    protocol = outcomes.load_protocol(args.protocol)
    results = [json.loads(Path(path).read_text()) for path in args.results]
    revisions = {r["protocol"] for r in results}
    if revisions != {protocol["revision"]}:
        raise SystemExit(f"Results come from protocol revisions {sorted(revisions)}, not {protocol['revision']}")
    summary = summarize(results, protocol)
    text = json.dumps(summary, indent=2)
    if args.output:
        args.output.write_text(text + "\n")
    print(text)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--protocol", type=Path, default=outcomes.PROTOCOL_PATH)
    commands = parser.add_subparsers(dest="command", required=True)
    validate = commands.add_parser("validate")
    validate.add_argument("--fixtures", type=Path, default=ROOT / "evaluation" / "repairs")
    validate.add_argument("--work", type=Path, required=True)
    trial = commands.add_parser("run")
    trial.add_argument("--set", choices=("t3", "repairs"), required=True)
    trial.add_argument("--binary", type=Path, default=ROOT / "target" / "release" / "briefcase")
    trial.add_argument("--work", type=Path, required=True)
    trial.add_argument("--output", type=Path)
    trial.add_argument("--conditions", default="")
    trial.add_argument("--only", default="", help="Comma-separated scenario or fixture ids")
    trial.add_argument("--repeats", type=int, default=0)
    trial.add_argument("--timeout", type=float, default=120)
    trial.add_argument("--fixtures", type=Path, default=ROOT / "evaluation" / "repairs")
    trial.add_argument("--root", type=Path)
    trial.add_argument("--db", type=Path)
    trial.add_argument("--scenarios", type=Path, default=ROOT / ".agents" / "artifacts" / "t3code-scenarios.json")
    trial.add_argument("--claims", type=Path, default=ROOT / "evaluation" / "t3-outcome-claims.json")
    trial.add_argument("--agent-command", help="Authorized agent command (paid model calls); omit for the deterministic tier only")
    trial.add_argument("--agent-repeats", type=int, default=0)
    trial.add_argument("--agent-timeout", type=float, default=900)
    summary = commands.add_parser("summarize")
    summary.add_argument("results", nargs="+")
    summary.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    if args.command == "run":
        if args.set == "t3" and not args.root:
            parser.error("--set t3 needs --root (and usually --db)")
        if args.agent_command and not args.agent_repeats:
            args.agent_repeats = outcomes.load_protocol(args.protocol)["tiers"]["agent"]["repeats"]
    return {"validate": command_validate, "run": command_run, "summarize": command_summarize}[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
