import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import contextlib
import io
import json
import tempfile

spec = importlib.util.spec_from_file_location("task_evaluate", Path(__file__).with_name("task-evaluate.py"))
task_evaluate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(task_evaluate)


class TaskEvaluationTests(unittest.TestCase):
    def test_followup_files_measure_missed_context(self):
        packet = {"id": "task-brief:test", "view_id": "view:1", "outcome": "partial", "slice": {},
                  "not_covered": [{"area": "external_installations"}],
                  "coverage": {"delivery": {"/not_covered": {"omitted": 0}}}}
        for role in task_evaluate.ROLES:
            packet["slice"][role] = []
            packet["coverage"]["delivery"][f"/slice/{role}"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "src/a.rs"}]
        result = task_evaluate.evaluate_scenario(
            {"id": "rename", "task": "rename a", "followup_paths": ["src/a.rs", "scripts/install"],
             "required_gap_areas": ["external_installations"]}, lambda *_: packet)
        self.assertEqual(result["status"], "miss")
        self.assertEqual(result["required_path_recall"], 0.5)
        self.assertEqual(result["missing_paths"], ["scripts/install"])

    def test_omitted_sections_are_retrieved_without_recomputing_the_task(self):
        calls = []

        def call(operation, args):
            calls.append((operation, args))
            offset = args["offset"]
            return {"items": [{"path": str(offset)}], "page": {"total": 2,
                    "next_offset": 1 if offset == 0 else None}}

        packet = {"id": "task-brief:test", "slice": {"test": [{"path": "0"}]},
                  "coverage": {"delivery": {"/slice/test": {"omitted": 1}}}}
        rows = task_evaluate.packet_section(packet, "/slice/test", call)
        self.assertEqual(rows, [{"path": "0"}, {"path": "1"}])
        self.assertEqual([c[1]["offset"] for c in calls], [0, 1])
        self.assertTrue(all(c[0] == "page" for c in calls))

    def test_cursor_gaps_and_retractions_fail_evaluation(self):
        packet = {"id": "task-brief:test"}
        with self.assertRaisesRegex(ValueError, "contiguous"):
            task_evaluate.packet_section(packet, "/slice/test", lambda *_:
                {"items": [{}], "page": {"total": 20, "next_offset": 10}})
        with self.assertRaisesRegex(ValueError, "Retractions"):
            task_evaluate.packet_section(packet, "/slice/test", lambda *_:
                {"items": [], "page": {"total": 0}, "historical_packet": True})

    def test_missing_labels_and_budget_notices_are_not_successes(self):
        with self.assertRaises(ValueError):
            task_evaluate.evaluate_scenario({"id": "unlabelled", "task": "rename"}, lambda *_: {})
        with self.assertRaisesRegex(ValueError, "omitted"):
            task_evaluate.packet_section({"id": "task-brief:test"}, "/slice/test", lambda *_:
                {"truncated": True, "report_id": "task-brief:test"})

    def test_false_complete_counts_missed_authored_paths_and_excludes_errors(self):
        packet = {"id": "task-case:test", "case_id": "task-case:test", "outcome": "complete",
                  "slice": {role: [] for role in task_evaluate.ROLES}, "not_covered": [],
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "src/a.rs"}]
        scenario = {"id": "rename", "task": "rename", "required_paths": ["src/a.rs", "src/b.rs"]}
        incorrect = task_evaluate.evaluate_scenario(scenario, lambda *_: packet)
        self.assertTrue(incorrect["false_complete"])
        self.assertEqual(incorrect["case_id"], "task-case:test")
        packet["outcome"] = "partial"
        partial = task_evaluate.evaluate_scenario(scenario, lambda *_: packet)
        self.assertFalse(partial["false_complete"])
        metrics = task_evaluate.false_complete_metrics([incorrect, partial, {"status": "error"}])
        self.assertEqual(metrics["false_complete_rate"], 0.5)
        self.assertEqual(metrics["false_complete_given_complete_rate"], 1.0)
        self.assertEqual(metrics["scored_scenarios"], 2)
        packet["outcome"] = "complete"
        scenario["required_paths"] = ["src/a.rs"]
        self.assertFalse(task_evaluate.evaluate_scenario(scenario, lambda *_: packet)["false_complete"])

    def test_new_case_ids_page_and_missing_outcomes_are_errors(self):
        items = task_evaluate.packet_section({"case_id": "task-case:new"}, "/slice/test", lambda *_:
            {"items": [], "page": {"total": 0, "next_offset": None}})
        self.assertEqual(items, [])
        with self.assertRaisesRegex(ValueError, "outcome"):
            task_evaluate.evaluate_scenario({"id": "x", "task": "x", "required_paths": ["x"]}, lambda *_: {})
        self.assertIsNone(task_evaluate.false_complete_metrics([{"status": "error"}])["false_complete_rate"])

    def test_scenarios_and_deprecated_cases_cli_and_input_keys(self):
        packet = {"case_id": "task-case:test", "id": "task-case:test", "outcome": "complete", "current_view": "view:test", "view_id": "view:test",
                  "slice": {role: [] for role in task_evaluate.ROLES}, "not_covered": [],
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "a"}]
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "scenarios.json"
            for option, key in [("--scenarios", "scenarios"), ("--cases", "cases")]:
                source.write_text(json.dumps({key: [{"id": "x", "task": "x", "required_paths": ["a"]}]}))
                output = io.StringIO()
                with patch("sys.argv", ["task-evaluate.py", option, str(source), "--skip-manifest"]), \
                     patch.object(task_evaluate, "execute", return_value=packet), contextlib.redirect_stdout(output):
                    self.assertEqual(task_evaluate.main(), 0)
                report = json.loads(output.getvalue())
                self.assertEqual(report["scenarios"][0]["case_id"], "task-case:test")
                self.assertEqual(report["false_complete_rate"], 0)


    def test_ranked_recall_and_actual_manifest_delivery_are_scored_separately(self):
        packet = {"case_id": "task-case:ranked", "id": "task-case:ranked", "outcome": "partial",
                  "slice": {role: [] for role in task_evaluate.ROLES},
                  "not_covered": [{"area": "producer_boundary", "count": 2000},
                                  {"area": "producer_boundary_scope", "actionable": False}],
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "a", "rank": 1, "bytes": 20}, {"path": "b", "rank": 2, "bytes": 40}]
        manifest = {"kind": "agent_context", "case_id": "task-case:ranked", "files": [
            {"path": "a", "fetch": {"operation": "explain", "arguments": {"report_id": "task-case:ranked", "ref": "/slice/impl/0"}}}],
            "continue_with": {"operation": "page"}, "coverage": {"delivery": {"files": {"omitted": 1}}}}
        calls = []
        def call(operation, arguments):
            calls.append((operation, arguments))
            return manifest if operation == "agent_context" else packet
        result = task_evaluate.evaluate_scenario({"id": "ranked", "task": "inspect a", "required_paths": ["a", "b"]}, call,
                                                manifest_bytes=8000, exclude_facets=("history",))
        self.assertEqual(result["recall_at_k"]["5"], 1)
        self.assertEqual(result["required_path_ranks"], {"a": 1, "b": 2})
        self.assertEqual(result["harness_context"]["required_path_recall"], 0.5)
        self.assertEqual(result["harness_context"]["omitted_files"], 1)
        self.assertEqual(result["actionable_gap_rows"], 1)
        self.assertEqual(result["producer_boundary_occurrences"], 2000)
        self.assertEqual(calls[0][1]["exclude_facets"], ["history"])
        self.assertNotIn("limit", next(args for operation, args in calls if operation == "agent_context"))
        self.assertEqual(result["selected_source_bytes"], 60)
        manifest["case_id"] = "task-case:other"
        with self.assertRaisesRegex(ValueError, "another retained case"):
            task_evaluate.evaluate_scenario({"id": "x", "task": "x", "required_paths": ["a"]}, call, manifest_bytes=8000)

    def test_manifest_byte_limit_and_unpinned_fetches_fail_evaluation(self):
        packet = {"case_id": "task-case:x", "id": "task-case:x", "outcome": "partial", "not_covered": [],
                  "slice": {role: [] for role in task_evaluate.ROLES},
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        manifest = {"kind": "agent_context", "case_id": "task-case:x", "files": [{"path": "a", "fetch": {}}], "continue_with": {}}
        call = lambda op, _: manifest if op == "agent_context" else packet
        scenario = {"id": "x", "task": "x", "required_paths": ["a"]}
        with self.assertRaisesRegex(ValueError, "not pinned"):
            task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)
        manifest["detail"] = "é" * 20000
        with self.assertRaisesRegex(ValueError, "UTF-8 byte budget"):
            task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)

    def test_shared_fetches_and_separate_anchor_preserve_harness_recall(self):
        packet = {"case_id": "task-case:v2", "id": "task-case:v2", "outcome": "partial", "not_covered": [],
                  "slice": {role: [] for role in task_evaluate.ROLES},
                  "coverage": {"delivery": {f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}
        packet["coverage"]["delivery"]["/not_covered"] = {"omitted": 0}
        packet["slice"]["impl"] = [{"path": "a", "rank": 1}, {"path": "b", "rank": 2}]
        manifest = {"schema_version": "2.0", "kind": "agent_context", "case_id": "task-case:v2",
                    "anchor": {"path": "a", "ref": "/slice/impl/0"},
                    "files": [{"path": "b", "ref": "/slice/impl/1", "source_offset": 42}],
                    "fetch_source": {"operation": "explain", "arguments": {"report_id": "task-case:v2"}},
                    "coverage": {"delivery": {"files": {"omitted": 0}}}, "continue_with": None}
        call = lambda operation, _: manifest if operation == "agent_context" else packet
        scenario = {"id": "v2", "task": "repair a", "required_paths": ["a", "b"]}
        result = task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)
        self.assertEqual(result["harness_context"]["required_path_recall"], 1)
        self.assertEqual(result["harness_context"]["delivered_files"], 2)
        self.assertEqual(result["harness_context"]["supplemental_files"], 1)
        self.assertEqual(result["harness_context"]["anchor_path"], "a")
        self.assertEqual(task_evaluate.manifest_source_fetch(manifest, manifest["files"][0]), {
            "operation": "explain", "arguments": {"report_id": "task-case:v2", "ref": "/slice/impl/1", "source_offset": 42}})
        for offset in (-1, True, "42"):
            manifest["files"][0]["source_offset"] = offset
            with self.assertRaisesRegex(ValueError, "not pinned"):
                task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)
        manifest["files"][0]["source_offset"] = 42
        manifest["fetch_source"]["arguments"]["report_id"] = "task-case:other"
        with self.assertRaisesRegex(ValueError, "not pinned"):
            task_evaluate.evaluate_scenario(scenario, call, manifest_bytes=8000)

    def test_delivery_rank_and_global_evidence_rank_are_reported_separately(self):
        packet = {"id": "task-case:diverse", "case_id": "task-case:diverse", "outcome": "partial",
                  "slice": {role: [] for role in task_evaluate.ROLES}, "not_covered": [],
                  "coverage": {"delivery": {"/not_covered": {"omitted": 0}, **{
                      f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}}
        packet["slice"]["impl"] = [{"path": "code.rs", "rank": 1, "candidate_rank": 12}]
        packet["slice"]["doc"] = [{"path": "guide.md", "rank": 2, "candidate_rank": 1}]
        result = task_evaluate.evaluate_scenario({"id": "diverse", "task": "repair", "required_paths": ["code.rs", "guide.md"]}, lambda *_: packet)
        self.assertEqual(result["required_path_ranks"], {"code.rs": 1, "guide.md": 2})
        self.assertEqual(result["required_path_evidence_ranks"], {"code.rs": 12, "guide.md": 1})


    def test_harness_framing_matches_the_actual_adapter_sources(self):
        import re
        root = Path(__file__).resolve().parents[1]
        for harness, path in [("pi", root / "extensions/briefcase-pi.ts"),
                              ("claude-code", root / "src/architecture/adoption.rs")]:
            literal = re.search(r'const CONTEXT_PREFIX[^=]*=\s*("(?:[^"\\]|\\.)*")', path.read_text()).group(1)
            self.assertEqual(task_evaluate.FRAMING[harness], json.loads(literal))

    def test_candidate_diagnostics_distinguish_discovery_selection_and_delivery(self):
        candidates = [
            {"path": "weak", "score": 12, "rank": 20, "selected": False},
            {"path": "crowded", "score": 70, "rank": 40, "role": "impl", "selected": False,
             "signals": [{"facet": "tests", "kind": "TESTS", "strength": 70, "depth": 1}]},
            {"path": "brief_lost", "score": 80, "rank": 2, "selected": True},
            {"path": "harness_lost", "score": 90, "rank": 1, "selected": True},
        ]
        selected = [{"path": "harness_lost", "rank": 1}]
        result = task_evaluate.required_path_diagnostics(
            ["absent", "weak", "crowded", "brief_lost", "harness_lost"], selected, candidates, set(), 24)
        self.assertEqual({path: row["state"] for path, row in result.items()}, {
            "absent": "not_a_candidate", "weak": "below_minimum_score", "crowded": "not_selected",
            "brief_lost": "brief_delivery_omission", "harness_lost": "harness_delivery_omission"})
        self.assertEqual(result["crowded"]["signals"][0]["facet"], "tests")
        unavailable = task_evaluate.required_path_diagnostics(["absent"], [], None, None, 24)
        self.assertEqual(unavailable["absent"]["state"], "candidate_inventory_unavailable")

    def test_retrieval_misses_do_not_require_a_manifest_omission_cursor(self):
        packet = {"case_id": "task-case:x", "outcome": "partial", "slice": {role: [] for role in task_evaluate.ROLES},
                  "not_covered": [], "coverage": {"delivery": {"/not_covered": {"omitted": 0}, **{
                      f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}}
        packet["slice"]["impl"] = [{"path": "found", "rank": 1}]
        manifest = {"kind": "agent_context", "case_id": "task-case:x", "anchor": {"path": "found", "ref": "/slice/impl/0"},
                    "files": [], "fetch_source": {"operation": "explain", "arguments": {"report_id": "task-case:x"}},
                    "continue_with": None}
        result = task_evaluate.evaluate_scenario({"id": "x", "task": "x", "required_paths": ["found", "missing"]},
            lambda operation, _: manifest if operation == "agent_context" else packet, manifest_bytes=8000)
        self.assertEqual(result["status"], "miss")
        self.assertEqual(result["harness_context"]["required_path_recall"], 0.5)

    def test_source_pins_do_not_drift_or_rewrite_explicit_scenario_views(self):
        calls = []
        def raw(operation, arguments):
            calls.append((operation, arguments))
            return {"view_id": "view:stable"}
        call = task_evaluate.PinnedCall(raw, "view:stable")
        call("brief", {"task": "x"})
        call("agent_context", {"task": "x", "view": "latest"})
        self.assertTrue(all(arguments["view"] == "view:stable" for _, arguments in calls))
        with self.assertRaisesRegex(ValueError, "differs"):
            call("brief", {"task": "x", "view": "view:other"})
        with self.assertRaisesRegex(ValueError, "changed"):
            task_evaluate.PinnedCall(lambda *_: {"view_id": "view:other"}, "view:stable")("brief", {})

    def test_ablation_deltas_expose_role_crowding_and_exclude_errors(self):
        base = {"id": "one", "status": "miss", "selected_paths": ["test", "code"], "missing_paths": ["needed"],
                "required_path_recall": 0.5, "selected_role_counts": {role: 0 for role in task_evaluate.ROLES}}
        base["selected_role_counts"].update(test=1, impl=1)
        changed = {**base, "status": "pass", "selected_paths": ["needed", "code"], "missing_paths": [],
                   "required_path_recall": 1.0, "selected_role_counts": {**base["selected_role_counts"], "test": 0, "impl": 2}}
        delta = task_evaluate.ablation_deltas([base], [changed, {"id": "error", "status": "error"}])[0]
        self.assertEqual(delta["required_paths_gained"], ["needed"])
        self.assertEqual(delta["selected_role_count_deltas"]["test"], -1)
        self.assertEqual(delta["selected_paths_removed"], ["test"])

    def test_input_mutations_fail_the_cli_even_when_all_labelled_paths_were_found(self):
        packet = {"id": "task-case:x", "case_id": "task-case:x", "outcome": "partial", "view_id": "view:stable",
                  "slice": {role: [] for role in task_evaluate.ROLES}, "not_covered": [],
                  "coverage": {"delivery": {"/not_covered": {"omitted": 0}, **{
                      f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}}
        packet["slice"]["impl"] = [{"path": "found"}]
        status_reads = []
        def execute(*arguments, **kwargs):
            if arguments[2] == "status":
                status_reads.append(1)
                return {"current_view": "view:stable", "ledger_watermark": len(status_reads)}
            return packet
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "scenarios.json"
            source.write_text(json.dumps({"scenarios": [{"id": "x", "task": "x", "required_paths": ["found"]}]}))
            output = io.StringIO()
            with patch("sys.argv", ["task-evaluate.py", "--scenarios", str(source), "--skip-manifest"]), \
                 patch.object(task_evaluate, "execute", side_effect=execute), contextlib.redirect_stdout(output):
                self.assertEqual(task_evaluate.main(), 1)
            report = json.loads(output.getvalue())
            self.assertEqual(report["passed"], 1)
            self.assertIn("ledger_watermark", report["inputs"]["validation_error"])
            self.assertEqual(report["errors"], 1)


class Stage1ControlTests(unittest.TestCase):
    def test_lexical_only_disables_every_non_lexical_facet(self):
        excluded = task_evaluate.lexical_only_exclusions()
        self.assertNotIn("lexical", excluded)
        self.assertEqual(set(excluded), set(task_evaluate.FACETS) - {"lexical"})
        self.assertEqual(task_evaluate.resolve_exclusions("lexical-only"), excluded)
        self.assertEqual(task_evaluate.resolve_exclusions("history"), ("history",))

    def test_manifest_latency_is_measured_separately_from_brief(self):
        packet = {"case_id": "task-case:t", "id": "task-case:t", "outcome": "partial", "not_covered": [],
                  "slice": {role: [] for role in task_evaluate.ROLES},
                  "coverage": {"delivery": {"/not_covered": {"omitted": 0}, **{
                      f"/slice/{role}": {"omitted": 0} for role in task_evaluate.ROLES}}}}
        packet["slice"]["impl"] = [{"path": "a", "rank": 1}]
        manifest = {"kind": "agent_context", "case_id": "task-case:t",
                    "anchor": {"path": "a", "ref": "/slice/impl/0"}, "files": [],
                    "fetch_source": {"operation": "explain", "arguments": {"report_id": "task-case:t"}},
                    "continue_with": None}
        call = lambda operation, _: manifest if operation == "agent_context" else packet
        result = task_evaluate.evaluate_scenario(
            {"id": "t", "task": "x", "required_paths": ["a"]}, call, manifest_bytes=8000, cold=True)
        self.assertEqual(result["brief_ms"], result["first_packet_ms"])
        self.assertIsNotNone(result["manifest_latency_ms"])
        self.assertEqual(result["timing_phase"], "cold")
        self.assertIsNotNone(result["first_relevant_passage_ms"])
        self.assertIn("integrity_verification_ms", result)


class SourceTransportTests(unittest.TestCase):
    def fixture(self):
        import hashlib
        bodies = {"a": "λ" * 2501, "b": "second\n", "empty": ""}
        selected = [{"path": path, "artifact_id": f"artifact:{path}", "digest": "sha256:" + hashlib.sha256(body.encode()).hexdigest(),
                     "bytes": len(body.encode()), "rank": index + 1} for index, (path, body) in enumerate(bodies.items())]
        packet = {"id": "task-case:fetch", "case_id": "task-case:fetch", "outcome": "partial", "not_covered": [],
                  "slice": {role: selected if role == "impl" else [] for role in task_evaluate.ROLES},
                  "coverage": {"delivery": {key: {"omitted": 0} for key in
                      ["/not_covered", *[f"/slice/{role}" for role in task_evaluate.ROLES]]}}}
        manifest = {"kind": "agent_context", "case_id": packet["id"],
                    "fetch_source": {"operation": "explain", "arguments": {"report_id": packet["id"]}},
                    "anchor": {"path": "a", "ref": "/slice/impl/0", "source_offset": 2},
                    "files": [], "continue_with": {"operation": "page"}}
        calls = []
        def call(operation, arguments):
            calls.append((operation, arguments))
            if operation == "brief":
                return packet
            if operation == "agent_context":
                return json.loads(json.dumps(manifest))
            if operation != "explain":
                raise AssertionError(operation)
            row = selected[int(arguments["ref"].rsplit("/", 1)[1])]
            raw, offset = bodies[row["path"]].encode(), arguments["source_offset"]
            end = min(offset + 4000, len(raw))
            while end < len(raw) and raw[end] & 192 == 128:
                end -= 1
            return {"report_id": packet["id"], "ref": arguments["ref"], "entry": row.copy(),
                    "source_offset": offset, "text": raw[offset:end].decode(),
                    "next_source_offset": end if end < len(raw) else None, "omitted_bytes": len(raw) - end}
        return packet, manifest, call, calls

    def test_full_utf8_sources_include_manifest_omissions_empty_files_and_nonzero_offsets(self):
        packet, manifest, call, calls = self.fixture()
        result = task_evaluate.evaluate_scenario({"id": "fetch", "task": "inspect", "required_paths": ["a", "b"]},
            call, manifest_bytes=8000, verify_sources=True)
        fetched = result["source_fetch"]
        self.assertEqual(fetched["state"], "verified")
        self.assertEqual(fetched["files"], 3)
        self.assertEqual(fetched["bytes"], 5009)
        self.assertFalse(next(row for row in fetched["sources"] if row["path"] == "b")["in_manifest"])
        offsets = [args["source_offset"] for operation, args in calls if operation == "explain" and args["ref"] == "/slice/impl/0"]
        self.assertEqual(offsets, [2, 0, 4000])
        self.assertEqual(result["harness_context"]["delivery_recall"], 1/3)

    def test_corruption_stalled_cursors_wrong_entries_and_budget_notices_fail_source_verification(self):
        changes = [
            {"text": "x" * 4000}, {"next_source_offset": 1}, {"source_offset": True},
            {"entry": {"path": "wrong"}}, {"entry": "malformed"}, {"text": None}, {"omitted_bytes": -1},
            {"report_id": "task-case:wrong"}, {"ref": "/slice/impl/1"},
        ]
        for change in changes:
            with self.subTest(change=change):
                packet, manifest, call, _ = self.fixture()
                def corrupt(operation, arguments):
                    page = call(operation, arguments)
                    return {**page, **change} if operation == "explain" else page
                with self.assertRaises(ValueError):
                    task_evaluate.evaluate_scenario({"id": "bad", "task": "inspect", "required_paths": ["a"]},
                        corrupt, manifest_bytes=8000, verify_sources=True)

    def test_correct_full_digest_still_rejects_a_wrong_advertised_excerpt(self):
        _, _, call, _ = self.fixture()
        def corrupt(operation, arguments):
            page = call(operation, arguments)
            if operation == "explain" and arguments["source_offset"] == 2:
                page["text"] = "μ" * (len(page["text"].encode()) // 2)
            return page
        with self.assertRaisesRegex(ValueError, "Advertised source span"):
            task_evaluate.evaluate_scenario({"id": "bad", "task": "inspect", "required_paths": ["a"]},
                corrupt, manifest_bytes=8000, verify_sources=True)

    def test_byte_digest_is_checked_even_when_page_counts_and_entries_agree(self):
        _, _, call, _ = self.fixture()
        def corrupt(operation, arguments):
            page = call(operation, arguments)
            if operation == "explain":
                page["text"] = page["text"].replace("λ", "μ")
            return page
        with self.assertRaisesRegex(ValueError, "digest differs"):
            task_evaluate.evaluate_scenario({"id": "bad", "task": "inspect", "required_paths": ["a"]},
                corrupt, manifest_bytes=8000, verify_sources=True)

    def test_budget_sweep_checks_same_case_order_and_total_pi_bytes(self):
        _, manifest, call, _ = self.fixture()
        result = task_evaluate.evaluate_scenario({"id": "sweep", "task": "inspect", "required_paths": ["a"]},
            call, manifest_bytes=8000, budget_sweep=(8000, 3000, 4000, 6000))
        self.assertEqual([row["budget"] for row in result["budget_sweep"]], [3000,4000,6000,8000])
        self.assertTrue(all(row["bytes"] <= row["budget"] for row in result["budget_sweep"]))
        def reorder(operation, arguments):
            page = call(operation, arguments)
            if operation == "agent_context":
                paths = ["b", "empty"] if arguments["max_bytes"] < 5000 else ["empty", "b"]
                page["files"] = [{"path": path, "ref": "/slice/impl/1"} for path in paths]
            return page
        with self.assertRaisesRegex(ValueError, "changed source order"):
            task_evaluate.evaluate_scenario({"id": "bad", "task": "inspect", "required_paths": ["a"]},
                reorder, manifest_bytes=8000, budget_sweep=(3000,8000))

    def test_diagnostics_preserve_hub_penalties_and_role_competitors(self):
        packet, _, call, _ = self.fixture()
        packet["coverage"]["delivery"]["/candidates"] = {"omitted": 0}
        packet["candidates"] = [{"path": "missing", "rank": 33, "score": 51, "selected": False, "role": "impl",
            "signals": [{"facet": "dependency", "kind": "IMPORTS", "strength": 51,
                "provenance": {"hub_degree": 16, "hub_penalty": 45}}]}]
        result = task_evaluate.evaluate_scenario({"id": "miss", "task": "inspect", "required_paths": ["missing"]}, call, diagnostics=True)
        self.assertEqual(result["required_path_diagnostics"]["missing"]["signals"][0]["provenance"]["hub_penalty"],45)
        self.assertEqual(result["retrieval_context"]["unselected_role_leaders"]["impl"][0]["path"],"missing")


if __name__ == "__main__":
    unittest.main()
