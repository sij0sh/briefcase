import unittest

from settings.export import REDACTED, redact

SETTINGS = {
    "api_token": "t-secret",
    "public_key": "pk-top",
    "providers": {"openai": {"api_key": "sk-secret", "public_key": "pk-provider", "model": "m"}},
}


class ProviderSecrets(unittest.TestCase):
    def test_provider_api_keys_are_redacted(self):
        self.assertEqual(redact(SETTINGS)["providers"]["openai"]["api_key"], REDACTED)


class TopLevelSecrets(unittest.TestCase):
    def test_api_token_is_redacted(self):
        self.assertEqual(redact(SETTINGS)["api_token"], REDACTED)

    def test_input_is_not_mutated(self):
        redact(SETTINGS)
        self.assertEqual(SETTINGS["api_token"], "t-secret")


class PublicFields(unittest.TestCase):
    def test_public_keys_stay_exported(self):
        exported = redact(SETTINGS)
        self.assertEqual(exported["public_key"], "pk-top")
        self.assertEqual(exported["providers"]["openai"]["public_key"], "pk-provider")
