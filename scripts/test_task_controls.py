import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("task_controls", Path(__file__).with_name("task-controls.py"))
controls = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(controls)


# A protocol fixture for exercising the Python driver and real Git clone. This
# does not execute, emulate or validate the Rust retrieval implementation.
FAKE_BINARY = r'''
import hashlib,json,sys
from pathlib import Path
args=sys.argv[1:]
root=Path(args[args.index('--project-root')+1])
db=Path(args[args.index('--db')+1])
position=args.index('--json')+1
command=args[position]
if command=='sync':
    files={str(p.relative_to(root)):p.read_text() for p in root.rglob('*.rs') if '.git' not in p.parts}
    db.parent.mkdir(parents=True,exist_ok=True)
    db.write_text(json.dumps({'files':files}))
files=json.loads(db.read_text())['files']
view='view:'+hashlib.sha256(str(root).encode()).hexdigest()
index='task-index:'+hashlib.sha256(str(db).encode()).hexdigest()
case='task-case:'+hashlib.sha256((str(root)+'case').encode()).hexdigest()
inventory={path:{'path':path,'artifact_id':'artifact:'+path,'digest':'sha256:'+hashlib.sha256(body.encode()).hexdigest(),
    'bytes':len(body.encode()),'role':'impl','language':'rust'} for path,body in files.items()}
roles=['impl','test','config','script','build','install','doc','adr','plan','fixture','generated']
profile={'language':'rust','semantic':False,'calls':True}
if command=='sync':
    result={'sync':'refreshed','view_id':view,'ready':False,'profile':profile,'static_calls':{'state':'partial'}}
elif command=='call':
    operation=args[position+1]
    arguments=json.loads(args[args.index('--args')+1])
    if operation=='status':
        result={'current_view':view,'ledger_watermark':1,'retracted_count':0,'pending_or_interrupted':False,
            'source_sync':{'view_id':view,'profile':profile},
            'inventory':{'history_commits_reachable':0,'session_publications':0}}
    elif operation=='explain': result={'entry':inventory}
    elif operation=='brief':
        selected=[{**row,'rank':i+1,'candidate_rank':i+1} for i,row in enumerate(inventory.values())]
        result={'id':case,'case_id':case,'view_id':view,'index_id':index,'outcome':'partial',
            'slice':{role:selected if role=='impl' else [] for role in roles},'not_covered':[],
            'candidates':[{'path':row['path'],'rank':row['rank'],'score':100,'selected':True,'role':'impl','signals':[]} for row in selected],
            'coverage':{'retrieval':{'minimum_score':24},'delivery':{section:{'omitted':0} for section in
                ['/not_covered','/candidates']+['/slice/'+role for role in roles]}}}
    elif operation=='agent_context':
        path=next(iter(inventory))
        result={'kind':'agent_context','case_id':case,'view_id':view,'files':[],
            'anchor':{'path':path,'ref':'/slice/impl/0'},
            'fetch_source':{'operation':'explain','arguments':{'report_id':case}},'continue_with':None}
    else: raise ValueError(operation)
else: raise ValueError(command)
print(json.dumps(result))
'''


class TaskControlsTests(unittest.TestCase):
    def test_manifest_order_is_stable_and_source_digest_changes_are_visible(self):
        entries = {"b": {"digest": "sha256:b", "bytes": 2}, "a": {"digest": "sha256:a", "bytes": 1}}
        original = controls.manifest(entries)["sha256"]
        self.assertEqual(original, controls.manifest(dict(reversed(list(entries.items()))))["sha256"])
        entries["a"]["digest"] = "sha256:changed"
        self.assertNotEqual(original, controls.manifest(entries)["sha256"])

    def test_copy_rejects_changed_bytes_traversal_and_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.rs"
            source.write_text("fn entry() {}")
            digest = controls.file_digest(source)
            self.assertEqual(controls.checked_source(root, "source.rs", digest), source)
            for path in ("../source.rs", "/outside.rs", ".git/config", "bad\\path.rs"):
                with self.assertRaisesRegex(ValueError, "Unsafe"):
                    controls.checked_source(root, path, digest)
            source.write_text("fn changed() {}")
            with self.assertRaisesRegex(ValueError, "differs"):
                controls.checked_source(root, "source.rs", digest)
            if hasattr(os, "symlink"):
                link = root / "link.rs"
                try:
                    link.symlink_to(source)
                except OSError:
                    return
                with self.assertRaisesRegex(ValueError, "symlink"):
                    controls.checked_source(root, "link.rs", controls.file_digest(source))

    @unittest.skipUnless(shutil.which("git"), "Git required")
    def test_clean_clone_includes_exact_retained_files_even_when_gitignore_matches(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root, output = base / "source", base / "output"
            root.mkdir(); output.mkdir()
            (root / ".gitignore").write_text("ignored.txt\n")
            (root / "ignored.txt").write_text("retained bytes\n")
            files = {path.name: {"digest": controls.file_digest(path)} for path in root.iterdir()}
            clone = controls.clean_clone(root, files, output)
            self.assertEqual((clone / "ignored.txt").read_text(), "retained bytes\n")
            result = subprocess.run(["git", "rev-list", "--all", "--count"], cwd=clone, capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout.strip(), "1")
            self.assertFalse((root / ".git").exists())

    def test_missing_binary_records_a_blocker_without_fabricating_measurements(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / "source"; root.mkdir()
            output = base / "results"
            with patch("sys.argv", ["task-controls.py", "--root", str(root), "--binary", str(base / "missing"),
                                    "--db", str(base / "missing.db"), "--output", str(output)]), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(controls.main(), 2)
            report = json.loads((output / "control-report.json").read_text())
            self.assertEqual(report["status"], "blocked")
            self.assertNotIn("comparisons", report)
            self.assertIn("Build", report["error"])

    @unittest.skipUnless(shutil.which("git") and os.name == "posix", "POSIX executable and Git required")
    def test_driver_runs_all_heldout_ablations_and_a_fresh_source_only_clone(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / "source"; (root / "src").mkdir(parents=True)
            body = "pub fn entry() {}\n"
            (root / "src/lib.rs").write_text(body)
            database = base / "baseline.db"; database.write_text(json.dumps({"files": {"src/lib.rs": body}}))
            binary = base / "fake-briefcase"
            binary.write_text("#!" + sys.executable + "\n" + FAKE_BINARY)
            binary.chmod(0o755)
            scenarios = base / "scenarios.json"
            scenarios.write_text(json.dumps({"scenarios": [{"id": "fixture", "task": "inspect entry", "required_paths": ["src/lib.rs"]}]}))
            output = base / "results"
            argv = ["task-controls.py", "--binary", str(binary), "--root", str(root), "--db", str(database),
                    "--original", str(scenarios), "--heldout", str(scenarios), "--output", str(output), "--sync-timeout", "10"]
            with patch("sys.argv", argv), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(controls.main(), 0, (output / "control-report.json").read_text())
            report = json.loads((output / "control-report.json").read_text())
            self.assertEqual(report["status"], "measured")
            self.assertEqual(report["retained"]["source_manifest_sha256"], report["clean"]["source_manifest_sha256"])
            self.assertFalse(report["clean"]["ready"], "partial native coverage must remain explicit")
            heldout = json.loads((output / "retained/heldout.json").read_text())
            self.assertEqual(len(heldout["facet_ablations"]), 10)
            self.assertEqual(heldout["inputs"]["source_view"], report["retained"]["view_id"])
            self.assertEqual(report["clean"]["session_publications"], 0)

    def test_comparison_rejects_changed_scenario_labels(self):
        normal = {"scenarios": [], "inputs": {"scenario_sha256": "one"}}
        clean = {"scenarios": [], "inputs": {"scenario_sha256": "two"}}
        with self.assertRaisesRegex(ValueError, "labels"):
            controls.compare(normal, clean)

    def test_profile_provenance_must_match_the_pinned_single_rust_capture(self):
        profile = {"language": "rust", "semantic": True, "calls": True}
        self.assertEqual(controls.source_profile({"source_sync": {"view_id": "view:a", "profile": profile}}, "view:a"), profile)
        for state in ({}, {"view_id": "view:b", "profile": profile},
                      {"view_id": "view:a", "profile": {"language": "typescript"}},
                      {"view_id": "view:a", "profile": {"profile_views": []}}):
            with self.assertRaisesRegex(ValueError, "profile"):
                controls.source_profile({"source_sync": state}, "view:a")


if __name__ == "__main__":
    unittest.main()
