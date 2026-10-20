#!/usr/bin/env python3
"""Check a seeded sample of syntactic TS binding REFERENCES against source.

A witness is correct when its retained span is exactly an identifier use of the
referenced definition: the span names the definition, or a local alias the
subject file imports for it. Read-only against the database and source root.
"""
import argparse
import json
from pathlib import Path
import random
import re
import sqlite3

SCOPE_PREFIX = "imported binding uses resolved syntactically"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--view", required=True, help="Profile view holding the binding run")
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--sample", type=int, default=400)
    parser.add_argument("--seed", type=int, default=20261020)
    parser.add_argument("--output", type=Path, required=True)
    a = parser.parse_args()
    with sqlite3.connect(a.db.resolve().as_uri() + "?mode=ro", uri=True) as connection:
        bundle = json.loads(connection.execute("SELECT bundle FROM views WHERE id=?", (a.view,)).fetchone()[0])
    runs = {r["id"] for r in bundle["runs"]
            if any(g["predicate"] == "REFERENCES" and g.get("scope", "").startswith(SCOPE_PREFIX) for g in r["coverage"]["guarantees"])}
    keys = {e["id"]: e["canonical_key"] for e in bundle["entity_versions"]}
    artifacts = {x["id"]: x["path"] for x in bundle["artifacts"]}
    evidence = {e["id"]: e for e in bundle["evidence_refs"]}
    references = [x for x in bundle["assertions"]
                  if x["proposition"]["predicate"] == "REFERENCES" and x["origin"].get("run_id") in runs]
    sample = random.Random(a.seed).sample(references, min(a.sample, len(references)))
    texts, outcomes, failures = {}, {"correct": 0, "alias": 0, "incorrect": 0}, []
    for assertion in sample:
        target = keys[assertion["proposition"]["object"]["entity_version_id"]]
        name = target.split("::")[-1].split("#")[0]
        ref = evidence[assertion["evidence_ref_ids"][0]]
        path = artifacts[ref["artifact_id"]]
        locator = ref["locator"]
        if path not in texts:
            texts[path] = (a.root / path).read_bytes()
        span = texts[path][locator["start"]:locator["end"]].decode("utf-8", "replace")
        source = texts[path].decode("utf-8", "replace")
        aliased = re.search(rf"\b{re.escape(name)}\s+as\s+{re.escape(span)}\b", source) or re.search(
            rf"^\s*import\s+{re.escape(span)}\b", source, re.M)
        if span == name:
            outcomes["correct"] += 1
        elif re.fullmatch(r"[A-Za-z_$][\w$]*", span) and aliased:
            outcomes["alias"] += 1
        else:
            outcomes["incorrect"] += 1
            failures.append({"path": path, "span": span[:80], "target": target})
    total = sum(outcomes.values())
    report = {"view": a.view, "binding_references": len(references), "sample": total, "seed": a.seed,
        "outcomes": outcomes, "precision": (outcomes["correct"] + outcomes["alias"]) / total if total else None,
        "failures": failures[:40],
        "interpretation": "Precision of the retained witness span only. A correct binding use is association between definitions, not a call, read or runtime dependency."}
    a.output.write_text(json.dumps(report, indent=1) + "\n")
    print(json.dumps({k: report[k] for k in ("binding_references", "sample", "outcomes", "precision")}))


if __name__ == "__main__":
    main()
