"""Independent wire-contract checks. These do not execute the Rust engine."""
import copy
import json
from pathlib import Path
import unittest

from schema_profile import SchemaError, inspect_schema, validate

ROOT = Path(__file__).resolve().parents[1]


class ImpactContractSchemaTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.schema = json.loads((ROOT / "contracts/v1/architectural-impact-contract.schema.json").read_text())

    def document(self):
        evidence = {"report_id": "task-index:fixture", "ref": "/files/src~1auth.py"}
        return {
            "schema_version": "1.0", "kind": "architectural_impact_contract", "method": "architectural-impact-v1",
            "report_id": "impact-contract:fixture", "mode": "pre", "task": "change token encoding",
            "case_id": "task-case:fixture", "view_id": "view:fixture", "index_id": "task-index:fixture",
            "pinned_views": ["view:fixture"],
            "source": {"repository_id": "repository:fixture", "base_view": None, "candidate_view": "view:fixture",
                       "base_snapshot": None, "candidate_snapshot": "snapshot:fixture"},
            "request": {"targets": ["src/auth.py"], "depth": 2, "max_nodes": 4096},
            "sides": [{"side": "candidate", "view_id": "view:fixture", "index_id": "task-index:fixture",
                       "snapshot_id": "snapshot:fixture", "repository_id": "repository:fixture", "captured_files": 1,
                       "capture_complete": True, "dependencies_complete_for_producer_scope": False,
                       "coverage_ref": {**evidence, "ref": "/coverage"}}],
            "changes": [{"path": "src/auth.py", "state": "planned", "before_digest": "sha256:fixture",
                         "after_digest": None, "outside_initial_slice": False, "evidence_refs": [evidence]}],
            "resolutions": [{"target": "src/auth.py", "status": "resolved", "candidates": ["src/auth.py"],
                             "paths": ["src/auth.py"]}],
            "affected_components": [{"id": "src/auth.py", "path": "src/auth.py", "kind": "file", "side": "candidate",
                                     "view_id": "view:fixture", "index_id": "task-index:fixture",
                                     "relationship": "direct_change", "depth": 0,
                                     "status": "supported_in_retained_scope", "evidence_refs": [evidence]}],
            "relationship_changes": [], "correspondence": [], "constraints": [], "findings": [],
            "verification": [{"id": "test:tests/test_auth.py", "side": "candidate", "path": "tests/test_auth.py",
                              "basis": "static_test_association", "requirement": "suggested",
                              "execution_status": "not_executed", "description": "Inspect and run externally",
                              "evidence_refs": [evidence]}],
            "governing_decisions": [], "governing_decisions_status": "not_connected",
            "unknowns": [{"area": "verification", "side": None, "status": "unknown",
                          "reason": "Tests were not executed", "evidence_refs": []}],
            "assessment": "partial", "assessment_scope": "retained_evidence_for_captured_source_only",
            "coverage": {"granularity": "captured files", "source_delta": "prospective targets",
                         "graph": "bounded directional traversal", "comparison": "observed relationships only",
                         "verification": "not_executed", "decision_registry": "not_connected"},
            "retraction_epoch": 0, "provenance": {"implementation_digest": "sha256:code", "schema_digest": "sha256:schema"},
            "instruction": "Page retained evidence before external validation",
        }

    def test_schema_uses_supported_local_vocabulary(self):
        inspect_schema(self.schema)
        validate(self.document(), self.schema)

    def test_post_shape_retains_base_and_candidate_identities(self):
        doc = self.document()
        doc["mode"] = "post"
        doc["source"].update(base_view="view:base", base_snapshot="snapshot:base")
        doc["changes"][0].update(state="modified", after_digest="sha256:candidate")
        doc["relationship_changes"] = [{"state": "observed_only_in_candidate", "predicate": "IMPORTS",
            "from": "api.py", "to": "src/auth.py", "class": "evidence",
            "evidence_refs": [{"report_id": "task-index:candidate", "ref": "task-link:import"}]}]
        validate(doc, self.schema)

    def test_static_tests_cannot_be_promoted_to_executed_results(self):
        for status in ("passed", "failed", "verified", "unknown"):
            doc = self.document()
            doc["verification"][0]["execution_status"] = status
            with self.assertRaises(SchemaError):
                validate(doc, self.schema)

    def test_report_only_rules_do_not_create_mandatory_test_obligations(self):
        for field, value in (("requirement", "mandatory"), ("basis", "approved_adr")):
            doc = self.document()
            doc["verification"][0][field] = value
            with self.assertRaises(SchemaError):
                validate(doc, self.schema)
        doc = self.document()
        doc["constraints"] = [{"side": "candidate", "rule_id": "isolation", "status": "unknown",
            "mode": "report-only", "detail": {"from": "api", "to": "auth"}, "evidence_refs": []}]
        validate(doc, self.schema)
        doc["constraints"][0]["mode"] = "enforce"
        with self.assertRaises(SchemaError):
            validate(doc, self.schema)

    def test_unconnected_registry_cannot_claim_approval(self):
        doc = self.document()
        doc["governing_decisions"] = [{"id": "ADR-012", "approved": True}]
        with self.assertRaises(SchemaError):
            validate(doc, self.schema)
        doc = self.document()
        doc["governing_decisions_status"] = "approved"
        with self.assertRaises(SchemaError):
            validate(doc, self.schema)

    def test_source_references_require_their_own_report_pin(self):
        for invalid in ({"ref": "/changes/0"}, {"report_id": "", "ref": "/changes/0"},
                        {"report_id": "task-index:fixture", "ref": ""}):
            doc = self.document()
            doc["changes"][0]["evidence_refs"] = [invalid]
            with self.assertRaises(SchemaError):
                validate(doc, self.schema)

    def test_missing_evidence_cannot_be_claimed_as_a_proven_relationship_removal(self):
        doc = self.document()
        doc["relationship_changes"] = [{"state": "removed", "predicate": "IMPORTS", "from": "api", "to": "auth",
            "class": "evidence", "evidence_refs": []}]
        with self.assertRaises(SchemaError):
            validate(doc, self.schema)
        for field, value in (("assessment", "complete"), ("assessment_scope", "runtime_truth")):
            doc = self.document()
            doc[field] = value
            with self.assertRaises(SchemaError):
                validate(doc, self.schema)

    def test_delivery_metadata_does_not_enter_retained_identity(self):
        for field in ("delivery", "freshness", "budget", "continue_with"):
            doc = self.document()
            doc[field] = {}
            with self.assertRaises(SchemaError):
                validate(doc, self.schema)
        for field in ("limit", "max_bytes"):
            doc = self.document()
            doc["request"][field] = 100
            with self.assertRaises(SchemaError):
                validate(doc, self.schema)

    def test_unknowns_and_suggested_checks_are_preserved_in_the_wire_shape(self):
        doc = self.document()
        doc["unknowns"].append({"area": "correspondence", "side": None, "status": "unknown",
                                "reason": "Ambiguous move; identities remain separate", "evidence_refs": []})
        validate(copy.deepcopy(doc), self.schema)
        for field in ("status", "reason", "evidence_refs"):
            invalid = copy.deepcopy(doc)
            invalid["unknowns"][0].pop(field)
            with self.assertRaises(SchemaError):
                validate(invalid, self.schema)


if __name__ == "__main__":
    unittest.main()
