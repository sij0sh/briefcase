import unittest

from task_evidence_metrics import aggregate, score, validate_oracles


class EvidenceMetricsTests(unittest.TestCase):
    def fixture(self):
        oracle = {"id": "task", "passages": [{"id": "p", "path": "a.ts", "pattern": "guard then apply",
            "source_sha256": "abc", "byte_range": {"start": 2, "end": 18}}],
            "relationships": [{"id": "r", "predicate": "IMPORTS", "from_path": "a.ts", "to_path": "b.ts"}],
            "claims": [{"id": "c", "text": "A statically imports B.", "passages": ["p"], "relationships": ["r"]}]}
        passages = {"a.ts": ["  guard then apply  "]}
        ranges = {"a.ts": [{"start": 0, "end": 20}]}
        identities = {"a.ts": {"digest": "sha256:abc"}}
        response = {"items": [{"id": "task-link:one", "predicate": "IMPORTS", "class": "evidence",
            "basis": "retained_assertion", "from_files": ["a.ts"], "to_files": ["b.ts"]}]}
        return oracle, passages, ranges, identities, [response]

    def test_passages_and_relationships_do_not_automatically_submit_conclusions(self):
        args = self.fixture()
        validate_oracles([args[0]])
        result = score(*args)
        self.assertEqual((result["passages_inspected"], result["relationships_delivered"], result["claims_supportable"]), (1, 1, 1))
        self.assertEqual(result["conclusions"]["state"], "not_submitted")
        self.assertEqual(result["conclusions"]["supported"], 0)

    def test_explicit_conclusions_require_exact_claim_and_inspected_citations(self):
        args = self.fixture()
        conclusion = {"claim_id": "c", "text": "A statically imports B.", "relationship_ids": ["task-link:one"],
            "citations": [{"path": "a.ts", "digest": "sha256:abc", "start_byte": 2, "end_byte": 18}]}
        self.assertEqual(score(*args, [conclusion])["conclusions"]["supported"], 1)
        for change in ({"text": "A always executes B."}, {"relationship_ids": []},
                       {"citations": [{"path": "a.ts", "digest": "sha256:abc", "start_byte": 2, "end_byte": 99}]}):
            self.assertEqual(score(*args, [{**conclusion, **change}])["conclusions"]["unsupported"], 1)

    def test_no_pattern_can_cross_an_uninspected_gap_or_changed_digest(self):
        oracle, _, _, identities, responses = self.fixture()
        parts = {"a.ts": ["guard ", "then apply"]}
        ranges = {"a.ts": [{"start": 2, "end": 8}, {"start": 20, "end": 30}]}
        self.assertEqual(score(oracle, parts, ranges, identities, responses)["passages_inspected"], 0)
        args = list(self.fixture())
        args[3] = {"a.ts": {"digest": "sha256:changed"}}
        self.assertEqual(score(*args)["passages_inspected"], 0)

    def test_shared_literal_or_file_cooccurrence_cannot_satisfy_a_call_relation(self):
        args = list(self.fixture())
        args[0]["relationships"][0]["predicate"] = "CALLS_STATIC"
        args[4] = [{"items": [{"kind": "exact_key_association", "authority": "static_association",
            "from_path": "a.ts", "to_path": "b.ts", "key": "event.changed.state"}]}]
        result = score(*args)
        self.assertEqual((result["relationships_delivered"], result["claims_supportable"]), (0, 0))

    def test_static_relationship_does_not_meet_execution_authority(self):
        args = list(self.fixture())
        args[0]["relationships"][0]["authority"] = "executed_evidence"
        self.assertEqual(score(*args)["relationships_delivered"], 0)
        args[0]["relationships"][0]["authority"] = "static_association"
        self.assertEqual(score(*args)["relationships_delivered"], 1)

    def test_alternatives_and_micro_macro_full_error_denominators(self):
        args = list(self.fixture())
        args[0]["claims"][0]["alternatives"] = [{"passages": ["p"]}]
        args[4] = []
        result = score(*args)
        self.assertEqual(result["claims_supportable"], 1)
        total = aggregate([result, {"id": "error", "status": "error", "passages_required": 3,
            "relationships_required": 1, "claims_required": 2}])
        self.assertEqual(total["passages_required"], 4)
        self.assertEqual(total["micro_passage_coverage_lower_bound"], .25)
        self.assertIsNone(total["micro_passage_coverage"])
        self.assertFalse(total["comparison_eligible"])


if __name__ == "__main__":
    unittest.main()
