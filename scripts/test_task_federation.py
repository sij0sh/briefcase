"""Federation joins repositories only through declared evidence at pinned views."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from task_federation import Federation, manifest_facts

BINARY = Path(__file__).resolve().parent.parent / "target/release/briefcase"


class Repository:
    def __init__(self, view, files):
        self.view, self.files, self.calls = view, files, []

    def __call__(self, operation, arguments):
        self.calls.append(operation)
        if operation == "status":
            return {"current_view": self.view, "pending_or_interrupted": False}
        if operation == "agent_context":
            return {"case_id": "task-case:" + self.view, "view_id": self.view, "index_id": "task-index:" + self.view}
        if operation == "worktree":
            return {"alignment": "matches", "tree_digest": "sha256:tree", "coverage": {"inventory": {"capture_head": "rev-" + self.view}}}
        if operation == "explain":
            path = arguments["ref"].removeprefix("/files/").replace("~1", "/").replace("~0", "~")
            if path not in self.files:
                raise RuntimeError("EXPLAIN: Unknown report reference")
            raw = self.files[path].encode()
            return {"view_id": self.view, "source_offset": 0, "next_source_offset": None, "text": self.files[path],
                    "entry": {"path": path, "artifact_id": "artifact:" + path, "digest": "sha256:" + hashlib.sha256(raw).hexdigest()}}
        if operation == "occurrences":
            rows = [{"path": p, "line": n + 1, "artifact_id": "artifact:" + p} for p, text in sorted(self.files.items())
                    for n, line in enumerate(text.splitlines()) if arguments["text"] in line]
            start, limit = arguments["offset"], arguments["limit"]
            more = start + limit < len(rows)
            return {"items": rows[start:start + limit], "page": {"offset": start, "next_offset": start + limit if more else None}}
        raise AssertionError(operation)


def workspace(*ids, contracts=()):
    return {"repositories": [{"id": i, "root": i} for i in ids], "contracts": list(contracts)}


class FederationTest(unittest.TestCase):
    def setUp(self):
        self.api = Repository("view:3", {"package.json": json.dumps({"name": "@acme/api", "version": "2.1.0"}),
                                         "src/index.js": "export function getUser() {}\n",
                                         "schema/api.json": '{"operations":["getUser"]}\n'})
        self.web = Repository("view:7", {"package.json": json.dumps({"name": "web", "dependencies": {"@acme/api": "^2.0.0"}}),
                                         "src/index.js": "import { getUser } from '@acme/api';\ngetUser();\n"})

    def run_federation(self, repositories, contracts=()):
        with tempfile.TemporaryDirectory() as directory:
            def caller(repo):
                if repo["id"] not in repositories:
                    raise OSError("root does not exist")
                return repositories[repo["id"]]
            state = Federation(workspace("api", "web", contracts=contracts), caller, Path(directory)).run("change getUser")
            self.assertTrue((Path(directory) / "federation.json").is_file())
            return state

    def test_declared_dependency_joins_provider_contract_to_consumer_uses_at_both_pins(self):
        state = self.run_federation({"api": self.api, "web": self.web})
        self.assertEqual({k: v["view_id"] for k, v in state["version_vector"].items()}, {"api": "view:3", "web": "view:7"})
        self.assertEqual(state["version_vector"]["web"]["revision"], "rev-view:7")
        [edge] = state["edges"]
        self.assertEqual((edge["provider"], edge["consumer"], edge["declared_requirement"], edge["provider_version"]),
                         ("api", "web", "^2.0.0", "2.1.0"))
        # Same relative path in both repositories, distinct qualified references.
        self.assertEqual([r["ref"] for r in edge["evidence"]], ["api@view:3:package.json", "web@view:7:package.json"])
        [uses] = state["consumer_evidence"]
        self.assertTrue(uses["complete"])
        self.assertIn("web@view:7:src/index.js", [r["ref"] for r in uses["occurrences"]])
        self.assertEqual(state["coverage"], "all_declared_repositories_pinned")
        self.assertEqual(state["compatibility"]["state"], "declared_only")

    def test_explicit_contract_terms_are_searched_in_bound_consumers(self):
        contract = {"provider": "api", "path": "schema/api.json", "terms": ["getUser"], "consumers": ["web"]}
        state = self.run_federation({"api": self.api, "web": self.web}, [contract])
        self.assertEqual(state["contracts"][0]["ref"], "api@view:3:schema/api.json")
        uses = next(e for e in state["consumer_evidence"] if e["term"] == "getUser")
        self.assertEqual(len(uses["occurrences"]), 2)

    def test_name_similarity_alone_is_not_an_edge(self):
        self.web.files["package.json"] = json.dumps({"name": "web"})
        state = self.run_federation({"api": self.api, "web": self.web})
        self.assertEqual(state["edges"], [])
        self.assertEqual(state["consumer_evidence"], [])

    def test_unavailable_consumer_is_a_boundary_not_absence_of_impact(self):
        contract = {"provider": "api", "path": "schema/api.json", "terms": ["getUser"], "consumers": ["web"]}
        state = self.run_federation({"api": self.api}, [contract])
        self.assertEqual(state["coverage"], "partial")
        self.assertTrue(any(b["repository"] == "web" and "Inspect web" in b["required_inspection"] for b in state["boundaries"]))

    def test_view_change_during_federation_invalidates_the_join(self):
        original = self.web.__call__
        calls = {"status": 0}

        def moving(operation, arguments):
            if operation == "status":
                calls["status"] += 1
                if calls["status"] > 1:
                    return {"current_view": "view:8", "pending_or_interrupted": False}
            return original(operation, arguments)
        with self.assertRaisesRegex(ValueError, "changed during federation"):
            self.run_federation({"api": self.api, "web": moving})

    def test_tampered_source_bytes_are_rejected(self):
        original = self.api.__call__

        def tampered(operation, arguments):
            result = original(operation, arguments)
            if operation == "explain":
                result = {**result, "text": result["text"] + " "}
            return result
        with self.assertRaisesRegex(ValueError, "digest"):
            self.run_federation({"api": tampered, "web": self.web})

    def test_manifest_dialects(self):
        self.assertEqual(manifest_facts("Cargo.toml", '[package]\nname="core"\nversion="1.0.0"\n[dependencies]\nserde="1"\nshared={path="../shared",package="acme-shared"}\n')[1].keys(),
                         {"serde", "acme-shared"})
        provides, requires = manifest_facts("pyproject.toml", '[project]\nname="Acme_Client"\ndependencies=["acme-api>=2","requests"]\n')
        self.assertEqual((list(provides), set(requires)), (["acme-client"], {"acme-api", "requests"}))
        provides, requires = manifest_facts("go.mod", "module example.com/web\nrequire (\n\texample.com/api v1.2.0\n)\nrequire example.com/x v0.1.0\n")
        self.assertEqual((list(provides), requires), (["example.com/web"], {"example.com/api": "v1.2.0", "example.com/x": "v0.1.0"}))


@unittest.skipUnless(BINARY.is_file(), "release binary not built")
class TwoRepositorySmoke(unittest.TestCase):
    def test_real_binary_joins_contract_and_consumer_at_committed_revisions(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            files = {"api": {"package.json": '{"name":"@acme/api","version":"2.1.0"}\n', "src/index.js": "export function getUser() {}\n"},
                     "web": {"package.json": '{"name":"web","dependencies":{"@acme/api":"^2.0.0"}}\n',
                             "src/index.js": "import { getUser } from '@acme/api';\n"}}
            for name, tree in files.items():
                for path, text in tree.items():
                    (base / name / path).parent.mkdir(parents=True, exist_ok=True)
                    (base / name / path).write_text(text)
                git = ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@local.invalid", "-c", "commit.gpgsign=false"]
                for command in (["git", "init", "-q"], ["git", "add", "."], git + ["commit", "-qm", "fixture"]):
                    subprocess.run(command, cwd=base / name, check=True, capture_output=True)
                # Capture is explicit; the coordinator only reads existing pins.
                subprocess.run([str(BINARY), "--db", str(base / f"{name}.db"), "--project-root", str(base / name), "sync"],
                               cwd=base / name, check=True, capture_output=True, timeout=300)
            (base / "workspace.json").write_text(json.dumps({"repositories": [
                {"id": "api", "root": "api", "db": "api.db"}, {"id": "web", "root": "web", "db": "web.db"},
                {"id": "mobile", "root": "mobile"}]}))
            script = Path(__file__).resolve().parent / "task-federate.py"
            result = subprocess.run([sys.executable, "-B", str(script), "--workspace", str(base / "workspace.json"),
                                     "--task", "change getUser", "--binary", str(BINARY), "--output", str(base / "out")],
                                    capture_output=True, text=True, timeout=300)
            self.assertEqual(result.returncode, 0, result.stderr)
            state = json.loads((base / "out/federation.json").read_text())
            for name in ("api", "web"):
                head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=base / name, capture_output=True, text=True).stdout.strip()
                self.assertEqual(state["version_vector"][name]["revision"], head)
            [edge] = state["edges"]
            self.assertEqual((edge["provider"], edge["consumer"]), ("api", "web"))
            self.assertIn("web@" + state["version_vector"]["web"]["view_id"] + ":src/index.js",
                          [r["ref"] for r in state["consumer_evidence"][0]["occurrences"]])
            self.assertEqual([b["repository"] for b in state["boundaries"]], ["mobile"])


if __name__ == "__main__":
    unittest.main()
