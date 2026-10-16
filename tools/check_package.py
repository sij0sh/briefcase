"""Run authored planning contracts and fixture expectations offline."""
import copy
import hashlib
import json
from pathlib import Path
import re

from identity import canonical_bytes, digest, load_json
from lifecycle_oracle import LifecycleOracle
from schema_profile import inspect_schema, validate as structural
from validate import ContractError, validate_bundle, validate_query, validate_batch, ROOT


def documentation_files(root=ROOT):
    """Check authored product documentation, not agent archives or execution evidence."""
    readme = root / "README.md"
    if readme.is_file():
        yield readme
    for name in ("docs", "contracts"):
        yield from sorted((root / name).rglob("*.md"))


def set_path(document, path, value):
    parts = path.split("/")
    current = document
    for part in parts[:-1]:
        current = current[int(part)] if isinstance(current, list) else current[part]
    if isinstance(current, list):
        current[int(parts[-1])] = value
    else:
        current[parts[-1]] = value


def edge_keys(bundle, predicate):
    entities = {e["id"]: e for e in bundle["entity_versions"]}
    return sorted([[entities[a["proposition"]["subject_id"]]["canonical_key"],
                    entities[a["proposition"]["object"]["entity_version_id"]]["canonical_key"]]
                   for a in bundle["assertions"] if a["proposition"]["predicate"] == predicate])


def main():
    fixture_root = ROOT / "evaluation/fixtures"
    index = load_json(fixture_root / "index.json")
    successes, failures = [], []

    def check(name, work):
        try:
            detail = work()
            successes.append({"id": name, "status": "passed", "detail": detail})
        except Exception as exc:
            failures.append({"id": name, "status": "failed", "error": str(exc)})

    def check_schema(path):
        schema = load_json(path)
        inspect_schema(schema)
        return "supported profile keywords and local references"

    for path in sorted((ROOT / "contracts/v1").glob("*.schema.json")):
        check("schema:" + path.name, lambda p=path: check_schema(p))
    for name in index["valid_bundles"]:
        check("valid:" + name, lambda n=name: len(validate_bundle(load_json(fixture_root / n))["tables"]["assertions"]))
    for case in index["valid_queries"]:
        check("query:" + case["query"], lambda c=case: validate_query(load_json(fixture_root / c["query"]), load_json(fixture_root / c["bundle"])))
    for case in index.get("valid_batches", []):
        check("batch:" + case["batch"], lambda c=case: validate_batch(load_json(fixture_root / c["batch"]), load_json(fixture_root / c["bundle"]), c["capability"]))

    def rejection(case):
        b = load_json(fixture_root / case["bundle"])
        q = load_json(fixture_root / case["query"]) if case.get("query") else None
        batch = load_json(fixture_root / case["batch"]) if case.get("batch") else None
        for patch in case["patches"]:
            if patch["operation"] != "set":
                raise ValueError("Unsupported mutation operation")
            path = patch["path"]
            if path.startswith("query/"):
                set_path(q, path[6:], copy.deepcopy(patch["value"]))
            elif path.startswith("batch/"):
                set_path(batch, path[6:], copy.deepcopy(patch["value"]))
            else:
                set_path(b, path, copy.deepcopy(patch["value"]))
        try:
            checked = validate_bundle(b)
            if q is not None:
                validate_query(q, b, checked)
            if batch is not None:
                validate_batch(batch, b, case["capability"], checked)
        except ContractError as exc:
            if exc.code != case["expected_code"]:
                raise AssertionError(f"Expected {case['expected_code']}, got {exc}") from exc
            return exc.code
        raise AssertionError("Invalid fixture was accepted")

    for case in load_json(fixture_root / index["rejections"]):
        check("reject:" + case["id"], lambda c=case: rejection(c))

    def lifecycle(path):
        scenario = load_json(path)
        oracle = LifecycleOracle()
        for i, step in enumerate(scenario["steps"]):
            actual = oracle.apply(step["action"])
            if actual != step["expected"]:
                raise AssertionError(f"Step {i}: expected {step['expected']}, got {actual}")
        return len(scenario["steps"])
    for path in sorted((ROOT / "evaluation/scenarios").glob("*.json")):
        check("lifecycle:" + path.stem, lambda p=path: lifecycle(p))

    def vector(item):
        raw = item["expected_canonical_utf8"].encode("utf-8")
        if canonical_bytes(item["input"]) != raw:
            raise AssertionError("Canonical bytes do not match independent literal")
        expected = "sha256:" + hashlib.sha256(raw).hexdigest()
        if digest(item["input"]) != item["expected_digest"] or expected != item["expected_digest"]:
            raise AssertionError("Identity vector mismatch")
        return expected
    for item in load_json(ROOT / "contracts/v1/identity-vectors.json"):
        check("identity:" + item["id"], lambda v=item: vector(v))

    def canonical_rejections():
        for item in [1.2, {"e\u0301": 1, "\u00e9": 2}]:
            try:
                canonical_bytes(item)
            except ValueError:
                continue
            raise AssertionError("Invalid identity input accepted")
        return "float and normalized-key collision rejected"
    check("identity:invalid-inputs", canonical_rejections)

    def gold_expectations():
        gold = load_json(ROOT / "evaluation/gold/source-expectations.json")
        base = load_json(fixture_root / "rust-base.json")
        changed = load_json(fixture_root / "rust-changed.json")
        for b, key in [(base, "base_imports"), (changed, "changed_imports")]:
            if edge_keys(b, "IMPORTS") != sorted(gold["rust"][key]):
                raise AssertionError("Authored normalized imports differ from independent gold")
        old_edges = {tuple(x) for x in edge_keys(base, "IMPORTS")}
        new_edges = {tuple(x) for x in edge_keys(changed, "IMPORTS")}
        if sorted(new_edges - old_edges) != sorted(tuple(x) for x in gold["rust"]["added_imports"]):
            raise AssertionError("Two-snapshot correspondence-key diff mismatch")
        if sorted(old_edges - new_edges) != sorted(tuple(x) for x in gold["rust"]["removed_imports"]):
            raise AssertionError("Removed relationship mismatch")
        # The gold cycle is a witness: check that each required edge is present.
        for component in gold["rust"]["changed_sccs"]:
            for i, name in enumerate(component):
                if (name, component[(i + 1) % len(component)]) not in new_edges:
                    raise AssertionError("Gold cycle witness edge missing")
        if edge_keys(base, "CALLS_STATIC") != sorted(gold["rust"]["exact_calls"]):
            raise AssertionError("Selected Rust call fixture mismatch")
        ts = load_json(fixture_root / "typescript-base.json")
        if edge_keys(ts, "CALLS_STATIC") != sorted(gold["typescript"]["selected_calls"]):
            raise AssertionError("Selected TypeScript call fixture mismatch")
        prohibited = tuple(gold["policy"]["witness"][0])
        if prohibited in old_edges or prohibited not in new_edges:
            raise AssertionError("Policy witness expected only in changed fixture")
        return "authored normalized expectations; no analyzer executed"
    check("gold:relationships-diff-and-policy-witness", gold_expectations)

    def docs():
        links = 0
        for path in documentation_files(ROOT):
            content = path.read_text()
            if content.count("```") % 2:
                raise AssertionError(f"Unclosed code fence in {path.relative_to(ROOT)}")
            for target in re.findall(r"\]\(([^)]+)\)", content):
                if target.startswith(("https:", "http:", "#")):
                    continue
                target = target.split("#")[0]
                if not (path.parent / target).is_file():
                    raise AssertionError(f"Missing link {path.relative_to(ROOT)} → {target}")
                links += 1
        adrs = sorted((ROOT / "docs/adr").glob("[0-9][0-9][0-9][0-9]-*.md"))
        if [p.name[:4] for p in adrs] != [f"{i:04}" for i in range(1,32)]:
            raise AssertionError("Expected ADR-0001 through ADR-0031")
        return {"relative_links": links, "adrs": len(adrs)}
    check("documentation:links-fences-and-adr-count", docs)

    def status_documents():
        # Status records drift as later sessions change the product: each
        # must say near its top whether it is current or a historical
        # record, so a reader (or agent) never takes an old run as current.
        declared = {}
        for path in sorted((ROOT / "docs").glob("*-STATUS.md")):
            head = path.read_text().splitlines()[:6]
            status = next((l for l in head if l.startswith("Status: ")), None)
            if not status or not status.startswith(("Status: current", "Status: historical")):
                raise AssertionError(f"{path.relative_to(ROOT)} needs 'Status: current' or 'Status: historical' in its first lines")
            declared[path.name] = status.split()[1].rstrip(" —")
        return declared
    check("documentation:status-documents-declare-currency", status_documents)

    report = {"status": "passed" if not failures else "failed",
              "scope": "planning contracts, authored fixtures, reference lifecycle; no production analyzer/storage execution",
              "passed": len(successes), "failed": len(failures),
              "checks": successes + failures}
    output = ROOT / "evaluation/verification-report.json"
    output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "checks"}, indent=2))
    for failure in failures:
        print(json.dumps(failure, ensure_ascii=False))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
