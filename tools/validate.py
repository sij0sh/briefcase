"""Validate the v1 planning contract, references and available evidence.

No repository analysis, network calls, production storage or model execution.
"""
import argparse
import datetime
import json
from pathlib import Path

from identity import digest, file_digest, load_json
from schema_profile import SchemaError, inspect_schema, validate as structural, _equal

ROOT = Path(__file__).resolve().parents[1]


class ContractError(ValueError):
    def __init__(self, code, message):
        self.code = code
        super().__init__(f"{code}: {message}")


def require(condition, code, message):
    if not condition:
        raise ContractError(code, message)


def _schema_check(value, filename):
    schema = load_json(ROOT / "contracts/v1" / filename)
    try:
        inspect_schema(schema)
        structural(value, schema)
    except SchemaError as exc:
        raise ContractError("SCHEMA", str(exc)) from exc


def context_contains(outer, inner):
    if outer == inner:
        return True
    return (outer["kind"] == "comparison" and inner["kind"] == "source"
            and inner["snapshot_id"] in outer["snapshot_ids"])


def validate_bundle(bundle):
    _schema_check(bundle, "knowledge.schema.json")
    tables = {key: {row["id"]: row for row in value}
              for key, value in bundle.items() if isinstance(value, list)}
    all_ids = [row["id"] for value in bundle.values() if isinstance(value, list) for row in value]
    require(len(all_ids) == len(set(all_ids)), "DUPLICATE_ID", "Record IDs must be globally unique")

    def get(table, key):
        require(key in tables[table], "UNKNOWN_REFERENCE", f"Missing {table} {key}")
        return tables[table][key]

    def check_context(context):
        if context["kind"] == "source":
            get("snapshots", context["snapshot_id"])
            build = get("build_contexts", context["build_context_id"])
            require(build["snapshot_id"] == context["snapshot_id"], "SCOPE", "Build belongs to another snapshot")
        elif context["kind"] == "runtime":
            get("runtime_contexts", context["runtime_context_id"])
        else:
            for sid in context["snapshot_ids"]:
                get("snapshots", sid)

    for snapshot in tables["snapshots"].values():
        paths = [f["path"] for f in snapshot["files"]]
        require(len(paths) == len(set(paths)), "DUPLICATE_PATH", "Snapshot paths must be unique")
        files = sorted(snapshot["files"], key=lambda f: f["path"])
        require(snapshot["manifest_digest"] == digest(files), "DIGEST", "Source manifest digest mismatch")
    for build in tables["build_contexts"].values():
        get("snapshots", build["snapshot_id"])
    for table in ["entity_versions", "runs", "input_packets", "authorities", "declaration_records", "unresolved_references", "projections", "assertions"]:
        for row in tables[table].values():
            check_context(row["context"])
    for runtime in tables["runtime_contexts"].values():
        start = datetime.datetime.fromisoformat(runtime["start"].replace("Z", "+00:00"))
        end = datetime.datetime.fromisoformat(runtime["end"].replace("Z", "+00:00"))
        require(start <= end, "INTERVAL", "Runtime observation interval is reversed")

    payloads = {}
    for artifact in tables["artifacts"].values():
        path = (ROOT / artifact["path"]).resolve()
        require(path.is_relative_to(ROOT), "ARTIFACT_PATH", "Artifact escaped package root")
        if artifact["availability"] != "available":
            continue
        require(path.is_file(), "UNAVAILABLE", f"Missing {artifact['path']}")
        data = path.read_bytes()
        require(file_digest(data) == artifact["digest"], "DIGEST", "Artifact content digest mismatch")
        payloads[artifact["id"]] = data
    for evidence in tables["evidence_refs"].values():
        artifact = get("artifacts", evidence["artifact_id"])
        if artifact["availability"] != "available":
            continue
        data = payloads[artifact["id"]]
        locator = evidence["locator"]
        if locator["kind"] == "span":
            require(0 <= locator["start"] < locator["end"] <= len(data), "ANCHOR", "Invalid UTF-8 byte span")
            try:
                data[:locator["start"]].decode("utf-8")
                data[locator["start"]:locator["end"]].decode("utf-8")
            except UnicodeDecodeError as exc:
                raise ContractError("ANCHOR", "Span splits a UTF-8 codepoint") from exc
        else:
            require(artifact["source_kind"] == "runtime", "ORIGIN", "Event locator requires runtime artifact")
            events = json.loads(data).get("events", [])
            require(any(x.get("id") == locator["event_id"] for x in events), "ANCHOR", "Unknown normalized event ID")

    for run in tables["runs"].values():
        get("analysis_configs", run["analysis_config_id"])
        c = run["coverage"]
        require(c["completed"] <= c["attempted"], "COVERAGE", "Completed exceeds attempted")
        if c["resolution_complete"]:
            require(c["completed"] == c["attempted"] and not c["unsupported_count"]
                    and not c["unresolved_count"] and run["status"] == "complete",
                    "COVERAGE", "Resolution-complete conflicts with run limitations")
    for declaration in tables["declaration_records"].values():
        authority = get("authorities", declaration["authority_id"])
        get("artifacts", declaration["artifact_id"])
        require(authority["context"] == declaration["context"], "AUTHORITY", "Declaration exceeds registered authority scope")

    registry = load_json(ROOT / "contracts/v1/predicate-registry.json")["predicates"]
    for row in tables["assertions"].values():
        proposition, context = row["proposition"], row["context"]
        name = proposition["predicate"]
        require(name in registry, "PREDICATE", f"Unregistered {name}")
        rule = registry[name]
        require(rule["status"] == "active", "PREDICATE", f"Reserved {name}")
        require(row["class"] in rule["classes"] and context["kind"] in rule["contexts"], "CLASS", "Predicate/class/context combination forbidden")
        subject = get("entity_versions", proposition["subject_id"])
        require(subject["kind"] in rule["subject_kinds"], "KIND", "Invalid subject kind")
        require(context_contains(context, subject["context"]), "SCOPE", "Subject outside assertion scope")
        object_value = proposition["object"]
        target = None
        if object_value["kind"] == "entity":
            target = get("entity_versions", object_value["entity_version_id"])
            require(target["kind"] in rule["object_kinds"], "KIND", "Invalid object kind")
            require(context_contains(context, target["context"]), "SCOPE", "Target outside assertion scope")
        else:
            require(rule["literal"], "KIND", "Literal object forbidden")
        if context["kind"] == "comparison" and target:
            require(subject["context"] != target["context"], "SCOPE", "Lineage must connect different version contexts")
        if row["class"] == "evidence":
            run = get("runs", row["origin"]["run_id"])
            require(run["context"] == context, "SCOPE", "Evidence run scope differs")
            require(run["status"] != "failed", "ORIGIN", "Failed run cannot publish evidence")
            for eid in row["evidence_ref_ids"]:
                evidence = get("evidence_refs", eid)
                artifact = get("artifacts", evidence["artifact_id"])
                if context["kind"] == "source":
                    files = get("snapshots", context["snapshot_id"])["files"]
                    require(any(f["path"] == artifact["path"] and f["digest"] == artifact["digest"] for f in files), "SCOPE", "Evidence bytes absent from source manifest")
            if name == "CALLS_STATIC":
                resolution = row.get("resolution")
                require(run["producer_kind"] == "semantic" and resolution
                        and resolution["method"] == "compiler" and resolution["status"] == "resolved",
                        "RESOLUTION", "Selected static calls require semantic callsite resolution")
                require(target and resolution["candidate_ids"] == [target["id"]], "RESOLUTION", "Exact call target and candidate differ")
                require(subject["identity_quality"] in {"compiler", "exact_mapping"}
                        and target["identity_quality"] in {"compiler", "exact_mapping"},
                        "RESOLUTION", "Syntax/heuristic symbol is not a resolved compiler target")
            if name in {"CALLS_OBSERVED", "CONNECTION_OBSERVED"}:
                require(run["producer_kind"] == "runtime", "ORIGIN", "Observed relation requires runtime producer")
        elif row["class"] == "derived":
            origin = get("derivations", row["origin"]["derivation_id"])
            require(origin["deterministic"], "CLASS", "Nondeterministic interpretation must be a Claim")
            require(set(row["supports"]) <= set(origin["input_assertion_ids"]), "INPUT_PACKET", "Derived support not declared as input")
        elif row["class"] == "claim":
            origin = get("inferences", row["origin"]["inference_id"])
            packet = get("input_packets", origin["packet_id"])
            require(packet["context"] == context, "SCOPE", "Inference packet scope differs")
            require(set(row["supports"]) <= set(packet["assertion_ids"]), "INPUT_PACKET", "Claim cites support outside actual packet")
            score = row.get("model_score", {})
            if score.get("semantics") == "calibrated":
                get("artifacts", score["calibration_artifact_id"])
        else:
            origin = get("declaration_records", row["origin"]["declaration_record_id"])
            require(origin["context"] == context, "SCOPE", "Declaration scope differs")
        for support in row.get("supports", []):
            parent = get("assertions", support)
            require(context_contains(context, parent["context"]), "SCOPE", "Support outside assertion scope")

    for derivation in tables["derivations"].values():
        for aid in derivation["input_assertion_ids"]:
            get("assertions", aid)
    for packet in tables["input_packets"].values():
        for aid in packet["assertion_ids"]:
            row = get("assertions", aid)
            require(context_contains(packet["context"], row["context"]), "SCOPE", "Packet input outside scope")
        require(packet["digest"] == digest({"context": packet["context"], "assertion_ids": packet["assertion_ids"]}), "DIGEST", "Input packet digest mismatch")
    for inference in tables["inferences"].values():
        get("input_packets", inference["packet_id"])

    taint = {}
    def includes_inference(aid, stack=()):
        require(aid not in stack, "SUPPORT_CYCLE", "Circular assertion support")
        if aid in taint:
            return taint[aid]
        row = get("assertions", aid)
        # Visit every dependency even when this row is already a Claim.
        children = [includes_inference(x, stack + (aid,)) for x in row.get("supports", [])]
        result = row["class"] == "claim" or any(children)
        if row["class"] == "claim":
            inference = get("inferences", row["origin"]["inference_id"])
            packet = get("input_packets", inference["packet_id"])
            # Input provenance itself must be acyclic, even when a claimed
            # rationale cites only a smaller witness subset.
            for input_id in packet["assertion_ids"]:
                includes_inference(input_id, stack + (aid,))
        if row["class"] == "derived":
            origin = get("derivations", row["origin"]["derivation_id"])
            all_inputs = [includes_inference(x, stack + (aid,)) for x in origin["input_assertion_ids"]]
            expected_origin = "includes_inference" if any(all_inputs) else "evidence_only"
            result = result or any(all_inputs)
            expected = "includes_inference" if result else "evidence_only"
            require(row["support_class"] == expected and origin["support_class"] == expected_origin,
                    "TAINT", "Derived support class hides inferred input")
            # A result whose pass used inferred inputs remains tainted even if
            # its selected witness does not cite that input directly.
        taint[aid] = result
        return taint[aid]
    for aid in tables["assertions"]:
        includes_inference(aid)

    for unresolved in tables["unresolved_references"].values():
        subject = get("entity_versions", unresolved["source_entity_id"])
        get("evidence_refs", unresolved["evidence_ref_id"])
        require(subject["context"] == unresolved["context"], "SCOPE", "Unresolved occurrence scope differs")
        for cid in unresolved["candidate_ids"]:
            candidate = get("entity_versions", cid)
            require(candidate["context"] == unresolved["context"], "SCOPE", "Candidate scope differs")
        if unresolved["status"] == "ambiguous":
            require(len(unresolved["candidate_ids"]) >= 2, "RESOLUTION", "Ambiguous occurrence needs multiple candidates")
    for projection in tables["projections"].values():
        for aid in projection["input_assertion_ids"]:
            row = get("assertions", aid)
            require(context_contains(projection["context"], row["context"]), "SCOPE", "Projection input outside scope")
            require(row["class"] in projection["eligible_classes"], "VIEW", "Projection includes forbidden class")
            if taint[aid] and "claim" not in projection["eligible_classes"]:
                raise ContractError("TAINT", "Evidence-only projection includes inferred support")
    return {"tables": tables, "includes_inference": taint}


def validate_batch(batch, bundle, capability, checked=None):
    _schema_check(batch, "batch.schema.json")
    checked = checked or validate_bundle(bundle)
    expected = batch["class"] + "_writer"
    require(capability == expected, "CAPABILITY", "Caller capability cannot submit this batch class")
    assertions = checked["tables"]["assertions"]
    rows = []
    for aid in batch["record_ids"]:
        require(aid in assertions, "UNKNOWN_REFERENCE", "Batch record missing from supplied payload")
        row = assertions[aid]
        require(row["class"] == batch["class"], "CLASS", "Typed batch contains another knowledge class")
        require(row["context"] == batch["context"], "SCOPE", "Batch contains another scope")
        rows.append(row)
    semantic_payload = {"context": batch["context"], "class": batch["class"],
                        "producer_id": batch["producer_id"], "records": sorted(rows, key=lambda r: r["id"])}
    artifacts = checked["tables"]["artifacts"]
    require(batch["payload_artifact_id"] in artifacts, "UNKNOWN_REFERENCE", "Batch payload artifact missing")
    artifact = artifacts[batch["payload_artifact_id"]]
    require(artifact["availability"] == "available", "UNAVAILABLE", "Batch payload must be retained")
    require(batch["payload_digest"] == artifact["digest"], "DIGEST", "Batch payload digest mismatch")
    payload = load_json(ROOT / artifact["path"])
    require(_equal(payload, semantic_payload), "DIGEST", "Retained transport payload differs from submitted records")


def validate_query(response, bundle, checked=None):
    _schema_check(response, "query.schema.json")
    checked = checked or validate_bundle(bundle)
    tables, taint = checked["tables"], checked["includes_inference"]
    selected = []
    for pid in response["projection_ids"]:
        require(pid in tables["projections"], "UNKNOWN_REFERENCE", "Unknown projection")
        p = tables["projections"][pid]
        require(p["context"] == response["context"], "SCOPE", "Query/projection scopes differ")
        selected.append(p)
    compatible = all(p["ledger_watermark"] == response["ledger_watermark"] for p in selected)
    if len(selected) > 1:
        compatible = compatible and all(set(p["input_assertion_ids"]) == set(selected[0]["input_assertion_ids"]) for p in selected)
    if response["outcome"] == "complete":
        require(compatible, "FRESHNESS", "Complete query combines incompatible projection inputs")
    elif not compatible:
        require(bool(response["limitations"]), "FRESHNESS", "Mixed views need an explicit limitation")
    allowed = {"evidence":{"evidence","derived"}, "intent":{"declaration"},
               "interpretation":{"claim","derived"}, "combined":{"evidence","derived","claim","declaration"}}[response["view"]]
    available = set().union(*(set(p["input_assertion_ids"]) for p in selected))
    for row in response["results"]:
        for aid in row["assertion_ids"]:
            require(aid in available and aid in tables["assertions"], "UNKNOWN_REFERENCE", "Result not in selected projections")
            assertion = tables["assertions"][aid]
            require(assertion["class"] in allowed, "VIEW", "Result class forbidden by requested view")
            if response["view"] == "evidence":
                require(not taint[aid], "TAINT", "Evidence result has inferred support")
        if row["conclusion"] in {"absent", "pass"}:
            require(response["resolution_complete"] and not response["truncated"], "COVERAGE", "Negative/compliance result lacks complete resolution")
            require(response["outcome"] == "complete", "COVERAGE", "Partial query cannot establish absence/compliance")
            require(bool(response["required_predicates"]), "COVERAGE", "Negative/compliance result requires named predicates")
            if response["context"]["kind"] == "source":
                snapshot = tables["snapshots"][response["context"]["snapshot_id"]]
                require(snapshot["capture_state"] == "complete", "COVERAGE", "Partial capture cannot establish absence/compliance")
            runs = [r for r in tables["runs"].values() if r["context"] == response["context"]]
            for predicate in response["required_predicates"]:
                require(any(r["coverage"]["resolution_complete"] and any(g["predicate"] == predicate and g["kind"] in {"exact_supported","sound_overapproximation"} for g in r["coverage"]["guarantees"]) for r in runs), "COVERAGE", "Predicate-specific absence guarantee missing")
    if response["truncated"]:
        require(bool(response.get("continuation_token")), "PAGINATION", "Truncated query needs pinned continuation")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--query", type=Path)
    parser.add_argument("--batch", type=Path)
    parser.add_argument("--capability", choices=["evidence_writer", "derived_writer", "claim_writer", "declaration_writer"])
    args = parser.parse_args()
    try:
        bundle = load_json(args.bundle)
        checked = validate_bundle(bundle)
        if args.query:
            validate_query(load_json(args.query), bundle, checked)
        if args.batch:
            require(args.capability is not None, "CAPABILITY", "Batch checks require explicit caller capability")
            validate_batch(load_json(args.batch), bundle, args.capability, checked)
        print(json.dumps({"status":"valid", "assertions":len(bundle["assertions"])}))
    except (ContractError, ValueError, OSError) as exc:
        print(json.dumps({"status":"invalid", "error":str(exc)}))
        raise SystemExit(1)


if __name__ == "__main__":
    main()
