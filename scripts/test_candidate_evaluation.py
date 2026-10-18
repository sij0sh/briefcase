import importlib.util
from pathlib import Path
import unittest

from test_task_investigation import Protocol

SPEC = importlib.util.spec_from_file_location("candidate_evaluator", Path(__file__).with_name("task-evaluate.py"))
evaluator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(evaluator)


class CandidateEvaluationTests(unittest.TestCase):
    def test_protocol_recovery_is_separate_from_manifest_recall_and_harness_use(self):
        protocol = Protocol()
        def call(operation, arguments):
            if operation == "brief":
                return {"id": "task-case:one", "case_id": "task-case:one", "view_id": "view:one", "index_id": "task-index:one",
                        "outcome": "partial", "slice": {"impl": [{"path": "src/a.rs", "rank": 1}]}, "not_covered": [], "candidates": [],
                        "coverage": {"delivery": {**{f"/slice/{role}": {"omitted": 0} for role in evaluator.ROLES},
                                                  "/not_covered": {"omitted": 0}}}}
            if operation == "agent_context":
                return {"kind": "agent_context", "case_id": "task-case:one", "files": [{"path": "src/a.rs", "ref": "/slice/impl/0"}],
                        "fetch_source": {"operation": "explain", "arguments": {"report_id": "task-case:one"}}}
            return protocol(operation, arguments)
        scenario = {"id": "protocol", "task": "storage change", "required_paths": protocol.paths}
        result = evaluator.evaluate_scenario(scenario, call, manifest_bytes=8000, verify_candidates=True, source_budget=2000)
        self.assertEqual(result["harness_context"]["required_path_recall"], 1 / 3)
        self.assertEqual(result["evidence_funnel"]["discovered"], 3)
        self.assertEqual(result["evidence_funnel"]["selected"], 1)
        self.assertEqual(result["evidence_funnel"]["source_bytes_verified"], 3)
        self.assertIsNone(result["evidence_funnel"]["harness_inspected"])
        self.assertEqual(result["candidate_source_fetch"]["response_budget"], 2000)
        source_calls = [args for op, args in protocol.calls if op == "explain"]
        self.assertTrue(all(args["max_bytes"] == 2000 for args in source_calls))


if __name__ == "__main__":
    unittest.main()
