"""Phase 4 outcome trials over matched conditions (evaluation/outcome-protocol.json).

Deterministic tier: reconstruct exactly what each condition delivered under the
same budgets and score claim supportability against scorer-only witnesses,
audit delivered findings for unsupported critical claims, and record cost.
Agent tier: an external command receives only the delivered context (and for
repair fixtures a writable project copy) and is scored on citations, forbidden
claims, repair tests and edit scope. Scorer-only files never reach a condition.
"""
import hashlib
import importlib.util
import json
import os
import random
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

from task_baseline import rg_read_baseline
from task_investigation import run_bounded_investigation

ROOT = Path(__file__).resolve().parents[1]
PROTOCOL_PATH = ROOT / "evaluation" / "outcome-protocol.json"
RG_READ_BYTES = 65536
SESSION_DIR = "agent-sessions"


def load_evaluator():
    spec = importlib.util.spec_from_file_location("task_evaluate", Path(__file__).with_name("task-evaluate.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_protocol(path=PROTOCOL_PATH):
    protocol = json.loads(Path(path).read_text())
    if protocol.get("kind") != "outcome_protocol":
        raise ValueError("Not an outcome protocol")
    return protocol


def condition_exclusions(name, protocol, evaluator):
    spec = protocol["conditions"][name]
    if spec["kind"] != "briefcase":
        return None
    excluded = spec["exclude_facets"]
    if isinstance(excluded, str):
        return list(evaluator.lexical_only_exclusions())
    unknown = set(excluded) - set(evaluator.FACETS)
    if unknown:
        raise ValueError(f"Protocol excludes unknown facets: {sorted(unknown)}")
    return list(excluded)


# --- Repair fixtures -------------------------------------------------------

def load_fixtures(directory):
    fixtures = []
    for path in sorted(Path(directory).glob("*/fixture.json")):
        fixture = json.loads(path.read_text())
        if fixture.get("kind") != "repair_fixture":
            raise ValueError(f"{path}: not a repair fixture")
        fixtures.append((path.parent, fixture))
    return fixtures


def apply_patch(root, patch, reverse=False):
    patch = Path(patch)
    if not patch.read_text().strip():
        return
    command = ["patch", "-p1", "-s", "--no-backup-if-mismatch", "-d", str(root), "-i", str(patch)]
    if reverse:
        command.insert(1, "-R")
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"{patch.name} did not apply: {(result.stdout + result.stderr).strip()[:500]}")


def materialize(fixture_dir, fixture, dest, state="before", sessions=False):
    """Copy only the fixture's project tree; scorer-only files stay behind."""
    dest = Path(dest)
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(Path(fixture_dir) / "project", dest)
    if state in ("reference", "naive"):
        apply_patch(dest, Path(fixture_dir) / f"{state}.patch")
    elif state != "before":
        raise ValueError(f"Unknown fixture state {state!r}")
    if sessions:
        for relative in fixture.get("sessions", []):
            target = dest / SESSION_DIR / Path(relative).name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(session_text(fixture_dir, relative, dest))
    return dest


def session_text(fixture_dir, relative, root):
    return (Path(fixture_dir) / relative).read_text().replace("{{ROOT}}", str(Path(root).resolve()))


def tree_digest(root):
    hasher = hashlib.sha256()
    for path in sorted(Path(root).rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts:
            hasher.update(str(path.relative_to(root)).encode() + b"\0" + path.read_bytes() + b"\0")
    return "sha256:" + hasher.hexdigest()


def run_tests(root, ids, fixture, timeout=120):
    """Each test id's status: pass, fail (assertions only) or error (anything else)."""
    root = Path(root).resolve()
    roots = [str(root / part) for part in fixture.get("source_roots", ".").split(",")]
    env = {**os.environ, "PYTHONPATH": os.pathsep.join([str(root), *roots]), "PYTHONDONTWRITEBYTECODE": "1",
           **fixture.get("test_env", {})}
    outcome = {}
    for test in ids:
        result = subprocess.run([sys.executable, "-m", "unittest", "-q", test], cwd=root, env=env,
                                capture_output=True, text=True, timeout=timeout)
        summary = (result.stderr.strip().splitlines() or [""])[-1]
        if result.returncode == 0:
            outcome[test] = "pass"
        elif re.fullmatch(r"FAILED \(failures=\d+\)", summary):
            outcome[test] = "fail"
        else:
            outcome[test] = "error"
    return outcome


def fixture_tests(fixture):
    tests = fixture["tests"]
    return tests["target"], tests["neighbors"] + tests["negative"]


def passed(run, ids=None):
    return all(run[t] == "pass" for t in (run if ids is None else ids))


def failed(run, ids=None):
    """A meaningful failure is an assertion failure; an import or setup error is not."""
    ids = list(run if ids is None else ids)
    return bool(ids) and all(run[t] == "fail" for t in ids)


def validate_fixture(fixture_dir, fixture, work):
    """A fixture is usable only when its reference repair is meaningful and reversible."""
    work = Path(work)
    targets, guards = fixture_tests(fixture)
    checks = []

    def check(name, ok, detail=None):
        checks.append({"check": name, "ok": bool(ok), **({"detail": detail} if detail is not None else {})})

    before = materialize(fixture_dir, fixture, work / "before")
    digest = tree_digest(before)
    run = run_tests(before, targets + guards, fixture)
    no_change = fixture["reference_decision"] == "no_change"
    if no_change:
        check("no_change has an empty reference and no target tests",
              not targets and not (Path(fixture_dir) / "reference.patch").read_text().strip())
    else:
        check("every target test fails (assertion) before the reference repair", failed(run, targets),
              {t: run[t] for t in targets})
    check("neighbor and negative tests pass before", passed(run, guards), {t: run[t] for t in guards})
    apply_patch(before, Path(fixture_dir) / "reference.patch")
    run = run_tests(before, targets + guards, fixture)
    check("every test passes after the reference repair", passed(run), run)
    apply_patch(before, Path(fixture_dir) / "reference.patch", reverse=True)
    check("reversing the reference restores the original tree", tree_digest(before) == digest)
    if targets:
        run = run_tests(before, targets, fixture)
        check("target tests fail again after reversal", failed(run), run)
    naive = materialize(fixture_dir, fixture, work / "naive", state="naive")
    run = run_tests(naive, fixture["naive_must_fail"], fixture)
    check("the naive repair fails (assertion) its condition-specific tests", failed(run), run)
    return {"id": fixture["id"], "ok": all(c["ok"] for c in checks), "checks": checks}


# --- Delivered context per condition ---------------------------------------

def deliver_rg(scenario, root, budgets):
    stopping = budgets["stopping"]
    result = rg_read_baseline(scenario, root, max_calls=stopping["max_calls"], max_bytes=stopping["max_bytes"],
                              max_seconds=stopping["max_seconds"], max_files=budgets["max_files"])
    files = {}
    for relative in result["read_files"]:
        files[relative] = (Path(root) / relative).read_bytes()[:RG_READ_BYTES].decode("utf-8", "replace")
    return {"files": files, "findings": [], "candidates": None,
            "cost": {"calls": result["calls"], "bytes": result["bytes_read"], "seconds": result["elapsed_seconds"],
                     "files": len(files), "limit_reached": result["limit_reached"]}}


def facet_call(call, exclude):
    def wrapped(operation, arguments):
        if operation in ("brief", "agent_context"):
            arguments = {**arguments, "exclude_facets": list(exclude)}
        return call(operation, arguments)
    return wrapped


def retained_findings(call, case_id):
    """Every retained finding of a Case, following page continuations."""
    retained, args = [], {"report_id": case_id, "section": "/findings", "offset": 0, "limit": 1000}
    while args:
        try:
            page = call("page", args)
        except RuntimeError as error:
            # A Case whose findings were not produced has no /findings section.
            if "Unknown report section" not in str(error):
                raise
            return []
        retained.extend(page.get("items", []))
        follow = page.get("continue_with") or {}
        args = follow.get("arguments") if page.get("items") else None
    return retained


def deliver_briefcase(call, scenario, directory, exclude, budgets):
    stopping = budgets["stopping"]
    directory = Path(directory)
    if directory.exists():
        shutil.rmtree(directory)
    result = run_bounded_investigation(facet_call(call, exclude), directory, scenario,
                                       mode=budgets.get("investigation_mode", "manifest-first"),
                                       max_calls=stopping["max_calls"], max_bytes=stopping["max_bytes"],
                                       max_seconds=stopping["max_seconds"], budget=budgets["manifest_bytes"])
    state = json.loads((directory / "state.json").read_text())
    events = {event["id"]: event for event in state["events"]}
    files = {}
    for path, source in state.get("sources", {}).items():
        parts = []
        for span in sorted(source.get("ranges", []), key=lambda s: s["start"]):
            parts.append(json.loads((directory / events[span["event"]]["file"]).read_text())["response"]["text"])
        if parts:
            files[path] = "".join(parts)
    file_passages = None
    if state.get("mode") in ("inventory_first", "excerpt_first"):
        from task_loss_witnesses import inspected_passages
        file_passages = inspected_passages(directory, state)
        files = {path: "\n[uninspected source gap]\n".join(parts) for path, parts in file_passages.items() if parts}
    manifest = json.loads((directory / state["events"][0]["file"]).read_text())["response"]
    # Retained findings are fetched scorer-side for the audit; this read is not
    # delivered to the condition and is not counted in its cost.
    retained = retained_findings(call, state["case_id"])
    return {"files": files, "file_passages": file_passages, "findings": list(manifest.get("findings") or []), "retained_findings": retained,
            "findings_status": manifest.get("findings_status"), "case_id": state["case_id"],
            "candidates": sorted(state.get("sources", {})),
            "cost": {"calls": result["calls"], "bytes": result["bytes"], "seconds": result["elapsed_seconds"],
                     "files": result["inspected_files"], "limit_reached": result["limit_reached"]}}


# --- Scoring ---------------------------------------------------------------

def witness_met(witness, delivered):
    pattern = re.compile(witness["pattern"], re.M)
    if "path" in witness:
        if delivered.get("file_passages") is not None:
            return any(pattern.search(part) is not None for part in delivered["file_passages"].get(witness["path"], []))
        text = delivered["files"].get(witness["path"])
        return text is not None and pattern.search(text) is not None
    return any(finding.get("kind") == witness["finding"] and pattern.search(json.dumps(finding, ensure_ascii=False))
               for finding in delivered["findings"])


def witness_sets(claim):
    return [claim["witnesses"], *claim.get("alternatives", [])]


def claim_supported(claim, delivered):
    return any(all(witness_met(w, delivered) for w in witnesses) for witnesses in witness_sets(claim))


def eligible_claims(claims, sessions):
    return [claim for claim in claims if sessions or not claim.get("requires_sessions")]


def classify_miss(claim, delivered, source_root):
    """Name the first failing stage of the primary witness set (protocol failure_classes)."""
    for witness in claim["witnesses"]:
        if witness_met(witness, delivered):
            continue
        if "finding" in witness:
            # Produced and retained, but its witnessed text was not in what was delivered.
            pattern = re.compile(witness["pattern"], re.M)
            retained = any(f.get("kind") == witness["finding"] and pattern.search(json.dumps(f, ensure_ascii=False))
                           for f in delivered.get("retained_findings") or [])
            return {"class": "consumption" if retained else "producer", "witness": witness}
        if witness["path"] in delivered["files"]:
            full = (Path(source_root) / witness["path"]).read_text(errors="replace")
            found = re.search(witness["pattern"], full, re.M) is not None
            return {"class": "consumption" if found else "producer", "witness": witness}
        candidates = delivered.get("candidates")
        if candidates is None:
            return {"class": "not_delivered", "witness": witness}
        return {"class": "selection" if witness["path"] in candidates else "discovery", "witness": witness}
    return None


def score_claims(claims, delivered, sessions, source_root):
    eligible = eligible_claims(claims, sessions)
    supported = [c["id"] for c in eligible if claim_supported(c, delivered)]
    misses = {c["id"]: classify_miss(c, delivered, source_root) for c in eligible if c["id"] not in supported}
    return {"claims": len(eligible), "supported": supported,
            "supportability": round(len(supported) / len(eligible), 6) if eligible else None, "misses": misses}


CRITICAL_VERIFICATION = re.compile(r"\b(passed|passes|executed|verified)\b", re.I)


def critical_audit(findings):
    """Delivered findings that assert more than their basis supports."""
    violations = []

    def flag(finding, reason):
        violations.append({"comparison_key": finding.get("comparison_key"), "kind": finding.get("kind"), "reason": reason})

    for finding in findings:
        payload, basis = finding.get("payload") or {}, finding.get("basis") or {}
        applicability = finding.get("applicability") or {}
        if applicability.get("current_guidance") is True:
            flag(finding, "session history presented as current guidance")
        kind = finding.get("kind")
        if kind == "behavior_condition":
            roles = {json.dumps(e) for e in basis.get("evidence", [])}
            if "declared relationship" in finding.get("claim", "") and not any('"capability"' in r for r in roles):
                flag(finding, "capability claimed without a declaration basis")
            for condition in payload.get("conditions", []):
                if condition.get("runtime_satisfaction") != "unknown":
                    flag(finding, "runtime satisfaction asserted from static evidence")
            for check in payload.get("verification", []):
                statement = check.get("statement", "")
                if check.get("assessment") == "established" or (
                        CRITICAL_VERIFICATION.search(statement) and "does not establish" not in statement):
                    flag(finding, "test execution asserted from a static association")
        elif payload.get("type") == "session_experience":
            for field in ("causality", "necessity"):
                if payload.get(field) not in (None, "unverified"):
                    flag(finding, f"session {field} asserted")
        elif kind == "architectural_convergence":
            if (payload.get("decision") or {}).get("automatic_change") is not False:
                flag(finding, "convergence proposes an automatic change")
            if basis.get("authority") != "observed_convention":
                flag(finding, "observed prevalence presented as policy")
    return violations


# --- Agent tier ------------------------------------------------------------

AGENT_INSTRUCTIONS = (
    "Use only the delivered context. Return one JSON object: {\"claims\": [{\"text\": str, \"citations\": [path]}], "
    "\"decision\": \"repair\" | \"no_change\", \"rationale\": str}. Cite delivered file paths, or finding:<kind> "
    "for a delivered finding. For a repair fixture, edit files in the working directory; do not edit tests unless "
    "the task requires it.")


def agent_bundle(task, condition, delivered):
    return {"instructions": AGENT_INSTRUCTIONS, "condition": condition,
            "task": task["task"], "target": task.get("target"),
            "files": [{"path": path, "text": text} for path, text in sorted(delivered["files"].items())],
            "findings": delivered["findings"]}


def run_agent(command, bundle, workdir, timeout):
    started = time.monotonic()
    result = subprocess.run(command, shell=True, input=json.dumps(bundle), cwd=workdir, capture_output=True,
                            text=True, timeout=timeout)
    seconds = round(time.monotonic() - started, 3)
    if result.returncode:
        return {"status": "error", "error": (result.stderr or result.stdout).strip()[:2000], "seconds": seconds}
    try:
        answer = json.loads(result.stdout[result.stdout.index("{"):result.stdout.rindex("}") + 1])
    except ValueError:
        return {"status": "error", "error": "agent output is not one JSON object", "seconds": seconds}
    return {"status": "ok", "answer": answer, "seconds": seconds}


def changed_files(before, after):
    def files(root):
        return {str(p.relative_to(root)): p.read_bytes() for p in Path(root).rglob("*")
                if p.is_file() and "__pycache__" not in p.parts}
    a, b = files(before), files(after)
    return sorted(path for path in set(a) | set(b) if a.get(path) != b.get(path))


def score_agent(task, answer, delivered, forbidden, workdir=None, pristine=None, fixture=None):
    claims = [c for c in answer.get("claims", []) if isinstance(c, dict)]
    delivered_kinds = {f.get("kind") for f in delivered["findings"]}

    def cited(path):
        if isinstance(path, str) and path.startswith("finding:"):
            return path.removeprefix("finding:") in delivered_kinds
        return path in delivered["files"]

    unsupported = [c.get("text", "") for c in claims
                   if not c.get("citations") or not all(cited(p) for p in c.get("citations", []))]
    text = "\n".join([*(c.get("text", "") for c in claims), answer.get("rationale", "")])
    critical = [p for p in forbidden if re.search(p, text)]
    citations = {p for c in claims for p in c.get("citations", []) if cited(p)}
    covered = [claim["id"] for claim in task["claims"]
               if any(all(w["path"] in citations for w in ws if "path" in w) and any("path" in w for w in ws)
                      for ws in witness_sets(claim))]
    scored = {"claims": len(claims), "unsupported_assertions": len(unsupported),
              "unsupported_critical_claims": len(critical), "critical_patterns": critical,
              "cited_claims": covered}
    if fixture is not None:
        targets, guards = fixture_tests(fixture)
        edits = changed_files(pristine, workdir)
        run = run_tests(workdir, targets + guards, fixture)
        decision = answer.get("decision")
        scored.update({
            "decision": decision, "edited": edits,
            "unnecessary_edits": [p for p in edits if p not in fixture["allowed_paths"]],
            "tests": run,
            "validated_repair": passed(run) and (bool(edits) if fixture["reference_decision"] == "repair" else not edits),
            "justified_decision": decision == fixture["reference_decision"]
            and any(p in citations for p in fixture["decision_witness_paths"])})
    return scored


# --- Statistics and gate ---------------------------------------------------

def paired_bootstrap(differences, resamples=10000, seed=20261018, level=0.95):
    if not differences:
        return {"n": 0, "mean": None, "interval": [None, None]}
    rng = random.Random(seed)
    n = len(differences)
    means = sorted(sum(differences[rng.randrange(n)] for _ in range(n)) / n for _ in range(resamples))
    tail = (1 - level) / 2
    lo, hi = means[int(tail * resamples)], means[min(resamples - 1, int((1 - tail) * resamples))]
    return {"n": n, "mean": round(sum(differences) / n, 6), "interval": [round(lo, 6), round(hi, 6)]}


def scenario_means(rows, condition, metric="supportability"):
    """Mean over repeats per scenario; None-valued scenarios are not scored."""
    by_scenario = {}
    for row in rows:
        if row["condition"] == condition and row.get(metric) is not None:
            by_scenario.setdefault(row["scenario"], []).append(row[metric])
    return {scenario: sum(values) / len(values) for scenario, values in by_scenario.items()}


def contrast(rows, treatment, control, statistics, metric="supportability"):
    a, b = scenario_means(rows, treatment, metric), scenario_means(rows, control, metric)
    shared = sorted(set(a) & set(b))
    result = paired_bootstrap([a[s] - b[s] for s in shared], statistics.get("resamples", 10000),
                              statistics.get("seed", 20261018))
    critical = {name: sum(r.get("unsupported_critical_claims", 0) for r in rows
                          if r["condition"] == name and r["scenario"] in shared)
                for name in (treatment, control)}
    return {"treatment": treatment, "control": control, "metric": metric, "scenarios": shared, **result,
            "unsupported_critical_claims": critical}


def gate(primary, protocol):
    rule = protocol["exit_gate"]
    if primary["mean"] is None:
        return {"status": "not_run", "reason": "no paired scenarios"}
    critical = primary["unsupported_critical_claims"]
    if critical[primary["treatment"]] > critical[primary["control"]]:
        return {"status": "not_met", "reason": "unsupported critical claims increased"}
    if primary["mean"] >= rule["minimum_useful_effect"] and primary["interval"][0] > 0:
        return {"status": "met", "reason": "predeclared minimum useful effect with a positive interval"}
    if primary["mean"] >= rule["minimum_useful_effect"]:
        return {"status": "inconclusive", "reason": "point gain reaches the minimum but the interval includes 0"}
    return {"status": "not_met", "reason": f"mean paired gain {primary['mean']} is below the minimum useful effect "
                                          f"{rule['minimum_useful_effect']}"}
