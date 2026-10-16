import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import contextlib
import io
import json
import tempfile

spec = importlib.util.spec_from_file_location("task_evaluate", Path(__file__).with_name("task-evaluate.py"))
task_evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(task_evaluate)


class TaskEvaluationTests(unittest.TestCase):
    def test_followup_files_measure_missed_context(self):
        packet = {"id": "task-brief:test", "view_id": "view:1", "outcome": "partial", "slice": {},
                  "not_covered": [{"area": "external_installations"}],
                  "coverage": {"delivery": {"/not_covered": {"omitted": 0}}}}
        for role in task_evaluate.ROLES:
            packet["slice"][role] = []
            packet["coverage"]["delivery"][f"/slice/{role}"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "src/a.rs"}]
        result = task_evaluate.evaluate_scenario(
            {"id": "rename", "task": "rename a", "followup_paths": ["src/a.rs", "scripts/install"],
             "required_gap_areas": ["external_installations"]}, lambda *_: packet)
        self.assertEqual(result["status"], "miss")
        self.assertEqual(result["required_path_recall"], 0.5)
        self.assertEqual(result["missing_paths"], ["scripts/install"])

    def test_omitted_sections_are_retrieved_without_recomputing_the_task(self):
        calls = []

        def call(operation, args):
            calls.append((operation, args))
            offset = args["offset"]
            return {"items": [{"path": str(offset)}], "page": {"total": 2,
                    "next_offset": 1 if offset == 0 else None}}

        packet = {"id": "task-brief:test", "slice": {"test": [{"path": "0"}]},
                  "coverage": {"delivery": {"/slice/test": {"omitted": 1}}}}
        rows = task_evaluate.packet_section(packet, "/slice/test", call)
        self.assertEqual(rows, [{"path": "0"}, {"path": "1"}])
        self.assertEqual([c[1]["offset"] for c in calls], [0, 1])
        self.assertTrue(all(c[0] == "page" for c in calls))

    def test_cursor_gaps_and_retractions_fail_evaluation(self):
        packet = {"id": "task-brief:test"}
        with self.assertRaisesRegex(ValueError, "contiguous"):
            task_evaluate.packet_section(packet, "/slice/test", lambda *_:
                {"items": [{}], "page": {"total": 20, "next_offset": 10}})
        with self.assertRaisesRegex(ValueError, "Retractions"):
            task_evaluate.packet_section(packet, "/slice/test", lambda *_:
                {"items": [], "page": {"total": 0}, "historical_packet": True})

    def test_missing_labels_and_budget_notices_are_not_successes(self):
        with self.assertRaises(ValueError):
            task_evaluate.evaluate_scenario({"id": "unlabelled", "task": "rename"}, lambda *_: {})
        with self.assertRaisesRegex(ValueError, "omitted"):
            task_evaluate.packet_section({"id": "task-brief:test"}, "/slice/test", lambda *_:
                {"truncated": True, "report_id": "task-brief:test"})

    def test_false_complete_counts_missed_authored_paths_and_excludes_errors(self):
        packet = {"id": "task-case:test", "case_id": "task-case:test", "outcome": "complete",
                  "slice": {role: [] for role in task_evaluate.ROLES}, "not_covered": [],
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "src/a.rs"}]
        scenario = {"id": "rename", "task": "rename", "required_paths": ["src/a.rs", "src/b.rs"]}
        incorrect = task_evaluate.evaluate_scenario(scenario, lambda *_: packet)
        self.assertTrue(incorrect["false_complete"])
        self.assertEqual(incorrect["case_id"], "task-case:test")
        packet["outcome"] = "partial"
        partial = task_evaluate.evaluate_scenario(scenario, lambda *_: packet)
        self.assertFalse(partial["false_complete"])
        metrics = task_evaluate.false_complete_metrics([incorrect, partial, {"status": "error"}])
        self.assertEqual(metrics["false_complete_rate"], 0.5)
        self.assertEqual(metrics["false_complete_given_complete_rate"], 1.0)
        self.assertEqual(metrics["scored_scenarios"], 2)
        packet["outcome"] = "complete"
        scenario["required_paths"] = ["src/a.rs"]
        self.assertFalse(task_evaluate.evaluate_scenario(scenario, lambda *_: packet)["false_complete"])

    def test_new_case_ids_page_and_missing_outcomes_are_errors(self):
        items = task_evaluate.packet_section({"case_id": "task-case:new"}, "/slice/test", lambda *_:
            {"items": [], "page": {"total": 0, "next_offset": None}})
        self.assertEqual(items, [])
        with self.assertRaisesRegex(ValueError, "outcome"):
            task_evaluate.evaluate_scenario({"id": "x", "task": "x", "required_paths": ["x"]}, lambda *_: {})
        self.assertIsNone(task_evaluate.false_complete_metrics([{"status": "error"}])["false_complete_rate"])

    def test_scenarios_and_deprecated_cases_cli_and_input_keys(self):
        packet = {"case_id": "task-case:test", "id": "task-case:test", "outcome": "complete",
                  "slice": {role: [] for role in task_evaluate.ROLES}, "not_covered": [],
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "a"}]
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "scenarios.json"
            for option, key in [("--scenarios", "scenarios"), ("--cases", "cases")]:
                source.write_text(json.dumps({key: [{"id": "x", "task": "x", "required_paths": ["a"]}]}))
                output = io.StringIO()
                with patch("sys.argv", ["task-evaluate.py", option, str(source)]), \
                     patch.object(task_evaluate, "execute", return_value=packet), contextlib.redirect_stdout(output):
                    self.assertEqual(task_evaluate.main(), 0)
                report = json.loads(output.getvalue())
                self.assertEqual(report["scenarios"][0]["case_id"], "task-case:test")
                self.assertEqual(report["false_complete_rate"], 0)


if __name__ == "__main__":
    unittest.main()
