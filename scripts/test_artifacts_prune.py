"""Run with: python3 -B -m unittest discover -s scripts -p 'test_*.py'."""
import importlib.util
import os
from pathlib import Path
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location('artifacts_prune', Path(__file__).with_name('artifacts-prune.py'))
prune = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prune)


class ArtifactsPruneTests(unittest.TestCase):
    def test_lists_old_large_and_database_entries_but_not_fresh_or_tracked(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            base = root / '.agents/artifacts'
            (base / 'old').mkdir(parents=True)
            (base / 'old/notes.txt').write_text('x')
            (base / 'fresh').mkdir()
            (base / 'fresh/notes.txt').write_text('x')
            (base / 'fresh/briefcase.db').write_bytes(b'db')
            (base / 'big.bin').write_bytes(b'0' * (2 * 2**20))
            (base / 'record.md').write_text('kept')
            old = time.time() - 30 * 86400
            for p in [base / 'old', base / 'old/notes.txt']:
                os.utime(p, (old, old))
            prune.tracked = lambda _root: {base / 'record.md'}
            found = {p.relative_to(base).as_posix(): why for p, _, why in
                     prune.stale(root, days=14, max_mb=1, now=time.time())}
            self.assertEqual(set(found), {'old', 'big.bin', 'fresh/briefcase.db'})
            self.assertEqual(found['fresh/briefcase.db'], ['stray database'])


if __name__ == '__main__':
    unittest.main()
