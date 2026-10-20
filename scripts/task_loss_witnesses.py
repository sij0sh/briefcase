#!/usr/bin/env python3
"""Score authored passage witnesses against saved loss-boundary checkpoints.

Offline only: no labels or patterns are passed to the investigator. Disjoint
source ranges stay separate so a regex cannot match across unread bytes.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re

from run_evidence import atomic_json
from task_investigation import key


def inspected_passages(directory, state, allowed_events=None):
    directory = Path(directory)
    events = {event["id"]: event for event in state["events"]}
    files = {}
    for path, source in state["sources"].items():
        chunks = []
        for span in source.get("ranges", []) + source.get("excerpts", []):
            if allowed_events is not None and span["event"] not in allowed_events:
                continue
            event = json.loads((directory / events[span["event"]]["file"]).read_text())
            if key(event) != span["event"]:
                raise ValueError("Stored source evidence was modified")
            response = event["response"]
            raw = response["text"].encode("utf-8")
            if (response["entry"]["path"] != path or response["source_offset"] != span["start"]
                    or span["end"] - span["start"] != len(raw)):
                raise ValueError("Stored source range differs from its evidence")
            chunks.append((span["start"], raw))
        passages, start, merged = [], None, b""
        for offset, raw in sorted(chunks):
            if start is None or offset > start + len(merged):
                if start is not None:
                    passages.append(merged.decode("utf-8"))
                start, merged = offset, raw
            else:
                relative = offset - start
                overlap = min(len(raw), len(merged) - relative)
                if merged[relative:relative + overlap] != raw[:overlap]:
                    raise ValueError("Overlapping pinned source ranges disagree")
                merged += raw[overlap:]
        if start is not None:
            passages.append(merged.decode("utf-8"))
        files[path] = passages
    return files


def score_claims(claims, passages, findings=()):
    def witnessed(witness):
        pattern = re.compile(witness["pattern"], re.M)
        if "path" in witness:
            return any(pattern.search(part) for part in passages.get(witness["path"], []))
        return any(finding.get("kind") == witness["finding"] and
                   pattern.search(json.dumps(finding, ensure_ascii=False)) for finding in findings)

    supported = [claim["id"] for claim in claims if any(all(witnessed(w) for w in witnesses)
                 for witnesses in [claim["witnesses"], *claim.get("alternatives", [])])]
    primary = [w for claim in claims for w in claim["witnesses"]]
    return {"supported_claims": len(supported), "total_claims": len(claims),
        "supportability": len(supported) / len(claims) if claims else None,
        "primary_witnesses_inspected": sum(witnessed(w) for w in primary), "primary_witnesses": len(primary),
        "supported_claim_ids": supported}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, nargs="+", required=True)
    parser.add_argument("--claims", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    claim_bytes = args.claims.read_bytes()
    claims = {row["id"]: row["claims"] for row in json.loads(claim_bytes)["scenarios"]}
    reports = []
    for path in args.results:
        raw = path.read_bytes()
        result, modes = json.loads(raw), {}
        for mode, rows in result["investigations"].items():
            scored = []
            for row in rows:
                if row.get("status") == "error":
                    raise ValueError(f"Cannot score a failed investigation: {row['id']}")
                checkpoint = Path(row["checkpoint"])
                state = json.loads(checkpoint.read_text())
                if any(state[k] != row[k] for k in ("case_id", "index_id", "view_id")):
                    raise ValueError("Checkpoint belongs to another investigation")
                first = state["events"][0]
                manifest_event = json.loads((checkpoint.parent / first["file"]).read_text())
                if key(manifest_event) != first["id"]:
                    raise ValueError("Stored manifest evidence was modified")
                # The historical stopping rule checks budgets before each RPC,
                # so the last response can cross the byte threshold. Score the
                # strict prefix as well; do not credit those final excess bytes.
                allowed, cumulative = set(), 0
                for event in state["events"]:
                    value = json.loads((checkpoint.parent / event["file"]).read_text())
                    if key(value) != event["id"]:
                        raise ValueError("Stored investigation evidence was modified")
                    cumulative += len((json.dumps(value["response"], ensure_ascii=False) + "\n").encode())
                    if cumulative > result["limits"]["max_bytes"]:
                        break
                    allowed.add(event["id"])
                passages = inspected_passages(checkpoint.parent, state)
                strict = inspected_passages(checkpoint.parent, state, allowed)
                findings = manifest_event["response"].get("findings") or []
                expected = set(row["recovered_required_paths"]) | set(row["missing_required_paths"])
                within = {path for path, parts in strict.items() if parts}
                scored.append({"id": row["id"], **score_claims(claims[row["id"]], passages, findings),
                    "strict_byte_prefix": {**score_claims(claims[row["id"]], strict, findings if first["id"] in allowed else []),
                        "required_paths_inspected": len(expected & within),
                        "required_paths_beyond_manifest": len((expected & within) - set(row["manifest_required_paths"]))}})
            total = sum(row["total_claims"] for row in scored)
            supported = sum(row["supported_claims"] for row in scored)
            modes[mode] = {"supported_claims": supported, "total_claims": total,
                "micro_supportability": supported / total if total else None,
                "macro_supportability": sum(row["supportability"] for row in scored) / len(scored) if scored else None,
                "primary_witnesses_inspected": sum(row["primary_witnesses_inspected"] for row in scored),
                "primary_witnesses": sum(row["primary_witnesses"] for row in scored),
                "strict_byte_prefix": {field: sum(row["strict_byte_prefix"][field] for row in scored) for field in
                    ("supported_claims", "primary_witnesses_inspected", "required_paths_inspected", "required_paths_beyond_manifest")},
                "scenarios": scored}
        reports.append({"results": str(path), "results_sha256": hashlib.sha256(raw).hexdigest(), "modes": modes})
    atomic_json(args.output, {"method": "inspected-passage-witnesses-v1",
        "claims_sha256": hashlib.sha256(claim_bytes).hexdigest(), "reports": reports,
        "interpretation": "Delivered witness patterns establish passage availability, not reasoning, runtime execution, or answer correctness."})
    print(json.dumps({report["results"]: {mode: {k: v for k, v in score.items() if k != "scenarios"}
                                         for mode, score in report["modes"].items()} for report in reports}))


if __name__ == "__main__":
    main()
