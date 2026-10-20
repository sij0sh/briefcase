import json
import tempfile
import unittest
from pathlib import Path

from task_evidence_metrics import aggregate, evaluate, score, state_conclusions, validate_oracles
from task_investigation import Investigation
from test_task_investigation import Protocol


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

    def test_coverage_per_effort_is_reported_only_for_complete_arms(self):
        args = self.fixture()
        conclusion = {"claim_id": "c", "text": "A statically imports B.", "relationship_ids": ["task-link:one"],
                      "citations": [{"path": "a.ts", "digest": "sha256:abc", "start_byte": 2, "end_byte": 18}]}
        result = score(*args, [conclusion])
        result["effort"] = {"calls": 10, "bytes": 2 * 1024 * 1024, "elapsed_seconds": 2}
        total = aggregate([result])
        self.assertEqual(total["effort"]["calls"], 10)
        self.assertEqual(total["coverage_per_effort"]["passage_hits_per_call"], .1)
        self.assertEqual(total["coverage_per_effort"]["supported_conclusions_per_call"], .1)
        self.assertEqual(total["coverage_per_effort"]["passage_hits_per_mib"], .5)
        error = {"id": "error", "status": "error", "passages_required": 0,
                 "relationships_required": 0, "claims_required": 0,
                 "effort": {"calls": 5, "bytes": 0, "elapsed_seconds": 1}}
        incomplete = aggregate([result, error])
        self.assertTrue(incomplete["errors"])
        self.assertIsNone(incomplete["coverage_per_effort"]["supported_conclusions_per_call"])
        self.assertIsNone(incomplete["coverage_per_effort"]["passage_hits_per_mib"])
        timed = {**result, "timing_error": "serial latency pass failed"}
        self.assertTrue(aggregate([timed])["errors"])
        self.assertIsNone(aggregate([timed])["coverage_per_effort"]["claim_hits_per_call"])

    def test_checkpoint_conclusions_are_only_read_from_a_list(self):
        self.assertEqual(state_conclusions({"conclusions": [{"claim_id": "c"}]}), [{"claim_id": "c"}])
        self.assertEqual(state_conclusions({}), [])
        with self.assertRaisesRegex(ValueError, "list"):
            state_conclusions({"conclusions": {}})

    def test_evaluate_uses_checkpoint_conclusions_unless_external_file_overrides(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            flow = Investigation(Protocol(), root / "investigation")
            flow.start({"task": "refactor storage", "target": "src/a.rs"}, budget=3000)
            while flow.state["queue"]:
                flow.step()
            source = flow.state["sources"]["src/a.rs"]
            claim_text = "The anchor bytes were inspected."
            flow.conclude({"claim_id": "c", "text": claim_text,
                           "citations": [{"path": "src/a.rs", "digest": source["identity"]["digest"],
                                          "start_byte": 0, "end_byte": 8}]})
            flow.save()
            results = root / "results.json"
            results.write_text(json.dumps({"investigations": {"excerpt-first": [{
                "id": "t", "case_id": flow.state["case_id"], "index_id": flow.state["index_id"],
                "view_id": flow.state["view_id"], "checkpoint": str(root / "investigation" / "state.json"),
                "calls": 1, "bytes": 100, "elapsed_seconds": 1}]}}))
            oracle = {"id": "t", "passages": [{"id": "p", "path": "src/a.rs", "pattern": "anchor",
                "source_sha256": source["identity"]["digest"].removeprefix("sha256:"),
                "byte_range": {"start": 0, "end": 8}}], "relationships": [],
                "claims": [{"id": "c", "text": claim_text, "passages": ["p"], "relationships": []}]}
            recorded = evaluate(results, [oracle])
            self.assertEqual(recorded["modes"]["excerpt-first"]["conclusions"]["supported"], 1)
            self.assertEqual(recorded["modes"]["excerpt-first"]["effort"]["calls"], 1)
            overridden = evaluate(results, [oracle], {("excerpt-first", "t"): []})
            self.assertEqual(overridden["modes"]["excerpt-first"]["conclusions"]["state"], "scored")
            self.assertEqual(overridden["modes"]["excerpt-first"]["conclusions"]["supported"], 0)

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
