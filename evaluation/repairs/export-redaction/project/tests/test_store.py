import unittest

from settings.export import REDACTED
from settings.store import save


class RedactedSave(unittest.TestCase):
    def test_redacted_save_keeps_stored_provider_secret(self):
        stored = {"providers": {"openai": {"api_key": "sk-secret", "model": "a"}}}
        merged = save(stored, {"providers": {"openai": {"api_key": REDACTED, "model": "b"}}})
        self.assertEqual(merged["providers"]["openai"], {"api_key": "sk-secret", "model": "b"})
