import json
from pathlib import Path
import tempfile
import unittest

from task_investigation import key
from task_loss_witnesses import inspected_passages, score_claims


class PassageWitnessTests(unittest.TestCase):
    def test_unread_gaps_cannot_create_a_witness_but_contiguous_pages_can(self):
        with tempfile.TemporaryDirectory() as directory:
            spans, events = [], []
            for index, (offset, text) in enumerate([(0, "guard"), (10, "checked"), (17, " here")]):
                event = {"response": {"entry": {"path": "a"}, "source_offset": offset, "text": text}}
                identifier = key(event)
                file = f"{index}.json"
                (Path(directory) / file).write_text(json.dumps(event))
                spans.append({"start": offset, "end": offset + len(text), "event": identifier})
                events.append({"id": identifier, "file": file})
            state = {"events": events, "sources": {"a": {"ranges": spans[:1], "excerpts": spans[1:]}}}
            passages = inspected_passages(directory, state)
            self.assertEqual(passages, {"a": ["guard", "checked here"]})
            self.assertEqual(inspected_passages(directory, state, {events[0]["id"]}), {"a": ["guard"]})
            claims = [{"id": "gap", "witnesses": [{"path": "a", "pattern": "guard.*checked"}]},
                      {"id": "read", "witnesses": [{"path": "a", "pattern": "checked here"}]}]
            score = score_claims(claims, passages)
            self.assertEqual(score["supported_claim_ids"], ["read"])
            (Path(directory) / events[0]["file"]).write_text("{}")
            with self.assertRaisesRegex(ValueError, "modified"):
                inspected_passages(directory, state)

    def test_alternative_witness_sets_and_missing_evidence(self):
        claim = {"id": "c", "witnesses": [{"path": "a", "pattern": "guard"}],
                 "alternatives": [[{"path": "b", "pattern": "checked"}]]}
        self.assertEqual(score_claims([claim], {"b": ["checked"]})["supported_claims"], 1)
        self.assertEqual(score_claims([claim], {})["supported_claims"], 0)


if __name__ == "__main__":
    unittest.main()
