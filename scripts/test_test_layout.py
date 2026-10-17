"""Guard numbered-stage test discovery when Cargo autotests is disabled."""
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


class TestLayoutTests(unittest.TestCase):
    def test_numbered_stage_regressions_have_explicit_cargo_targets_and_a_gate(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        self.assertFalse(manifest["package"]["autotests"])
        targets = {target["name"]: target["path"] for target in manifest["test"]}
        gate = (ROOT / "scripts/verify-context").read_text()
        for stage in ("stage2", "stage3", "stage4"):
            self.assertEqual(targets[stage], f"tests/briefcase_{stage}.rs")
            self.assertTrue((ROOT / targets[stage]).is_file())
            self.assertIn(f"--test {stage}", gate)
            self.assertNotIn(f"briefcase_{stage}", (ROOT / "tests/context.rs").read_text())
        self.assertIn("./verify-context", (ROOT / "scripts/verify-mvp").read_text())


if __name__ == "__main__":
    unittest.main()
