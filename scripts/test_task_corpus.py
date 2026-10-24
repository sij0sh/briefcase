"""Corpus fixtures are deterministic, held out by repository, and their oracles discriminate."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import subprocess

from task_corpus import FAMILIES, corpus, split

spec = importlib.util.spec_from_file_location("corpus_evaluate", Path(__file__).with_name("task-corpus-evaluate.py"))
evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluate)


class CorpusTest(unittest.TestCase):
    def test_generation_is_deterministic_and_variants_differ(self):
        self.assertEqual(corpus(2, seed=3), corpus(2, seed=3))
        for family in FAMILIES:
            a, b = corpus(2, seed=3, families=[family])
            self.assertNotEqual((a["task"], sorted(a["files"])), (b["task"], sorted(b["files"])), family)

    def test_tasks_carry_no_answer_paths(self):
        for fixture in corpus(3):
            for path in fixture["answer_key"]["required_paths"]:
                self.assertNotIn(path, fixture["task"])
                self.assertIn(path, fixture["files"])

    def test_held_out_split_is_per_repository_and_stable(self):
        fixtures = corpus(8)
        self.assertEqual({f["split"] for f in fixtures}, {"dev", "held_out"})
        self.assertTrue(all(f["split"] == split(f["id"]) for f in fixtures))

    def test_every_oracle_fails_before_and_passes_with_the_reference_patch(self):
        for fixture in corpus(1):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for path, text in fixture["files"].items():
                    (root / "repo" / path).parent.mkdir(parents=True, exist_ok=True)
                    (root / "repo" / path).write_text(text)
                script = root / "check.py"
                script.write_text(fixture["answer_key"]["check"])
                self.assertFalse(evaluate.check(script, root / "repo")["passed"], fixture["id"])
                for path, text in fixture["answer_key"]["patch"].items():
                    (root / "repo" / path).write_text(text)
                self.assertTrue(evaluate.check(script, root / "repo")["passed"], fixture["id"])

    def test_summary_keeps_errors_in_the_denominator(self):
        ok = {"id": "a", "family": "f", "split": "dev", "oracle": {"valid": True}, "driver": {"calls": 4},
              "required": [{"candidate": True, "selected": False, "verified_source": True}], "outcome": {"state": "not_run"}}
        summary = evaluate.summarize([ok, {"id": "b", "family": "f", "split": "held_out", "error": "timeout"}])
        self.assertEqual((summary["all"]["fixtures"], summary["all"]["errors"]), (2, 1))
        self.assertEqual(summary["split:held_out"]["fixtures_all_required_verified"], 0)
        self.assertEqual(summary["family:f"]["verified_source_recall"], 1)

    def test_scaling_requires_prerequisite_lifecycle_repair(self):
        fixture = corpus(1, families=["prerequisite_migration"])[0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for path, text in fixture["files"].items():
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_text(text)
            script = root.parent / (root.name + ".check.py")
            self.addCleanup(script.unlink, missing_ok=True)
            script.write_text(fixture["answer_key"]["check"])
            for path, text in fixture["answer_key"]["patch"].items():
                if not path.endswith("/lifecycle.py"):
                    (root / path).write_text(text)
            self.assertFalse(evaluate.check(script, root)["passed"],
                             "adding concurrency without repairing the shared lifecycle must fail")
            for path, text in fixture["answer_key"]["patch"].items():
                (root / path).write_text(text)
            self.assertTrue(evaluate.check(script, root)["passed"])

    def test_distinct_lifecycles_allow_shared_pure_logic_but_reject_lost_queue_state(self):
        fixture = corpus(1, families=["intentional_separation"])[0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for path, text in {**fixture["files"], **fixture["answer_key"]["patch"]}.items():
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_text(text)
            script = root.parent / (root.name + ".check.py")
            self.addCleanup(script.unlink, missing_ok=True)
            script.write_text(fixture["answer_key"]["check"])
            durable = next(root.glob("*/durable.py"))
            package = durable.parent
            (package / "shared.py").write_text("def transform(values):\n    return [v * 2 for v in values]\n")
            interactive = package / "interactive.py"
            interactive.write_text("from .shared import transform\n" + interactive.read_text().replace(
                "[value * 2 for value in values[:max(0, limit)]]", "transform(values[:max(0, limit)])"))
            durable.write_text("from .shared import transform\n" + durable.read_text().replace(
                "[value * 2 for value in self.pending[:limit]]", "transform(self.pending[:limit])"))
            self.assertTrue(evaluate.check(script, root)["passed"], "a valid design need not equal the gold patch")
            durable.write_text(durable.read_text().replace("del self.pending[:limit]", "self.pending.clear()"))
            self.assertFalse(evaluate.check(script, root)["passed"], "shared transformation cannot erase durable state")

    def test_harness_failure_and_timeout_cannot_be_reported_as_verified(self):
        failed = SimpleNamespace(returncode=7, stdout="", stderr="agent failed")
        with patch.object(evaluate.subprocess, "run", return_value=failed), patch.object(evaluate, "check") as check:
            result = evaluate.harness_outcome("agent", Path("."), Path("check.py"), {}, 1)
        self.assertEqual(result["state"], "error")
        self.assertEqual(result["harness_exit"], 7)
        check.assert_not_called()
        with patch.object(evaluate.subprocess, "run", side_effect=subprocess.TimeoutExpired("agent", 1)):
            self.assertEqual(evaluate.harness_outcome("agent", Path("."), Path("check.py"), {}, 1)["state"], "error")

    def test_successful_harness_still_needs_passing_behavior(self):
        with patch.object(evaluate.subprocess, "run", return_value=SimpleNamespace(returncode=0)), \
                patch.object(evaluate, "check", return_value={"passed": False, "stderr": "contract broken"}):
            result = evaluate.harness_outcome("agent", Path("."), Path("check.py"), {}, 1)
        self.assertEqual(result["state"], "verified_fail")


if __name__ == "__main__":
    unittest.main()
