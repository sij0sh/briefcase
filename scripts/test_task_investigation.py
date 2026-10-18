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
            return self.page([{ "from_files": ["src/consumer.rs"], "to_files": ["src/a.rs"]}], args, op)
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
        self.flow.save()
        self.assertEqual(self.flow.state["context_readiness"], "blocked")

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


if __name__ == "__main__":
    unittest.main()
