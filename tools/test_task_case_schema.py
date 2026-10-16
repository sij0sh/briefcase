"""Offline v1 case/brief wire compatibility checks, independent of Rust."""
import copy
import json
from pathlib import Path
import unittest

from schema_profile import SchemaError, inspect_schema, validate

ROOT = Path(__file__).resolve().parents[1]


class TaskCaseSchemaTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.case_schema = json.loads((ROOT / "contracts/v1/task-case.schema.json").read_text())
        cls.brief_schema = json.loads((ROOT / "contracts/v1/task-brief.schema.json").read_text())

    def case(self):
        roles = ("impl", "test", "config", "script", "build", "install", "doc", "adr", "plan", "fixture", "generated")
        return {"schema_version": "1.0", "kind": "case", "method": "task-context-v1", "id": "task-case:fixture",
                "outcome": "partial", "outcome_scope": "retained_evidence", "task": "inspect src/a.rs", "duty": "test",
                "request": {"task": "inspect src/a.rs", "duty": "test", "target": "src/a.rs", "depth": 1, "view": "latest",
                            "scope": {"deployment_view": None, "runtime_environment": None, "runtime_from": None, "runtime_to": None}},
                "view_id": "view:fixture", "index_id": "task-index:fixture", "pinned_views": ["view:fixture"],
                "anchors": {"status": "resolved", "candidates": ["src/a.rs"], "paths": ["src/a.rs"]},
                "slice": {role: [] for role in roles}, "neighborhood": [], "history": [], "sessions": [], "errors": [],
                "joins": [], "cochange": [], "external_touchpoints": [], "lexical_candidates": [],
                "not_covered": [{"area": "executed_test_coverage", "dimension": "verification", "reason": "Not executed"}],
                "coverage": {"completeness": {"anchors": "established", "slice": "partial", "neighborhood": "partial",
                                              "history": "not_required", "sessions": "not_required", "gap_inventory": "established",
                                              "verification": "unknown"}}, "readiness": {}, "provenance": {"retraction_epoch": 0}}

    def test_owned_schemas_use_the_supported_offline_vocabulary(self):
        inspect_schema(self.case_schema)
        inspect_schema(self.brief_schema)
        validate(self.case(), self.case_schema)

    def test_presentation_fields_do_not_enter_the_retained_case(self):
        for key in ("limit", "max_bytes"):
            case = self.case()
            case["request"][key] = 2
            with self.assertRaises(SchemaError):
                validate(case, self.case_schema)
        case = self.case()
        case["continue_with"] = {}
        with self.assertRaises(SchemaError):
            validate(case, self.case_schema)

    def test_unknown_gap_areas_dimensions_and_verification_claims_are_rejected(self):
        for key, value in (("area", "unassigned"), ("dimension", "unassigned")):
            case = self.case()
            case["not_covered"][0][key] = value
            with self.assertRaises(SchemaError):
                validate(case, self.case_schema)
        case = self.case()
        case["coverage"]["completeness"]["verification"] = "established"
        with self.assertRaises(SchemaError):
            validate(case, self.case_schema)

    def test_new_and_legacy_briefs_share_the_v1_schema(self):
        brief = self.case()
        brief.pop("request")
        brief.update(kind="brief", case_id=brief["id"], report_id=brief["id"], instruction="Retained scope only", continue_with=None)
        artifact = {"path": "src/a.rs", "artifact_id": "artifact:a", "digest": "sha256:a", "role": "impl", "language": "rust",
                    "bytes": 0, "selection": "resolved_anchor", "reached_by": {"depth": 0, "via_link": None}}
        brief["slice"]["impl"] = [artifact]
        validate(brief, self.brief_schema)
        legacy = copy.deepcopy(brief)
        for key in ("case_id", "outcome_scope"):
            legacy.pop(key)
        legacy["slice"]["impl"][0].pop("reached_by")
        legacy["not_covered"][0].pop("dimension")
        legacy["coverage"].pop("completeness")
        legacy["id"] = legacy["report_id"] = "task-brief:old"
        validate(legacy, self.brief_schema)


if __name__ == "__main__":
    unittest.main()
