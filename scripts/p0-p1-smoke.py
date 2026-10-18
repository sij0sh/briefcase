#!/usr/bin/env python3
"""Real-binary P0/P1 acceptance. Newly authored fixtures, never historical scores."""
import argparse
import json
from pathlib import Path
import sys

from run_evidence import RunEvidence, atomic_json, digest
from task_investigation import Investigation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary, output = args.binary.resolve(), args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Use a new output directory")
    output.mkdir(parents=True, exist_ok=True)
    run = RunEvidence(output / "raw")
    root, db = output / "repository", output / "evidence.db"
    root.mkdir()
    report = {"scope": "new P0/P1 protocol acceptance fixtures", "status": "blocked", "checks": []}
    try:
        fixtures = {"src/store.rs": "// p0p1token\npub fn save() {}\n",
                    "src/consumer.rs": "// p0p1token consumer\npub fn call(){ crate::store::save(); }\n",
                    "tests/store.rs": '#[path="../src/store.rs"] mod store;\n// p0p1token\n#[test] fn stores(){store::save();}\n',
                    "scripts/verify": "#!/bin/sh\n# p0p1token\ncat src/store.rs\n",
                    "config/demo.env": "P0P1_TOKEN=p0p1token\n",
                    "docs/long.md": "# p0p1token\n" + "Required Unicode evidence 雪é\n" * 500}
        for name, content in fixtures.items():
            path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text(content)
        atomic_json(output / "fixture-manifest.json", {name: digest(content.encode()) for name, content in fixtures.items()})
        initialized = run.run(["git", "init", "--quiet"], cwd=root)
        if initialized.returncode:
            raise RuntimeError(initialized.stderr)

        def call(operation, arguments):
            budget = arguments.get("max_bytes", 8000)
            result = run.run([str(binary), "--db", str(db), "--project-root", str(root), "--json", "call", operation,
                              "--args", json.dumps({**arguments, "max_bytes": budget})], cwd=root)
            if result.returncode:
                raise RuntimeError(result.stderr or result.stdout)
            if len(result.stdout.encode()) > budget:
                raise ValueError("Native response exceeds its advertised budget")
            return json.loads(result.stdout)

        workflow = Investigation(call, output / "investigation")
        workflow.start({"task": "refactor p0p1token safely", "target": "src/store.rs", "duty": "refactor", "max_files": 1})
        for step in range(1000):
            if workflow.step() is None:
                break
        else:
            raise ValueError("Investigation did not make bounded progress")
        assert len(workflow.state["sources"]) > 1, "No recovery beyond initial selection"
        assert all(s["state"] == "digest_verified" for s in workflow.state["sources"].values())
        assert workflow.state["context_readiness"] == "open", "Fetched bytes falsely established readiness"
        report["checks"].append({"id": "followups", "status": "passed", "source_files": len(workflow.state["sources"]), "actions": step})
        # Real budget reduction must leave a contiguous UTF-8 source stream.
        candidates = call("page", {"report_id": workflow.state["case_id"], "section": "/candidates"})
        long = next(row for row in candidates["items"] if row["path"] == "docs/long.md")
        offset, collected = 0, bytearray()
        while True:
            page = call("explain", {"report_id": workflow.state["case_id"], "ref": long["source_ref"],
                                    "source_offset": offset, "max_bytes": 2600})
            body = page.get("text", "").encode()
            if not body or page.get("source_offset") != offset:
                raise ValueError("Small-budget source fetch stalled")
            collected.extend(body)
            if page.get("next_source_offset") is None:
                break
            if page["next_source_offset"] != offset + len(body):
                raise ValueError("Source budget skipped bytes")
            offset = page["next_source_offset"]
        assert collected == fixtures["docs/long.md"].encode()
        report["checks"].append({"id": "source-budget", "status": "passed", "bytes": len(collected), "budget": 2600})
        (root / "src/consumer.rs").write_text("// modified out-of-slice consumer\n")
        (root / "src/new.rs").write_text("// untracked new caller\n")
        (root / "config/demo.env").unlink()
        assert not workflow.check_current()
        assert workflow.state["status"] == "invalidated"
        assert workflow.state["worktree"]["outside_initial_slice"] == 3
        report["checks"].append({"id": "out-of-slice-recheck", "status": "passed"})
        old_case = workflow.state["case_id"]
        published = run.run([str(binary), "--db", str(db), "--project-root", str(root), "--json", "capture-text", str(root)], cwd=root)
        if published.returncode:
            raise RuntimeError(published.stderr)
        rebound = Investigation(call, output / "rebound")
        rebound.start(workflow.state["request"])
        assert rebound.state["case_id"] != old_case and rebound.state["status"] != "invalidated"
        report["checks"].append({"id": "fresh-pin-rebind", "status": "passed"})
        report["status"] = "passed"
    except (OSError, RuntimeError, ValueError, KeyError, AssertionError) as error:
        report.update(status="failed", error=str(error))
    atomic_json(output / "acceptance.json", report)
    print(json.dumps(report, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
