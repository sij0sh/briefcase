"""Run with: python3 -B -m unittest discover -s tools -p 'test_*.py'."""
from pathlib import Path
import tempfile
import unittest

from check_package import documentation_files


class DocumentationScopeTests(unittest.TestCase):
    def test_product_documentation_includes_authored_roots_not_agent_archives(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            included = ["README.md", "docs/guide.md", "docs/adr/0001-test.md", "contracts/v1/README.md"]
            excluded = [".agents/artifacts/source/archive.md", ".agents/.exitcode/state/evidence.md", "target/generated.md"]
            for name in included + excluded:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("# Product docs\n" if name in included else "Unmatched copied fence: ```\n")
            self.assertEqual(
                {path.relative_to(root).as_posix() for path in documentation_files(root)},
                set(included),
            )

    def test_missing_optional_documentation_roots_are_empty(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(list(documentation_files(Path(directory))), [])


if __name__ == "__main__":
    unittest.main()
