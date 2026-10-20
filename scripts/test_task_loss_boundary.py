import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

from task_loss_boundary import investigation_summary, retained_loss_report


class LossBoundaryTests(unittest.TestCase):
    def test_first_loss_stages_and_saturation_are_independent(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "frozen.db"
            paths = ["missing", "undiscovered", "low", "cut", "transport", "delivered"]
            candidates = [{"path": path, "score": 10 if path == "low" else 50, "rank": index}
                          for index, path in enumerate(paths[2:], 1)]
            case = {"view_id": "view:1", "index_id": "task-index:one", "candidates": candidates,
                "slice": {"impl": [{"path": "transport"}, {"path": "delivered"}]},
                "coverage": {"retrieval": {"minimum_score": 24, "lexical_discovery": {
                    "report_id": "lexical:one", "candidate_limit": 1}}}}
            index = {"source_view": "view:1", "files": {path: {} for path in paths[1:]},
                "links": [{"predicate": "IMPORTS"}], "coverage": {"profile_views": ["view:2"],
                "relationship_limit": 1, "omitted_relationship_occurrences": 2,
                "omitted_relationship_predicates": {"DECLARES": 2}}}
            with sqlite3.connect(database) as connection:
                connection.executescript("CREATE TABLE reports(id TEXT,body TEXT); CREATE TABLE views(id TEXT,diagnostics TEXT);")
                connection.executemany("INSERT INTO reports VALUES (?,?)", [(key, json.dumps(value)) for key, value in (
                    ("task-case:one", case), ("task-index:one", index),
                    ("lexical:one", {"items": [{"path": "low", "rank": 1}, {"path": "undiscovered", "rank": 2}]}))])
                connection.execute("INSERT INTO views VALUES (?,?)", ("view:2", json.dumps({"workspace": {
                    "bounded": False, "bounds": {"max_files": 20000}, "omitted_by_reason": {"outside_partitions": 3}}})))
            baseline = {"scenarios": [{"id": "task", "case_id": "task-case:one", "view_id": "view:1",
                "index_id": "task-index:one", "harness_context": {"missing_required_paths": paths[:-1]}}]}
            before = database.read_bytes()
            result = retained_loss_report(database, baseline, [{"id": "task", "required_paths": paths}])
            self.assertEqual(database.read_bytes(), before)
            self.assertEqual(result["first_loss_stage_counts"], {stage: 1 for stage in (
                "not_captured", "not_a_candidate", "below_minimum_score", "selection_cap_or_policy", "manifest_delivery", "manifest_delivered")})
            observed = result["indexes"]["task-index:one"]
            self.assertEqual(observed["relationships_before_limit"], 3)
            self.assertFalse(observed["producers"][0]["workspace_bounded"])
            tail = result["scenarios"][0]["paths"]["undiscovered"]
            self.assertEqual(tail["lexical_rank"], 2)
            self.assertFalse(tail["lexical_admitted"])

    def test_errors_and_inspection_are_not_counted_as_digest_verification(self):
        result = investigation_summary([{"status": "error"}, {"recovered_required_paths": ["a", "b"],
            "digest_verified_required_paths": ["a"], "recovered_beyond_manifest": ["b"],
            "missing_required_paths": [], "limit_reached": "calls"}])
        self.assertEqual(result["errors"], 1)
        self.assertEqual(result["required_paths_inspected"], 2)
        self.assertEqual(result["required_paths_digest_verified"], 1)


if __name__ == "__main__":
    unittest.main()
