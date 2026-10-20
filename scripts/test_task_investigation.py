"""The workflow must find follow-ups without access to authored answer paths."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import subprocess
import sys

from task_investigation import Investigation


class Protocol:
    def __init__(self):
        self.view = "view:one"
        self.alignment = "matches"
        self.pending = False
        self.corrupt = False
        self.calls = []
        self.contents = {"src/a.rs": "// anchor 雪\n", "src/b.rs": "// further é雪\n" * 180,
                         "scripts/check": "cat src/a.rs\n", "src/consumer.rs": "// graph-only consumer\n"}
        self.paths = list(self.contents)[:3]

    def __call__(self, op, args):
        self.calls.append((op, copy.deepcopy(args)))
        if op == "agent_context":
            return {"kind": "agent_context", "case_id": "task-case:one", "view_id": self.view,
                    "index_id": "task-index:one", "files": [{"path": "src/a.rs"}],
                    "coverage": {"completeness": {"verification": "unknown"}}}
        if op == "status":
            return {"current_view": self.view, "pending_or_interrupted": self.pending, "retracted_count": 0}
        if op == "worktree":
            return {"alignment": self.alignment, "tree_digest": "sha256:tree", "changed_paths": int(self.alignment != "matches"),
                    "outside_initial_slice": int(self.alignment != "matches"), "report_id": "context-report:tree"}
        if op == "explain":
            ref = args["ref"]
            if ref == "/coverage/artifact_roles":
                return {"report_id": args["report_id"], "entry": {"impl": {"captured_files": 3}, "script": {"captured_files": 1}, "test": {"captured_files": 0}}}
            if ref == "/coverage/required_roles":
                return {"report_id": args["report_id"], "entry": ["impl", "script", "test"]}
            if ref.startswith("/candidates/"):
                path = self.paths[int(ref.rsplit("/", 1)[1])]
            else:
                path = ref.removeprefix("/files/").replace("~1", "/").replace("~0", "~")
            raw = self.contents[path].encode()
            offset = args.get("source_offset", 0)
            end = min(offset + 80, len(raw))
            while end < len(raw) and raw[end] & 0xC0 == 0x80:
                end -= 1
            body = raw[offset:end].decode()
            return {"view_id": "view:one", "index_id": "task-index:one", "report_id": args["report_id"], "ref": ref,
                    "entry": {"path": path, "artifact_id": "artifact:" + path, "bytes": len(raw),
                              "digest": "sha256:" + ("0" * 64 if self.corrupt else hashlib.sha256(raw).hexdigest())},
                    "source_offset": offset, "next_source_offset": end if end < len(raw) else None,
                    "omitted_bytes": len(raw) - end, "text": body}
        if op == "neighbors":
            return self.page([{"id": "task-link:one", "from_files": ["src/consumer.rs"], "to_files": ["src/a.rs"]}], args, op)
        if op == "occurrences":
            return self.page([{"path": "src/consumer.rs"}], args, op)
        if op == "page":
            section = args.get("section")
            if section == "/candidates":
                items = [{"path": path, "selected": index == 0, "source_ref": f"/candidates/{index}"}
                         for index, path in enumerate(self.paths)]
            else:
                items = [{"area": "executed_test_coverage", "dimension": "verification"}]
            return self.page(items, args, "page")
        raise AssertionError((op, args))

    def page(self, items, args, op):
        offset = args.get("offset", 0)
        chunk = items[offset:offset + 1]
        end = offset + len(chunk)
        return {"report_id": args.get("report_id", "context-report:enumerator"), "view_id": "view:one", "index_id": "task-index:one", "items": chunk,
                "page": {"offset": offset, "returned": len(chunk), "total": len(items), "next_offset": end if end < len(items) else None},
                "continue_with": {"operation": op, "arguments": {**args, "offset": end}} if end < len(items) else None}


class InvestigationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.protocol = Protocol()
        self.flow = Investigation(self.protocol, self.directory.name)
        self.flow.start({"task": "refactor storage", "target": "src/a.rs"}, budget=3000)

    def drain(self):
        steps = 0
        while self.flow.state["queue"]:
            self.flow.step(); steps += 1
            self.assertLess(steps, 200)

    def test_candidate_and_graph_followups_recover_source_without_gold_labels(self):
        self.drain()

        self.assertEqual(set(self.flow.state["sources"]), set(self.protocol.contents))
        self.assertTrue(all(s["state"] == "digest_verified" for s in self.flow.state["sources"].values()))
        self.assertEqual(self.flow.state["context_readiness"], "open")
        self.assertEqual(self.flow.state["verification"]["state"], "pending")
        self.assertEqual(self.flow.state["requirements"]["role:test"]["status"], "not_applicable")
        self.assertTrue(any(args.get("source_offset", 0) > 0 for op, args in self.protocol.calls if op == "explain"))

    def test_resume_preserves_progress_and_does_not_refetch_completed_source(self):
        for _ in range(7):
            self.flow.step()
        state = json.loads((Path(self.directory.name) / "state.json").read_text())
        self.flow = Investigation(self.protocol, self.directory.name, state)
        self.assertTrue(self.flow.check_current())
        self.drain()
        first_reads = [args["ref"] for op, args in self.protocol.calls if op == "explain" and args.get("source_offset") == 0]
        self.assertEqual(len(first_reads), len(set(first_reads)))

    def test_changed_out_of_slice_source_invalidates_the_remaining_queue(self):
        self.protocol.alignment = "changed"
        self.assertFalse(self.flow.check_current())
        self.assertEqual(self.flow.state["worktree"]["outside_initial_slice"], 1)
        with self.assertRaisesRegex(ValueError, "Live source differs"):
            self.flow.step()
        with self.assertRaisesRegex(ValueError, "invalidated"):
            self.flow.conclude({"claim_id": "c", "text": "x", "citations": []})
        self.flow.save()
        self.assertEqual(self.flow.state["context_readiness"], "blocked")

    def test_unknown_worktree_alignment_does_not_invalidate_traversal(self):
        self.protocol.alignment = "unknown"
        self.assertTrue(self.flow.check_current())
        self.assertEqual(self.flow.state["requirements"]["scope"]["status"], "open")

    def test_pin_change_and_pending_capture_are_not_retrieval_completion(self):
        self.protocol.view = "view:two"
        self.assertFalse(self.flow.check_current())
        self.assertEqual(self.flow.state["status"], "invalidated")

    def test_budget_truncated_status_blocks_without_invalidating(self):
        protocol = self.flow.call
        self.flow.call = lambda op, args: {"truncated": True, "pinned_views": ["view:one"]} if op == "status" else protocol(op, args)
        with self.assertRaisesRegex(ValueError, "increase --max-bytes"):
            self.flow.check_current()
        self.assertNotEqual(self.flow.state["status"], "invalidated")

    def test_assessment_requires_delivered_evidence_and_does_not_clear_verification(self):
        self.drain()
        event_id = self.flow.state["events"][-1]["id"]
        claim = {"status": "supported", "reason": "Reviewed the relevant source", "evidence": [event_id]}
        with self.assertRaisesRegex(ValueError, "evidence"):
            self.flow.assess({"requirements": {"implementation": claim}})
        self.flow.read_event(event_id)
        self.flow.assess({"requirements": {name: claim for name in self.flow.state["requirements"] if name != "scope"}})
        self.flow.save()
        self.assertEqual(self.flow.state["context_readiness"], "ready")
        self.assertEqual(self.flow.state["verification"]["state"], "pending")

    def test_conclusion_requires_inspected_citations_and_unique_claim(self):
        self.drain()
        source = self.flow.state["sources"]["src/a.rs"]
        citation = {"path": "src/a.rs", "digest": source["identity"]["digest"],
                    "start_byte": 0, "end_byte": 1}
        conclusion = {"claim_id": "c", "text": "Pinned bytes were inspected.",
                      "citations": [citation]}
        self.flow.conclude(conclusion)
        self.flow.conclude({"claim_id": "r", "text": "A retained link exists.",
                            "citations": [], "relationship_ids": ["task-link:one"]})
        with self.assertRaisesRegex(ValueError, "at most once"):
            self.flow.conclude(conclusion)
        for bad in (
            {**conclusion, "claim_id": "bad-digest", "citations": [{**citation, "digest": "sha256:bad"}]},
            {**conclusion, "claim_id": "bad-range", "citations": [{**citation, "end_byte": 9999}]},
            {**conclusion, "claim_id": "bad-link", "citations": [], "relationship_ids": ["task-link:missing"]},
            {**conclusion, "claim_id": "no-evidence", "citations": []},
        ):
            with self.assertRaises(ValueError):
                self.flow.conclude(bad)
        self.flow.save()
        state = json.loads((Path(self.directory.name) / "state.json").read_text())
        self.assertEqual([row["claim_id"] for row in state["conclusions"]], ["c", "r"])

    def test_stalled_pages_and_corrupted_bytes_leave_the_action_pending(self):
        self.protocol.corrupt = True
        with self.assertRaisesRegex(ValueError, "digest"):
            self.drain()
        committed = json.loads((Path(self.directory.name) / "state.json").read_text())
        self.assertTrue(committed["queue"])
        self.assertNotEqual(committed["context_readiness"], "ready")

    def test_queries_cannot_change_pins_or_execute_commands(self):
        with self.assertRaisesRegex(ValueError, "source pin"):
            self.flow.add_query({"operation": "occurrences", "arguments": {"text": "save", "view": "view:other"}})
        with self.assertRaisesRegex(ValueError, "Additional investigations"):
            self.flow.add_query({"operation": "sync"})
        self.flow.add_query({"operation": "occurrences", "arguments": {"text": "save"}})
        self.drain()

    def test_protected_structural_queue_recovers_a_non_candidate_and_resumes(self):
        class StructuralProtocol(Protocol):
            def __call__(self, op, args):
                if op == "inventory":
                    self.calls.append((op, copy.deepcopy(args)))
                    self.assert_filter = args.get("graph_only") is True and args.get("selected") is False
                    return {**self.page([{"path": "src/consumer.rs", "source_ref": "/files/src~1consumer.rs"}], args, op),
                            "fetch_source": {"arguments": {"report_id": "task-index:one"}}}
                result = super().__call__(op, args)
                if op == "agent_context":
                    result["files"][0]["ref"] = "/files/src~1a.rs"
                return result
        protocol = StructuralProtocol()
        with tempfile.TemporaryDirectory() as directory:
            flow = Investigation(protocol, directory)
            flow.start({"task": "inspect"}, mode="structural-explore", verify_sources=False)
            self.assertEqual(flow.next_action()["kind"], "structural_candidates")
            flow.step()
            # The protected action does not advance the ordinary excerpt phase.
            self.assertNotIn("excerpt_turn", flow.state)
            self.assertEqual(flow.next_action()["kind"], "roles")
            flow.step()
            self.assertEqual(flow.state["excerpt_turn"], 1)
            self.assertEqual(flow.next_action()["kind"], "excerpt")
            for _ in range(4):
                flow.step()
            state = json.loads((Path(directory) / "state.json").read_text())
            flow = Investigation(protocol, directory, state)
            for _ in range(60):
                if not flow.state["queue"]:
                    break
                flow.step()
            self.assertTrue(protocol.assert_filter)
            self.assertIn("src/consumer.rs", flow.state["sources"])
            self.assertEqual(flow.state["sources"]["src/consumer.rs"]["inspection_state"], "inspected_pinned_range")
            self.assertNotIn("src/consumer.rs", protocol.paths)
            exploration = flow.state["exploration"]
            self.assertLessEqual(len(exploration["roots"]), 4)
            self.assertLessEqual(len(exploration["paths"]), 16)
            self.assertTrue(any(op == "neighbors" and args.get("relationship_scope") == "all" for op, args in protocol.calls))

    def test_bounded_investigation_never_spends_the_last_oversized_response(self):
        from task_investigation import run_bounded_investigation
        protocol = Protocol()
        original = protocol
        def padded(op, args):
            response = original(op, args)
            if op == "worktree":
                response["padding"] = "x" * 35000
            return response
        with tempfile.TemporaryDirectory() as directory:
            result = run_bounded_investigation(padded, directory, {"id": "x", "task": "inspect", "required_paths": ["src/a.rs"]},
                max_calls=100, max_bytes=70000)
            self.assertEqual(result["limit_reached"], "bytes")
            self.assertEqual(result["byte_limit_overshoot"], 0)
            self.assertLessEqual(result["bytes"], 70000)


class ManifestFirstTests(unittest.TestCase):
    def protocol_with_manifest(self):
        class ManifestProtocol(Protocol):
            def __call__(self, op, args):
                result = super().__call__(op, args)
                if op == "agent_context":
                    result["anchor"] = {"path": "src/a.rs", "ref": "/files/src~1a.rs"}
                    result["files"] = [{"path": "src/b.rs", "ref": "/files/src~1b.rs"}]
                return result
        return ManifestProtocol()

    def test_manifest_first_reads_anchor_and_selected_before_candidates(self):
        with tempfile.TemporaryDirectory() as directory:
            flow = Investigation(self.protocol_with_manifest(), directory)
            flow.start({"task": "refactor storage", "target": "src/a.rs"}, budget=3000, mode="manifest-first")
            self.assertEqual(flow.state["mode"], "manifest_first")
            self.assertEqual(flow.state["manifest"]["paths"], ["src/a.rs", "src/b.rs"])
            queue = flow.state["queue"]
            self.assertEqual([a["kind"] for a in queue[:2]], ["source", "source"])
            self.assertEqual([a["path"] for a in queue[:2]], ["src/a.rs", "src/b.rs"])
            self.assertTrue(any(a["kind"] == "candidates" for a in queue[2:]))

    def test_exhaustive_mode_keeps_candidate_traversal_without_manifest_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            flow = Investigation(self.protocol_with_manifest(), directory)
            flow.start({"task": "refactor storage", "target": "src/a.rs"}, budget=3000, mode="exhaustive")
            self.assertEqual(flow.state["mode"], "exhaustive")
            self.assertFalse(any(a["kind"] == "source" for a in flow.state["queue"]))

    def test_unknown_mode_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            flow = Investigation(Protocol(), directory)
            with self.assertRaisesRegex(ValueError, "mode"):
                flow.start({"task": "x"}, budget=3000, mode="sideways")

    def test_bounded_investigation_reports_call_limit(self):
        from task_investigation import run_bounded_investigation
        protocol = self.protocol_with_manifest()
        with tempfile.TemporaryDirectory() as directory:
            result = run_bounded_investigation(protocol, directory,
                {"task": "refactor storage", "target": "src/a.rs", "required_paths": ["src/a.rs"]},
                mode="manifest-first", max_calls=3, max_bytes=2_000_000, max_seconds=60, budget=3000)
            self.assertEqual(result["limit_reached"], "calls")
            self.assertLessEqual(result["calls"], 3)
            self.assertIn("recovered_required_paths", result)


class InvestigationCliTests(unittest.TestCase):
    def test_cli_resumes_and_delivers_evidence_inside_the_complete_output_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "briefcase-stub"
            scripts = Path(__file__).resolve().parent
            binary.write_text(f"#!{sys.executable}\nimport sys,json\nsys.path.insert(0,{str(scripts)!r})\n"
                              "from test_task_investigation import Protocol\n"
                              "operation=sys.argv[sys.argv.index('call')+1]\n"
                              "args=json.loads(sys.argv[sys.argv.index('--args')+1])\n"
                              "print(json.dumps(Protocol()(operation,args),ensure_ascii=False))\n")
            binary.chmod(0o700)
            command = [sys.executable, "-B", str(scripts / "task-investigate.py"), "--binary", str(binary),
                       "--root", str(root), "--state-dir", str(root / "state"), "--max-bytes", "3000"]
            first = subprocess.run([*command, "--task", "refactor storage", "--target", "src/a.rs"], capture_output=True, timeout=15)
            self.assertEqual(first.returncode, 0, first.stderr.decode())
            self.assertLessEqual(len(first.stdout), 3000)
            response = json.loads(first.stdout)
            event_id = response["event_id"]
            resumed = subprocess.run(command, capture_output=True, timeout=15)
            self.assertEqual(resumed.returncode, 0, resumed.stderr.decode())
            self.assertGreater(json.loads(resumed.stdout)["event_count"], response["event_count"])
            delivered = subprocess.run([*command, "--read-event", event_id], capture_output=True, timeout=15)
            self.assertEqual(delivered.returncode, 0, delivered.stderr.decode())
            self.assertEqual(json.loads(delivered.stdout)["event_id"], event_id)
            self.assertLessEqual(len(delivered.stdout), 3000)

    def test_cli_records_a_cited_conclusion_in_the_investigation_state(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "briefcase-stub"
            scripts = Path(__file__).resolve().parent
            binary.write_text(f"#!{sys.executable}\nimport sys,json\nsys.path.insert(0,{str(scripts)!r})\n"
                              "from test_task_investigation import ManifestFirstTests\n"
                              "operation=sys.argv[sys.argv.index('call')+1]\n"
                              "args=json.loads(sys.argv[sys.argv.index('--args')+1])\n"
                              "protocol=ManifestFirstTests().protocol_with_manifest()\n"
                              "print(json.dumps(protocol(operation,args),ensure_ascii=False))\n")
            binary.chmod(0o700)
            command = [sys.executable, "-B", str(scripts / "task-investigate.py"), "--binary", str(binary),
                       "--root", str(root), "--state-dir", str(root / "state"), "--max-bytes", "3000"]
            first = subprocess.run([*command, "--task", "refactor storage", "--target", "src/a.rs"],
                                   capture_output=True, timeout=15)
            self.assertEqual(first.returncode, 0, first.stderr.decode())
            state = json.loads((root / "state" / "state.json").read_text())
            citation = {"path": "src/a.rs", "digest": state["sources"]["src/a.rs"]["identity"]["digest"],
                        "start_byte": 0, "end_byte": 1}
            conclusion = root / "conclusion.json"
            conclusion.write_text(json.dumps({"claim_id": "c", "text": "Pinned bytes were inspected.",
                                              "citations": [citation]}))
            recorded = subprocess.run([*command, "--conclusion", str(conclusion)], capture_output=True, timeout=15)
            self.assertEqual(recorded.returncode, 0, recorded.stderr.decode())
            self.assertEqual(json.loads(recorded.stdout)["conclusions"], 1)
            state = json.loads((root / "state" / "state.json").read_text())
            self.assertEqual(state["conclusions"][0]["claim_id"], "c")


class FocusedInvestigationTests(unittest.TestCase):
    def protocol(self):
        class FocusedProtocol(ManifestFirstTests().protocol_with_manifest().__class__):
            def __call__(self, op, args):
                result = super().__call__(op, args)
                if op == "agent_context":
                    result["files"][0]["source_offset"] = 100
                return result
        protocol = FocusedProtocol()
        protocol.contents["src/b.rs"] = "// oversized anchor\n" * 10000
        return protocol

    def test_excerpt_first_reaches_tail_before_finishing_a_large_manifest_file(self):
        from task_investigation import run_bounded_investigation
        scenario = {"task": "refactor storage", "target": "src/a.rs", "required_paths": ["scripts/check"]}
        with tempfile.TemporaryDirectory() as directory:
            baseline = run_bounded_investigation(self.protocol(), Path(directory) / "old", scenario,
                                                 max_calls=22, budget=3000)
            focused = run_bounded_investigation(self.protocol(), Path(directory) / "new", scenario,
                                                mode="excerpt-first", max_calls=22, budget=3000)
            self.assertEqual(baseline["recovered_beyond_manifest"], [])
            self.assertEqual(focused["recovered_beyond_manifest"], ["scripts/check"])
            self.assertGreater(focused["candidate_inventory_paths"], 0)
            self.assertIn("source", focused["remaining_actions"])

    def test_localized_excerpt_is_pinned_and_resume_verifies_the_whole_file(self):
        with tempfile.TemporaryDirectory() as directory:
            protocol = self.protocol()
            protocol.contents["src/b.rs"] = "// ASCII localized bytes\n" * 12
            flow = Investigation(protocol, directory)
            flow.start({"task": "refactor storage"}, mode="excerpt-first")
            while not flow.state["sources"]["src/b.rs"].get("excerpts"):
                flow.step()
            source = flow.state["sources"]["src/b.rs"]
            self.assertEqual(source["excerpts"][0]["start"], 100)
            self.assertEqual(source["state"], "excerpt_inspected")
            self.assertEqual(source["ranges"], [])
            flow = Investigation(protocol, directory, json.loads((Path(directory) / "state.json").read_text()))
            while flow.state["queue"]:
                flow.step()
            self.assertEqual(flow.state["sources"]["src/b.rs"]["state"], "digest_verified")
            reads = [args["source_offset"] for op, args in protocol.calls
                     if op == "explain" and args.get("ref") == "/files/src~1b.rs"]
            self.assertEqual(reads[:2], [100, 0])

    def test_inventory_first_enumerates_all_candidates_before_source_reading(self):
        with tempfile.TemporaryDirectory() as directory:
            protocol = self.protocol()
            flow = Investigation(protocol, directory)
            flow.start({"task": "refactor storage"}, mode="inventory-first")
            while flow.next_action()["kind"] != "excerpt":
                flow.step()
            self.assertEqual(flow.state["candidate_inventory"], protocol.paths)
            self.assertFalse(any(src["ranges"] or src.get("excerpts") for src in flow.state["sources"].values()))

    def test_excerpt_rejects_identity_changes_before_full_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            protocol = self.protocol()
            flow = Investigation(protocol, directory)
            flow.start({"task": "refactor storage"}, mode="excerpt-first")
            while not flow.state["sources"]["src/b.rs"].get("excerpts"):
                flow.step()
            protocol.contents["src/b.rs"] += "changed"
            with self.assertRaisesRegex(ValueError, "identity changed"):
                while flow.state["queue"]:
                    flow.step()


class IndependentEvidenceStagesTests(unittest.TestCase):
    class CompactProtocol(Protocol):
        def __call__(self, op, args):
            if op == "inventory":
                self.calls.append((op, copy.deepcopy(args)))
                items = [{"path": path, "source_ref": f"/candidates/{index}", "source_offset": 0}
                         for index, path in enumerate(self.paths)]
                result = self.page(items, args, "inventory")
                result["fetch_source"] = {"operation": "explain", "arguments": {"report_id": args["case_id"]}}
                return result
            if op == "expand_case":
                self.calls.append((op, copy.deepcopy(args)))
                return {"kind": "case_revision", "parent_case_id": args["case_id"], "case_id": "task-case:child",
                        "view_id": self.view, "index_id": "task-index:one"}
            return super().__call__(op, args)

    def test_inspection_verification_and_assembly_require_independent_progress(self):
        with tempfile.TemporaryDirectory() as directory:
            protocol = self.CompactProtocol()
            flow = Investigation(protocol, directory)
            flow.start({"task": "refactor storage"}, mode="excerpt-first", compact_inventory=True, verify_sources=False)
            while flow.state["queue"]:
                flow.step()
            source = flow.state["sources"]["src/b.rs"]
            self.assertEqual(source["inspection_state"], "inspected_pinned_range")
            self.assertEqual(source["verification_state"], "not_requested")
            self.assertEqual(flow.state["case_id"], "task-case:one")
            self.assertEqual(flow.state["case_revisions"], [])
            flow = Investigation(protocol, directory, json.loads((Path(directory) / "state.json").read_text()))
            flow.verify_source("src/b.rs")
            while flow.state["queue"]:
                flow.step()
            self.assertEqual(flow.state["sources"]["src/b.rs"]["verification_state"], "digest_verified")
            self.assertEqual(flow.state["case_id"], "task-case:one")
            evidence = flow.state["events"][-1]["id"]
            flow.read_event(evidence)
            flow.assess({"requirements": {"implementation": {"status": "supported", "reason": "Reviewed prior scope", "evidence": [evidence]}}})
            flow.add_query({"operation": "expand_case", "arguments": {"paths": ["src/b.rs"]}})
            self.assertEqual(flow.state["case_id"], "task-case:child")
            self.assertEqual(flow.state["case_revisions"][0]["parent_case_id"], "task-case:one")
            self.assertEqual(flow.state["sources"]["src/b.rs"]["report_id"], "task-case:one")
            self.assertEqual(flow.state["requirements"]["implementation"]["status"], "open")
            self.assertEqual(flow.state["case_revisions"][0]["previous_requirements"]["implementation"]["status"], "supported")

    def test_queries_cannot_replace_the_case_pin(self):
        with tempfile.TemporaryDirectory() as directory:
            flow = Investigation(self.CompactProtocol(), directory)
            flow.start({"task": "refactor storage"}, compact_inventory=True)
            for operation in ("inventory", "expand_case"):
                with self.assertRaisesRegex(ValueError, "case"):
                    flow.add_query({"operation": operation, "arguments": {"case_id": "task-case:other"}})


if __name__ == "__main__":
    unittest.main()
