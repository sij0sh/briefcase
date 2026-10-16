import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("task_evaluate", Path(__file__).with_name("task-evaluate.py"))
task_evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(task_evaluate)


class TaskEvaluationTests(unittest.TestCase):
    def test_followup_files_measure_missed_context(self):
        packet = {"id": "task-brief:test", "view_id": "view:1", "slice": {},
                  "not_covered": [{"area": "external_installations"}],
                  "coverage": {"delivery": {"/not_covered": {"omitted": 0}}}}
        for role in task_evaluate.ROLES:
            packet["slice"][role] = []
            packet["coverage"]["delivery"][f"/slice/{role}"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "src/a.rs"}]
        result = task_evaluate.evaluate_case(
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
            task_evaluate.evaluate_case({"id": "unlabelled", "task": "rename"}, lambda *_: {})
        with self.assertRaisesRegex(ValueError, "omitted"):
            task_evaluate.packet_section({"id": "task-brief:test"}, "/slice/test", lambda *_:
                {"truncated": True, "report_id": "task-brief:test"})


if __name__ == "__main__":
    unittest.main()
