from pathlib import Path
import tempfile
import unittest

from task_key_audit import key_like, literal_index, literals


class KeyAuditTests(unittest.TestCase):
    def test_exact_keys_exclude_specifiers_placeholders_and_prose(self):
        source = "\n".join([
            'import { a } from "@w/core";',
            '} from "@w/multi-line";',
            'import "./side-effect.ts";',
            'const header = "x-t3-relay-delivery-id";',
            "const tag = 'CommandPolicyUnsupportedError';",
            'const event = "thread.meta.update";',
            'const prose = "plain";',
            'const shout = "LOUD";',
            "const template = `t3-env:${id}`;",
            'const spaced = "two words";',
        ])
        self.assertEqual(literals(source), {"x-t3-relay-delivery-id", "CommandPolicyUnsupportedError", "thread.meta.update"})
        self.assertFalse(key_like("../relative/path"))
        self.assertTrue(key_like("active_steering"))

    def test_index_maps_each_exact_literal_to_its_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "node_modules").mkdir()
            (root / "node_modules" / "dep.ts").write_text('"x-shared-key"')
            (root / "a.ts").write_text('send("x-shared-key")')
            (root / "b.json").write_text('{"x-shared-key": 1}')
            (root / "c.md").write_text('"x-shared-key"')
            files, index = literal_index(root)
            self.assertEqual(index["x-shared-key"], {"a.ts", "b.json"})
            self.assertEqual(set(files), {"a.ts", "b.json"})


if __name__ == "__main__":
    unittest.main()
