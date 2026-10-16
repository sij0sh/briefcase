#!/usr/bin/env python3
"""Focused checks for the dogfood question evaluator (no CLI run needed)."""
import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "dogfood", Path(__file__).with_name("dogfood.py"))
dogfood = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dogfood)


class FakeBriefcase:
    def __init__(self, payload):
        self.payload = payload

    def call(self, operation, arguments):
        return self.payload


class AnswerTests(unittest.TestCase):
    def question(self, **expect):
        return {"id": "T", "question": "q", "operation": "status",
                "arguments": {}, "expect": expect}

    def test_contains_and_excludes(self):
        trace = FakeBriefcase({"text": "ledger DEPENDS_ON knowledge"})
        ok = dogfood.answer(trace, self.question(
            contains=["DEPENDS_ON"], excludes=[".exitcode/state/"]))
        self.assertEqual(ok["status"], "pass")
        bad = dogfood.answer(FakeBriefcase({"text": "see .exitcode/state/x"}),
                             self.question(excludes=[".exitcode/state/"]))
        self.assertEqual(bad["status"], "fail")
        self.assertEqual(bad["unexpected"], [".exitcode/state/"])

    def test_byte_cap_fails_oversized_answers(self):
        trace = FakeBriefcase({"text": "x" * 100})
        capped = dogfood.answer(trace, self.question(max_answer_bytes=10))
        self.assertEqual(capped["status"], "fail")
        self.assertTrue(capped["over_bytes"])
        uncapped = dogfood.answer(trace, self.question())
        self.assertEqual(uncapped["status"], "pass")

    def test_operation_errors_are_counted_not_raised(self):
        class Boom:
            def call(self, operation, arguments):
                raise RuntimeError("ARGUMENTS: Unknown argument sql")
        result = dogfood.answer(Boom(), self.question())
        self.assertEqual(result["status"], "error")
        self.assertIn("Unknown argument", result["error"])


if __name__ == "__main__":
    unittest.main()
