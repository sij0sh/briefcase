import tempfile
import unittest
from pathlib import Path

from task_baseline import rg_read_baseline


class RgBaselineTests(unittest.TestCase):
    def test_ranks_files_by_matched_task_terms_within_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "hit.ts").write_text("rollback capability adapter\nrollback again\n")
            (root / "miss.ts").write_text("unrelated content\n")
            result = rg_read_baseline(
                {"id": "x", "task": "rollback capability", "required_paths": ["hit.ts"]},
                root, max_calls=300, max_bytes=2_000_000, max_seconds=60)
            self.assertEqual(result["recovered_required_paths"], ["hit.ts"])
            self.assertEqual(result["limit_reached"], "complete")
            self.assertGreaterEqual(result["calls"], 1)
            self.assertLessEqual(result["bytes_read"], 2_000_000)


if __name__ == "__main__":
    unittest.main()
