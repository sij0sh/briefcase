"""Run with: python3 -B -m unittest discover -s scripts -p 'test_*.py'."""
import os
import runpy
from pathlib import Path
import sys
import tempfile
import unittest

from evaluation_support import Briefcase, digest, present, token


class EvaluationSupportTests(unittest.TestCase):
    def test_digest_is_over_bytes(self):
        self.assertEqual(
            digest(b'abc'),
            'sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
        )
        self.assertNotEqual(digest(b'a'), digest(b'A'))

    def test_token_and_fact_matching_stay_within_one_unit(self):
        fact = {'all': [['app'], ['save', 'store']]}
        for units, expected in [
            (['app saves'], False),
            (['app save'], True),
            (['app', 'save'], False),
            (['my_app save'], False),
            (['app store'], True),
        ]:
            self.assertEqual(present(fact, units), expected, units)
        self.assertTrue(token('a.b').search(' a.b '))
        self.assertFalse(token('a.b').search(' axb '))

    def test_task_packet_evaluator_is_independent_of_retired_products(self):
        evaluator = runpy.run_path(str(Path(__file__).resolve().parent / 'task-evaluate.py'))
        calls = []
        def call(operation, arguments):
            calls.append((operation, arguments))
            return {'items': [{'path': 'src/雪.rs'}], 'page': {'total': 1, 'next_offset': None}}
        section = evaluator['packet_section']({'id': 'task-brief:test'}, '/slice/impl', call)
        self.assertEqual(section, [{'path': 'src/雪.rs'}])
        self.assertEqual(calls[0][0], 'page')

    @unittest.skipIf(os.name == 'nt', 'The stub executable uses a POSIX shebang.')
    def test_runner_keeps_arguments_json_text_and_bounded_errors(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'briefcase-stub'
            binary.write_text(
                f'#!{sys.executable}\n'
                'import json, sys\n'
                'if "fail" in sys.argv:\n'
                '    print("x" * 500, file=sys.stderr)\n'
                '    sys.exit(7)\n'
                'if "--json" in sys.argv:\n'
                '    print(json.dumps({"args": sys.argv[1:], "text": "café"}))\n'
                'else:\n'
                '    print("literal café")\n'
            )
            binary.chmod(0o700)
            runner = Briefcase(binary, Path(directory) / 'briefcase.db')
            result = runner.run('status')
            self.assertEqual(result['text'], 'café')
            self.assertEqual(result['args'], ['--db', str(runner.db), '--json', 'status'])
            self.assertEqual(runner.run('status', json_output=False), 'literal café\n')
            with self.assertRaises(RuntimeError) as error:
                runner.run('fail')
            self.assertEqual(str(error.exception), 'fail: ' + 'x' * 400)


if __name__ == '__main__':
    unittest.main()
