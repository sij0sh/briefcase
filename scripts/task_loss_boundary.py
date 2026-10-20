"""Offline loss attribution for a frozen task evaluation.

Authored paths are used only here, after retrieval. These counters describe the
retained substrate; global pruning does not prove a particular missing edge.
"""
from collections import Counter
import json
from pathlib import Path
import sqlite3


def retained_loss_report(database, baseline, scenarios):
    indexes, cases, stages = {}, [], Counter()
    scored = {row["id"]: row for row in baseline["scenarios"]}
    with sqlite3.connect(Path(database).resolve().as_uri() + "?mode=ro", uri=True) as connection:
        def report(identifier):
            row = connection.execute("SELECT body FROM reports WHERE id=?", (identifier,)).fetchone()
            if row is None:
                raise ValueError(f"Frozen report is unavailable: {identifier}")
            return json.loads(row[0])

        for scenario in scenarios:
            previous = scored[scenario["id"]]
            case = report(previous["case_id"])
            index_id = case["index_id"]
            if index_id != previous["index_id"] or case["view_id"] != previous["view_id"]:
                raise ValueError("Frozen case/index pins differ from baseline")
            if index_id not in indexes:
                index = report(index_id)
                coverage = index["coverage"]
                profiles = []
                for view in coverage.get("profile_views", []):
                    row = connection.execute("SELECT diagnostics FROM views WHERE id=?", (view,)).fetchone()
                    if row is None:
                        raise ValueError(f"Frozen producer view is unavailable: {view}")
                    diagnostics = json.loads(row[0])
                    workspace = diagnostics.get("workspace") or {}
                    profiles.append({"view_id": view, "provider": diagnostics.get("provider", {}).get("name"),
                        "coverage": diagnostics.get("coverage"), "call_coverage": diagnostics.get("call_coverage"),
                        "workspace_bounds": workspace.get("bounds"), "workspace_bounded": workspace.get("bounded"),
                        "omitted_by_reason": workspace.get("omitted_by_reason"),
                        "omitted_source_files": len(diagnostics.get("omitted_source_files") or []),
                        "partitions": workspace.get("partitions"), "calls": workspace.get("calls")})
                retained = len(index["links"])
                omitted = coverage.get("omitted_relationship_occurrences")
                indexes[index_id] = {"source_view": index["source_view"], "files": set(index["files"]),
                    "relationship_limit": coverage.get("relationship_limit"), "retained_relationships": retained,
                    "relationships_before_limit": retained + omitted if omitted is not None else None,
                    "omitted_relationships": omitted,
                    "retained_predicates": dict(Counter(link["predicate"] for link in index["links"])),
                    "omitted_predicates": coverage.get("omitted_relationship_predicates"),
                    "capture": coverage.get("capture"), "producers": profiles}
            policy = case["coverage"]["retrieval"]
            lexical = policy.get("lexical_discovery", {})
            lexical_rows = report(lexical["report_id"])["items"] if lexical.get("report_id") else []
            lexical_ranks = {row["path"]: row["rank"] for row in lexical_rows}
            candidates = {row["path"]: row for row in case["candidates"]}
            selected = {row["path"] for rows in case["slice"].values() for row in rows}
            missing = set(previous["harness_context"]["missing_required_paths"])
            paths = {}
            for path in scenario["required_paths"]:
                captured = path in indexes[index_id]["files"]
                candidate = candidates.get(path)
                if not captured:
                    stage = "not_captured"
                elif candidate is None:
                    stage = "not_a_candidate"
                elif path not in selected:
                    stage = "below_minimum_score" if candidate["score"] < policy["minimum_score"] else "selection_cap_or_policy"
                elif path in missing:
                    stage = "manifest_delivery"
                else:
                    stage = "manifest_delivered"
                stages[stage] += 1
                rank = lexical_ranks.get(path)
                paths[path] = {"first_loss_stage": stage, "captured": captured,
                    "lexical_rank": rank,
                    "lexical_admitted": rank <= lexical["candidate_limit"] if rank is not None else None,
                    "candidate_rank": candidate.get("rank") if candidate else None,
                    "score": candidate.get("score") if candidate else None,
                    "selected": path in selected, "manifest_delivered": path not in missing}
            cases.append({"id": scenario["id"], "case_id": previous["case_id"], "index_id": index_id,
                "paths": paths, "lexical_discovery": {k: lexical.get(k) for k in
                    ("report_id", "candidate_limit", "matched_files", "omitted_candidates", "candidate_limit_reached", "omitted_terms")},
                "join_discovery": policy.get("join_discovery"), "graph_expansion": policy.get("graph_expansion")})
    for index in indexes.values():
        index["captured_files"] = len(index.pop("files"))
    return {"first_loss_stage_counts": dict(sorted(stages.items())), "indexes": indexes, "scenarios": cases,
        "interpretation": "Path attribution is scorer-only. Global producer/index counters do not identify the cause of a particular missing relationship. Missing counters remain null."}


def investigation_summary(rows):
    successful = [row for row in rows if row.get("status") != "error"]
    return {"errors": len(rows) - len(successful), "scenarios": len(successful),
        "required_paths_inspected": sum(len(row["recovered_required_paths"]) for row in successful),
        "required_paths_digest_verified": sum(len(row["digest_verified_required_paths"]) for row in successful),
        "required_paths_beyond_manifest": sum(len(row["recovered_beyond_manifest"]) for row in successful),
        "all_required_paths_inspected": sum(not row["missing_required_paths"] for row in successful),
        "stop_reasons": dict(Counter(row["limit_reached"] for row in successful))}
