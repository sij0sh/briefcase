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
                with patch("sys.argv", ["task-evaluate.py", option, str(source), "--skip-manifest"]), \
                     patch.object(task_evaluate, "execute", return_value=packet), contextlib.redirect_stdout(output):
                    self.assertEqual(task_evaluate.main(), 0)
                report = json.loads(output.getvalue())
                self.assertEqual(report["scenarios"][0]["case_id"], "task-case:test")
                self.assertEqual(report["false_complete_rate"], 0)


    def test_ranked_recall_and_actual_manifest_delivery_are_scored_separately(self):
        packet = {"case_id": "task-case:ranked", "id": "task-case:ranked", "outcome": "partial",
                  "slice": {role: [] for role in task_evaluate.ROLES},
                  "not_covered": [{"area": "producer_boundary", "count": 2000},
                                  {"area": "producer_boundary_scope", "actionable": False}],
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "a", "rank": 1, "bytes": 20}, {"path": "b", "rank": 2, "bytes": 40}]
        manifest = {"kind": "agent_context", "case_id": "task-case:ranked", "files": [
            {"path": "a", "fetch": {"operation": "explain", "arguments": {"report_id": "task-case:ranked", "ref": "/slice/impl/0"}}}],
            "continue_with": {"operation": "page"}, "coverage": {"delivery": {"files": {"omitted": 1}}}}
        calls = []
        def call(operation, arguments):
            calls.append((operation, arguments))
            return manifest if operation == "agent_context" else packet
        result = task_evaluate.evaluate_scenario({"id": "ranked", "task": "inspect a", "required_paths": ["a", "b"]}, call,
                                                manifest_bytes=8000, exclude_facets=("history",))
        self.assertEqual(result["recall_at_k"]["5"], 1)
        self.assertEqual(result["required_path_ranks"], {"a": 1, "b": 2})
        self.assertEqual(result["harness_context"]["required_path_recall"], 0.5)
        self.assertEqual(result["harness_context"]["omitted_files"], 1)
        self.assertEqual(result["actionable_gap_rows"], 1)
        self.assertEqual(result["producer_boundary_occurrences"], 2000)
        self.assertEqual(calls[0][1]["exclude_facets"], ["history"])
        self.assertNotIn("limit", next(args for operation, args in calls if operation == "agent_context"))
        self.assertEqual(result["selected_source_bytes"], 60)
        manifest["case_id"] = "task-case:other"
        with self.assertRaisesRegex(ValueError, "another retained case"):
            task_evaluate.evaluate_scenario({"id": "x", "task": "x", "required_paths": ["a"]}, call, manifest_bytes=8000)

    def test_manifest_byte_limit_and_unpinned_fetches_fail_evaluation(self):
        packet = {"case_id": "task-case:x", "id": "task-case:x", "outcome": "partial", "not_covered": [],
                  "slice": {role: [] for role in task_evaluate.ROLES},
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        manifest = {"kind": "agent_context", "case_id": "task-case:x", "files": [{"path": "a", "fetch": {}}], "continue_with": {}}
        call = lambda op, _: manifest if op == "agent_context" else packet
        scenario = {"id": "x", "task": "x", "required_paths": ["a"]}
        with self.assertRaisesRegex(ValueError, "not pinned"):
            task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)
        manifest["detail"] = "é" * 20000
        with self.assertRaisesRegex(ValueError, "UTF-8 byte budget"):
            task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)

    def test_shared_fetches_and_separate_anchor_preserve_harness_recall(self):
        packet = {"case_id": "task-case:v2", "id": "task-case:v2", "outcome": "partial", "not_covered": [],
                  "slice": {role: [] for role in task_evaluate.ROLES},
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "a", "rank": 1}, {"path": "b", "rank": 2}]
        manifest = {"schema_version": "2.0", "kind": "agent_context", "case_id": "task-case:v2",
                    "anchor": {"path": "a", "ref": "/slice/impl/0"},
                    "files": [{"path": "b", "ref": "/slice/impl/1", "source_offset": 42}],
                    "fetch_source": {"operation": "explain", "arguments": {"report_id": "task-case:v2"}},
                    "coverage": {"delivery": {"files": {"omitted": 0}}}, "continue_with": None}
        call = lambda operation, _: manifest if operation == "agent_context" else packet
        scenario = {"id": "v2", "task": "repair a", "required_paths": ["a", "b"]}
        result = task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)
        self.assertEqual(result["harness_context"]["required_path_recall"], 1)
        self.assertEqual(result["harness_context"]["delivered_files"], 2)
        self.assertEqual(result["harness_context"]["supplemental_files"], 1)
        self.assertEqual(result["harness_context"]["anchor_path"], "a")
        self.assertEqual(task_evaluate.manifest_source_fetch(manifest, manifest["files"][0]), {
            "operation": "explain", "arguments": {"report_id": "task-case:v2", "ref": "/slice/impl/1", "source_offset": 42}})
        for offset in (-1, True, "42"):
            manifest["files"][0]["source_offset"] = offset
            with self.assertRaisesRegex(ValueError, "not pinned"):
                task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)
        manifest["files"][0]["source_offset"] = 42
        manifest["fetch_source"]["arguments"]["report_id"] = "task-case:other"
        with self.assertRaisesRegex(ValueError, "not pinned"):
            task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)


    def test_harness_framing_matches_the_actual_adapter_sources(self):
        import re
        root = Path(__file__).resolve().parents[1]
        for harness, path in [("pi", root / "extensions/briefcase-pi.ts"),
                              ("claude-code", root / "src/architecture/adoption.rs")]:
            literal = re.search(r'const CONTEXT_PREFIX[^=]*=\s*("(?:[^"\\]|\\.)*")', path.read_text()).group(1)
            self.assertEqual(task_evaluate.FRAMING[harness], json.loads(literal))


if __name__ == "__main__":
    unittest.main()
