#!/usr/bin/env python3
"""Join contract and consumer evidence across explicitly listed local repositories.

Workspace JSON: {"repositories":[{"id":"api","root":"../api","db":"../api/.briefcase/briefcase.db"},
{"id":"web","root":"."}], "contracts":[{"provider":"api","path":"schema/api.json","terms":["getUser"],
"consumers":["web"]}], "compatibility_set": optional declared release set}.
Relative paths resolve against the workspace file. Each repository keeps its own
pin; the record holds the version vector, qualified references and boundaries.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from run_evidence import RunEvidence
from task_federation import Federation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--task", required=True)
    parser.add_argument("--binary", type=Path, default=Path("target/release/briefcase"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--max-bytes", type=int, default=24000)
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    workspace_path, binary, output = args.workspace.resolve(), args.binary.resolve(), args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Use a new output directory")
    output.mkdir(parents=True, exist_ok=True)
    workspace = json.loads(workspace_path.read_text())
    run = RunEvidence(output / "raw")

    def caller(repo):
        root = (workspace_path.parent / repo["root"]).resolve()
        if not root.is_dir():
            raise OSError(f"root {root} does not exist")
        db = (workspace_path.parent / repo["db"]).resolve() if repo.get("db") else root / ".briefcase/briefcase.db"

        def call(operation, arguments):
            command = [str(binary), "--db", str(db), "--project-root", str(root), "--json", "call", operation,
                       "--args", json.dumps(arguments, ensure_ascii=False)]
            result = run.run(command, cwd=root, timeout=args.timeout)
            if result.returncode:
                raise RuntimeError((result.stderr or result.stdout).strip()[:2000])
            return json.loads(result.stdout)
        return call

    try:
        state = Federation(workspace, caller, output, args.max_bytes).run(args.task)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"status": "blocked", "error": str(error)}), file=sys.stderr)
        return 2
    print(json.dumps({"record": str(output / "federation.json"), "coverage": state["coverage"],
                      "version_vector": {k: v["view_id"] for k, v in state["version_vector"].items()},
                      "edges": len(state["edges"]), "boundaries": state["boundaries"],
                      "consumer_occurrences": sum(len(e["occurrences"]) for e in state["consumer_evidence"])}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
