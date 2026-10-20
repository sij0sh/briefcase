import json
import tempfile
import unittest
from pathlib import Path

import task_outcomes as outcomes

FIXTURES = outcomes.ROOT / "evaluation" / "repairs"


def delivered(files=None, findings=None, retained=None, candidates=None):
    return {"files": files or {}, "findings": findings or [], "retained_findings": retained or [],
            "candidates": candidates}


class WitnessScoringTests(unittest.TestCase):
    claim = {"id": "C1", "witnesses": [{"path": "a.py", "pattern": r"MAX_DELAY\s*="}],
             "alternatives": [[{"finding": "session_review_requirement", "pattern": "MAX_DELAY"}]]}

    def test_every_witness_of_one_set_must_match(self):
        self.assertTrue(outcomes.claim_supported(self.claim, delivered({"a.py": "MAX_DELAY = 30\n"})))
        self.assertFalse(outcomes.claim_supported(self.claim, delivered({"a.py": "delay = 30\n"})))

    def test_focused_passages_do_not_credit_an_artificial_join_across_unread_bytes(self):
        evidence = delivered({"a.py": "MAX_DELAY ="})
        evidence["file_passages"] = {"a.py": ["MAX_DELAY", " ="]}
        self.assertFalse(outcomes.claim_supported(self.claim, evidence))
        evidence["file_passages"]["a.py"] = ["MAX_DELAY ="]
        self.assertTrue(outcomes.claim_supported(self.claim, evidence))

    def test_finding_alternative_needs_matching_kind_and_text(self):
        right = {"kind": "session_review_requirement", "claim": "keep under MAX_DELAY"}
        wrong = {"kind": "behavior_condition", "claim": "keep under MAX_DELAY"}
        self.assertTrue(outcomes.claim_supported(self.claim, delivered(findings=[right])))
        self.assertFalse(outcomes.claim_supported(self.claim, delivered(findings=[wrong])))

    def test_session_claims_are_scored_only_with_sessions(self):
        claims = [{"id": "A", "witnesses": []}, {"id": "B", "requires_sessions": True, "witnesses": []}]
        self.assertEqual([c["id"] for c in outcomes.eligible_claims(claims, False)], ["A"])
        self.assertEqual(len(outcomes.eligible_claims(claims, True)), 2)


class MissClassificationTests(unittest.TestCase):
    def test_classes_follow_the_first_failing_stage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.py").write_text("head\n" + "x\n" * 50 + "SECRET = 1\n")
            claim = {"id": "C", "witnesses": [{"path": "a.py", "pattern": "SECRET"}]}
            classify = lambda d: outcomes.classify_miss(claim, d, root)["class"]
            self.assertEqual(classify(delivered({"a.py": "head\n"})), "consumption")
            self.assertEqual(classify(delivered(candidates=["a.py"])), "selection")
            self.assertEqual(classify(delivered(candidates=["b.py"])), "discovery")
            missing = {"id": "C", "witnesses": [{"path": "a.py", "pattern": "NOT_THERE"}]}
            self.assertEqual(outcomes.classify_miss(missing, delivered({"a.py": "head\n"}), root)["class"], "producer")

    def test_truncated_finding_is_consumption_and_absent_finding_is_producer(self):
        claim = {"id": "S", "witnesses": [{"finding": "session_review_requirement", "pattern": "MAX_DELAY"}]}
        compact = {"kind": "session_review_requirement", "claim": "You must always keep jittered delays unde"}
        full = {"kind": "session_review_requirement", "claim": "keep jittered delays under the MAX_DELAY ceiling"}
        self.assertEqual(outcomes.classify_miss(claim, delivered(findings=[compact], retained=[full]), ".")["class"],
                         "consumption")
        self.assertEqual(outcomes.classify_miss(claim, delivered(findings=[compact]), ".")["class"], "producer")


class RetainedFindingsTests(unittest.TestCase):
    def test_follows_continuations_and_tolerates_a_case_without_findings(self):
        pages = {0: {"items": [{"n": 0}, {"n": 1}], "continue_with": {"arguments": {"cursor": "next"}}},
                 "next": {"items": [{"n": 2}], "continue_with": None}}
        calls = []

        def call(op, args):
            calls.append(args)
            return pages[args.get("cursor", 0)]

        self.assertEqual([f["n"] for f in outcomes.retained_findings(call, "task-case:x")], [0, 1, 2])
        self.assertEqual(len(calls), 2)

        def missing(op, args):
            raise RuntimeError("error: PAGINATION: Unknown report section")

        self.assertEqual(outcomes.retained_findings(missing, "task-case:x"), [])

        def broken(op, args):
            raise RuntimeError("error: STORAGE: database is locked")

        with self.assertRaises(RuntimeError):
            outcomes.retained_findings(broken, "task-case:x")


class CriticalAuditTests(unittest.TestCase):
    def test_bounded_findings_pass(self):
        findings = [
            {"kind": "behavior_condition", "claim": "has a retained declared relationship",
             "basis": {"evidence": [{"role": "capability"}]},
             "payload": {"conditions": [{"runtime_satisfaction": "unknown"}],
                         "verification": [{"assessment": "association_established_execution_unknown",
                                           "statement": "Static association does not establish that the test passed"}]}},
            {"kind": "session_failure_recovery", "applicability": {"current_guidance": False},
             "payload": {"type": "session_experience", "causality": "unverified", "necessity": "unverified"}},
            {"kind": "architectural_convergence", "basis": {"authority": "observed_convention"},
             "payload": {"decision": {"automatic_change": False}}},
        ]
        self.assertEqual(outcomes.critical_audit(findings), [])

    def test_overclaims_are_flagged(self):
        findings = [
            {"kind": "behavior_condition", "claim": "has a retained declared relationship", "basis": {"evidence": []},
             "payload": {"conditions": [{"runtime_satisfaction": "satisfied"}],
                         "verification": [{"assessment": "established", "statement": "tests passed"}]}},
            {"kind": "session_review_requirement", "applicability": {"current_guidance": True},
             "payload": {"type": "session_experience", "causality": "caused"}},
            {"kind": "architectural_convergence", "basis": {"authority": "declaration"},
             "payload": {"decision": {"automatic_change": True}}},
        ]
        reasons = {v["reason"] for v in outcomes.critical_audit(findings)}
        self.assertEqual(reasons, {
            "capability claimed without a declaration basis", "runtime satisfaction asserted from static evidence",
            "test execution asserted from a static association", "session history presented as current guidance",
            "session causality asserted", "convergence proposes an automatic change",
            "observed prevalence presented as policy"})


class StatisticsTests(unittest.TestCase):
    protocol = outcomes.load_protocol()

    def test_bootstrap_is_seeded_and_brackets_the_mean(self):
        differences = [0.0, 0.5, 0.25, 1.0, 0.0, 0.3]
        first, second = outcomes.paired_bootstrap(differences), outcomes.paired_bootstrap(differences)
        self.assertEqual(first, second)
        self.assertLessEqual(first["interval"][0], first["mean"])
        self.assertGreaterEqual(first["interval"][1], first["mean"])
        self.assertEqual(outcomes.paired_bootstrap([])["mean"], None)

    def rows(self, treatment, control, critical=(0, 0)):
        rows = []
        for index, (t, c) in enumerate(zip(treatment, control)):
            rows.append({"scenario": f"s{index}", "condition": "findings", "supportability": t,
                         "unsupported_critical_claims": critical[0] if index == 0 else 0})
            rows.append({"scenario": f"s{index}", "condition": "hybrid", "supportability": c,
                         "unsupported_critical_claims": critical[1] if index == 0 else 0})
        return rows

    def gate(self, rows):
        primary = outcomes.contrast(rows, "findings", "hybrid", {"resamples": 2000, "seed": 1})
        return outcomes.gate(primary, self.protocol)["status"]

    def test_gate_outcomes(self):
        self.assertEqual(self.gate(self.rows([1.0] * 8, [0.5] * 8)), "met")
        self.assertEqual(self.gate(self.rows([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0])), "inconclusive")
        self.assertEqual(self.gate(self.rows([0.5] * 4, [0.5] * 4)), "not_met")
        self.assertEqual(self.gate(self.rows([1.0] * 8, [0.5] * 8, critical=(1, 0))), "not_met")
        self.assertEqual(outcomes.gate(outcomes.contrast([], "findings", "hybrid", {}), self.protocol)["status"],
                         "not_run")


class AgentScoringTests(unittest.TestCase):
    def test_citations_must_name_delivered_paths_or_findings(self):
        task = {"claims": [{"id": "C", "witnesses": [{"path": "a.py", "pattern": "x"}]}]}
        answer = {"claims": [{"text": "a holds", "citations": ["a.py"]},
                             {"text": "b holds", "citations": ["b.py"]},
                             {"text": "review says so", "citations": ["finding:session_review_requirement"]},
                             {"text": "uncited"}],
                  "rationale": "the tests passed at runtime"}
        scored = outcomes.score_agent(task, answer, delivered({"a.py": "x"}), [r"tests passed at runtime"])
        self.assertEqual(scored["unsupported_assertions"], 3)
        self.assertEqual(scored["unsupported_critical_claims"], 1)
        self.assertEqual(scored["cited_claims"], ["C"])


class AgentTrialTests(unittest.TestCase):
    def test_first_failure_stops_further_paid_trials(self):
        import argparse, importlib.util
        spec = importlib.util.spec_from_file_location("task_outcomes_cli", Path(__file__).with_name("task-outcomes.py"))
        cli = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cli)
        with tempfile.TemporaryDirectory() as work:
            args = argparse.Namespace(set="t3", work=work, agent_command="exit 3", agent_timeout=30,
                                      agent_repeats=3, agent_workers=1, agent_max_errors=1)
            trials = cli.AgentTrials(args)
            trials.add({"id": "T", "task": "x", "claims": []}, ["a"], {"a": delivered()})
            rows = trials.run()
        self.assertEqual([row["status"] for row in rows], ["error", "skipped", "skipped"])
        self.assertIn("T/a", trials.aborted)


class FixtureTests(unittest.TestCase):
    def test_materialized_projects_hold_no_scorer_only_files(self):
        scorer_only = {"fixture.json", "reference.patch", "naive.patch"}
        with tempfile.TemporaryDirectory() as directory:
            for fixture_dir, fixture in outcomes.load_fixtures(FIXTURES):
                root = outcomes.materialize(fixture_dir, fixture, Path(directory) / fixture["id"],
                                            sessions=bool(fixture.get("sessions")))
                names = {p.name for p in root.rglob("*")}
                self.assertFalse(names & scorer_only, fixture["id"])
                text = "\n".join(p.read_text(errors="replace") for p in root.rglob("*") if p.is_file())
                for claim in fixture["claims"]:
                    self.assertNotIn(claim["id"], text, fixture["id"])
                self.assertNotIn("{{ROOT}}", text)

    def test_every_repair_fixture_is_meaningful_and_reversible(self):
        fixtures = outcomes.load_fixtures(FIXTURES)
        self.assertGreaterEqual(len(fixtures), 4)
        self.assertTrue(any(f["reference_decision"] == "no_change" for _, f in fixtures),
                        "an intentional exception must be represented")
        with tempfile.TemporaryDirectory() as directory:
            for fixture_dir, fixture in fixtures:
                result = outcomes.validate_fixture(fixture_dir, fixture, Path(directory) / fixture["id"])
                self.assertTrue(result["ok"], json.dumps(result, indent=1))


if __name__ == "__main__":
    unittest.main()
