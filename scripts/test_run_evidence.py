import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from run_evidence import RunEvidence, check_build_receipt, digest, implementation


class RunEvidenceTests(unittest.TestCase):
    def test_exact_raw_outputs_commands_and_failure_codes_are_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            run = RunEvidence(Path(directory) / "run")
            result = run.run([sys.executable, "-c", "import sys; print('雪'); print('failed',file=sys.stderr);sys.exit(7)"])
            self.assertEqual(result.returncode, 7)
            self.assertEqual((run.directory / "000001.stdout").read_bytes(), "雪\n".encode())
            meta = json.loads((run.directory / "000001.json").read_text())
            self.assertEqual(meta["exit_code"], 7)
            self.assertEqual(meta["stderr_sha256"], digest(b"failed\n"))
            with self.assertRaisesRegex(ValueError, "empty"):
                RunEvidence(run.directory)

    def test_source_and_binary_changes_reject_a_stale_build_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir(); (root / "src/lib.rs").write_text("pub fn a() {}")
            binary = root / "binary"; binary.write_bytes(b"binary")
            source = implementation(root)
            receipt = {"build": {"exit_code": 0}, "implementation_sha256": source["sha256"], "binary_sha256": digest(binary.read_bytes())}
            self.assertEqual(check_build_receipt(receipt, source, binary)["state"], "matching_build_receipt")
            (root / "src/lib.rs").write_text("pub fn b() {}")
            with self.assertRaisesRegex(ValueError, "bind"):
                check_build_receipt(receipt, implementation(root), binary)
            binary.write_bytes(b"different")
            with self.assertRaisesRegex(ValueError, "bind"):
                check_build_receipt(receipt, source, binary)

    def test_missing_executable_is_recorded_as_an_error_not_a_success(self):
        with tempfile.TemporaryDirectory() as directory:
            run = RunEvidence(Path(directory) / "run")
            with self.assertRaises(FileNotFoundError):
                run.run([str(Path(directory) / "not-installed")])
            meta = json.loads((run.directory / "000001.json").read_text())
            self.assertEqual(meta["state"], "error")
            self.assertNotIn("exit_code", meta)
