"""Corpus fixtures are deterministic, held out by repository, and their oracles discriminate."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

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


if __name__ == "__main__":
    unittest.main()
