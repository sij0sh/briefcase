"""Score pinned inspected passages, delivered relationships and explicit conclusions.

Oracles are scorer-only. Passage availability is reported separately from
conclusion support; an empty conclusion submission makes no answer-quality claim.
"""
import hashlib
import json
from pathlib import Path
import re

from run_evidence import atomic_json
from task_investigation import key
from task_loss_witnesses import inspected_passages


def validate_oracles(scenarios):
    ids = [s["id"] for s in scenarios]
    if len(set(ids)) != len(ids):
        raise ValueError("Duplicate scenario oracle")
    for scenario in scenarios:
        groups = {name: scenario.get(name, []) for name in ("passages", "relationships", "claims")}
        for name, rows in groups.items():
            if len({r["id"] for r in rows}) != len(rows):
                raise ValueError(f"Duplicate {name} oracle")
        for passage in groups["passages"]:
            re.compile(passage["pattern"], re.M)
            span = passage.get("byte_range")
            if span and not (type(span.get("start")) is int and type(span.get("end")) is int
                             and 0 <= span["start"] < span["end"]):
                raise ValueError("Invalid witness byte range")
        for claim in groups["claims"]:
            for alternative in [claim, *claim.get("alternatives", [])]:
                if not (alternative.get("passages") or alternative.get("relationships")):
                    raise ValueError("A claim needs at least one evidence obligation")
                for name in ("passages", "relationships"):
                    if set(alternative.get(name, [])) - {r["id"] for r in groups[name]}:
                        raise ValueError("Claim names an unknown evidence obligation")


def read_evidence(checkpoint):
    checkpoint = Path(checkpoint)
    state = json.loads(checkpoint.read_text())
    responses = []
    by_event = {}
    for record in state["events"]:
        event = json.loads((checkpoint.parent / record["file"]).read_text())
        if key(event) != record["id"]:
            raise ValueError("Stored evidence was modified")
        response = event["response"]
        if response.get("historical_packet") or response.get("historical_index"):
            raise ValueError("Retracted evidence cannot support a current conclusion")
        if response.get("view_id") not in (None, state["view_id"]):
            raise ValueError("Evidence belongs to a different source pin")
        if response.get("index_id") not in (None, state["index_id"]):
            raise ValueError("Evidence belongs to a different relationship index")
        responses.append(response)
        by_event[record["id"]] = response
    passages = inspected_passages(checkpoint.parent, state)
    # Keep offsets/digests as well as merged text. Patterns never bridge a gap.
    ranges = {path: source.get("ranges", []) + source.get("excerpts", [])
              for path, source in state["sources"].items()}
    identities = {path: source.get("identity", {}) for path, source in state["sources"].items()}
    for path, spans in ranges.items():
        for span in spans:
            entry = by_event[span["event"]]["entry"]
            if any(entry.get(k) != identities[path].get(k) for k in ("artifact_id", "digest", "bytes")):
                raise ValueError("Checkpoint source identity differs from the raw pinned response")
    return state, passages, ranges, identities, responses


def relationship_records(responses):
    found = {}
    def visit(value):
        if isinstance(value, list):
            for item in value:
                visit(item)
        elif isinstance(value, dict):
            if (str(value.get("id", "")).startswith("task-link:")
                    and value.get("predicate") and "from_files" in value and "to_files" in value
                    and value.get("basis") and value.get("class")):
                row = dict(value)
                # The predicate establishes the kind of static witness; it
                # never upgrades a retained relationship to executed evidence.
                if row["predicate"] in ("IMPORTS", "REFERENCES", "DEPENDS_ON", "TESTS", "READS_CONFIG", "DECLARES_CONFIG_KEY", "CALLS_STATIC"):
                    row.setdefault("authority", "static_association")
                found[value["id"]] = row
            # Candidate associations have their own identity and can never
            # satisfy an oracle for a call or executed dependency.
            elif value.get("kind") == "exact_key_association" and value.get("authority") == "static_association":
                identity = key(value)
                found[identity] = {**value, "id": identity, "predicate": "EXACT_KEY_ASSOCIATION",
                                   "from_files": [value["from_path"]], "to_files": [value["to_path"]]}
            for child in value.values():
                visit(child)
    for response in responses:
        visit(response)
    return found


def score(oracle, passages, ranges, identities, responses, conclusions=None):
    passage_hits, relation_hits = {}, {}
    for witness in oracle.get("passages", []):
        path = witness["path"]
        expected_digest = witness.get("source_sha256")
        digest_matches = not expected_digest or identities.get(path, {}).get("digest") == "sha256:" + expected_digest
        span = witness.get("byte_range")
        # Adjacent inspected chunks can jointly cover an oracle span.
        covered = not span
        if span:
            end = span["start"]
            for part in sorted(ranges.get(path, []), key=lambda r: (r["start"], r["end"])):
                if part["start"] <= end:
                    end = max(end, part["end"])
            covered = end >= span["end"]
        passage_hits[witness["id"]] = bool(digest_matches and covered and any(
            re.search(witness["pattern"], part, re.M) for part in passages.get(path, [])))
    records = relationship_records(responses)
    for oracle_relation in oracle.get("relationships", []):
        relation_hits[oracle_relation["id"]] = [identity for identity, row in records.items()
            if row["predicate"] == oracle_relation["predicate"]
            and oracle_relation["from_path"] in row["from_files"]
            and oracle_relation["to_path"] in row["to_files"]
            and all(not oracle_relation.get(k) or row.get(k) == oracle_relation[k]
                    for k in ("basis", "class", "authority"))]
    claims = {c["id"]: c for c in oracle.get("claims", [])}
    def met(spec):
        return all(passage_hits[p] for p in spec.get("passages", [])) and all(
            relation_hits[r] for r in spec.get("relationships", []))
    supported_claims = [c["id"] for c in claims.values() if any(met(a) for a in [c, *c.get("alternatives", [])])]
    assessed = []
    submitted_ids = [c.get("claim_id") for c in conclusions or []]
    if len(set(submitted_ids)) != len(submitted_ids):
        raise ValueError("A conclusion submission must assess each claim at most once")
    for conclusion in conclusions or []:
        claim = claims.get(conclusion.get("claim_id"))
        citations = conclusion.get("citations", [])
        cited = set()
        # Explicit conclusion text must match the registered claim, rather
        # than inheriting support for an arbitrary assertion with the same ID.
        eligible = claim is not None and conclusion.get("text") == claim.get("text")
        for witness in oracle.get("passages", []):
            span = witness.get("byte_range")
            for citation in citations:
                if (span and citation.get("path") == witness["path"]
                    and citation.get("digest") == identities.get(witness["path"], {}).get("digest")
                    and type(citation.get("start_byte")) is int and type(citation.get("end_byte")) is int
                    and citation["start_byte"] <= span["start"] < span["end"] <= citation["end_byte"]
                    and any(p["start"] <= citation["start_byte"] <= citation["end_byte"] <= p["end"]
                            for p in ranges.get(witness["path"], []))):
                    cited.add(witness["id"])
        ids = set(conclusion.get("relationship_ids", []))
        support = eligible and any(met(a) and set(a.get("passages", [])) <= cited
            and all(ids & set(relation_hits[r]) for r in a.get("relationships", []))
            for a in [claim, *claim.get("alternatives", [])])
        assessed.append({"claim_id": conclusion.get("claim_id"), "supported": bool(support)})
    return {"passages_inspected": sum(passage_hits.values()), "passages_required": len(passage_hits),
            "passage_recall": sum(passage_hits.values()) / len(passage_hits) if passage_hits else None,
            "passage_hits": passage_hits, "relationships_delivered": sum(bool(x) for x in relation_hits.values()),
            "relationships_required": len(relation_hits),
            "relationship_recall": sum(bool(x) for x in relation_hits.values()) / len(relation_hits) if relation_hits else None,
            "relationship_hits": relation_hits, "claims_supportable": len(supported_claims),
            "claims_required": len(claims), "supportable_claim_ids": supported_claims,
            "claim_supportability": len(supported_claims) / len(claims) if claims else None,
            "conclusions": {"state": "scored" if conclusions is not None else "not_submitted",
                "submitted": len(assessed), "supported": sum(r["supported"] for r in assessed),
                "unsupported": sum(not r["supported"] for r in assessed),
                "precision": sum(r["supported"] for r in assessed) / len(assessed) if assessed else None,
                "claim_coverage": sum(r["supported"] for r in assessed) / len(claims) if claims and conclusions is not None else None,
                "rows": assessed}}


def aggregate(rows):
    totals = {name: sum(r.get(name, 0) for r in rows) for name in
        ("passages_inspected", "passages_required", "relationships_delivered", "relationships_required", "claims_supportable", "claims_required")}
    errors = sum(r.get("status") == "error" for r in rows)
    for prefix, hits, total, ratio in (("passage", "passages_inspected", "passages_required", "passage_recall"),
        ("relationship", "relationships_delivered", "relationships_required", "relationship_recall"),
        ("claim", "claims_supportable", "claims_required", "claim_supportability")):
        totals[f"micro_{prefix}_coverage_lower_bound"] = totals[hits] / totals[total] if totals[total] else None
        totals[f"micro_{prefix}_coverage"] = totals[f"micro_{prefix}_coverage_lower_bound"] if not errors else None
        values = [r[ratio] for r in rows if r.get(ratio) is not None]
        totals[f"macro_{prefix}_coverage"] = sum(values) / len(values) if values and not errors else None
    totals["conclusions"] = {field: sum(r.get("conclusions", {}).get(field, 0) for r in rows)
                             for field in ("submitted", "supported", "unsupported")}
    conclusions = totals["conclusions"]
    conclusions["precision"] = conclusions["supported"] / conclusions["submitted"] if conclusions["submitted"] and not errors else None
    submitted = any(r.get("conclusions", {}).get("state") == "scored" for r in rows)
    conclusions["claim_coverage"] = conclusions["supported"] / totals["claims_required"] if submitted and totals["claims_required"] and not errors else None
    conclusions["state"] = "scored" if submitted else "not_submitted"
    totals.update(errors=errors, comparison_eligible=not errors, scenarios=rows)
    return totals


def evaluate(results_path, oracles, submissions=None):
    raw = Path(results_path).read_bytes()
    report = json.loads(raw)
    labels = {s["id"]: s for s in oracles}
    modes = {}
    for mode, rows in report.get("investigations", {}).items():
        if len(rows) != len(labels) or {r["id"] for r in rows} != set(labels):
            raise ValueError("Every investigation arm must retain every authored scenario")
        scored = []
        for row in rows:
            oracle = labels[row["id"]]
            if row.get("status") == "error":
                scored.append({"id": row["id"], "status": "error", "passages_required": len(oracle.get("passages", [])),
                    "relationships_required": len(oracle.get("relationships", [])), "claims_required": len(oracle.get("claims", []))})
                continue
            state, passages, ranges, identities, responses = read_evidence(row["checkpoint"])
            if any(state[k] != row[k] for k in ("case_id", "index_id", "view_id")):
                raise ValueError("Checkpoint differs from the measured investigation")
            conclusions = None if submissions is None else submissions.get((mode, row["id"]), [])
            scored.append({"id": row["id"], **score(oracle, passages, ranges, identities, responses, conclusions)})
        modes[mode] = aggregate(scored)
    return {"results": str(results_path), "results_sha256": hashlib.sha256(raw).hexdigest(), "modes": modes}


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, nargs="+", required=True)
    parser.add_argument("--oracles", type=Path, required=True)
    parser.add_argument("--conclusions", type=Path, help="JSON scenarios: [{id, mode, conclusions: [{claim_id, text, citations, relationship_ids}]}]")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    oracle_bytes = args.oracles.read_bytes()
    oracles = json.loads(oracle_bytes)["scenarios"]
    validate_oracles(oracles)
    submissions = None
    if args.conclusions:
        submissions = {(r["mode"], r["id"]): r["conclusions"] for r in json.loads(args.conclusions.read_text())["scenarios"]}
    atomic_json(args.output, {"method": "priority3-inspected-evidence-v1",
        "oracles_sha256": hashlib.sha256(oracle_bytes).hexdigest(),
        "conclusions_sha256": hashlib.sha256(args.conclusions.read_bytes()).hexdigest() if args.conclusions else None,
        "reports": [evaluate(p, oracles, submissions) for p in args.results],
        "interpretation": "Scorer-only oracles never enter retrieval. Passage/relationship availability is distinct from explicitly submitted supported conclusions. Static associations cannot establish execution or arbitrary answer correctness."})


if __name__ == "__main__":
    main()
