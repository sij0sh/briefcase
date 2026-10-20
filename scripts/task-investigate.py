#!/usr/bin/env python3
"""Ask a task question, resume evidence retrieval, and review its requirements.

Example: python3 scripts/task-investigate.py --task 'rename save safely' \
  --target src/store.rs --root . --state-dir .briefcase/investigations/rename --steps 20
Use the same state directory to resume. --read-event delivers earlier evidence;
--query adds a pinned occurrence/neighbor/search request; --assessment records
the harness's cited judgments. Neither fetching bytes nor empty queues prove
completeness. After syncing changed source use --rebind to start fresh pins.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from run_evidence import RunEvidence, atomic_json
from task_investigation import Investigation, MODES, key


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/briefcase"))
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--db", type=Path)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--task")
    parser.add_argument("--target")
    parser.add_argument("--duty")
    parser.add_argument("--initial-files", type=int, help="Initial case selection only; follow-ups remain available")
    parser.add_argument("--steps", type=int, default=1, help="Actions in this invocation; remaining work is resumable")
    parser.add_argument("--max-bytes", type=int, default=8000, help="Entire printed response budget")
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--query", type=Path, help="JSON object/list of pinned occurrences, neighbors or search requests")
    parser.add_argument("--assessment", type=Path, help="JSON requirements with status, reason and delivered evidence event IDs")
    parser.add_argument("--read-event", help="Deliver one retained response by event ID")
    parser.add_argument("--rebind", action="store_true", help="Rebuild the task at latest after explicit source sync")
    parser.add_argument("--compact-inventory", action="store_true", help="Use compact pinned candidate inventories (requires Priority 1 engine)")
    parser.add_argument("--inspection-only", action="store_true", help="Read pinned excerpts without automatically requesting whole-file verification")
    parser.add_argument("--verify-source", action="append", default=[], help="Explicitly request full-file digest verification for an already queued source path")
    parser.add_argument("--mode", choices=tuple(mode.replace("_", "-") for mode in MODES), default="manifest-first",
                        help="manifest-first preserves source-first behavior; inventory-first enumerates before excerpts; "
                             "excerpt-first interleaves inventories and localized excerpts before full-file verification")
    args = parser.parse_args()
    if args.steps < 0 or not 2048 <= args.max_bytes <= 1048576 or (args.initial_files is not None and not 1 <= args.initial_files <= 1000):
        parser.error("steps must be nonnegative; max-bytes must be 2048..1048576")
    root, binary, directory = args.root.resolve(), args.binary.resolve(), args.state_dir.resolve()
    db = args.db.resolve() if args.db else root / ".briefcase/briefcase.db"
    directory.mkdir(parents=True, exist_ok=True)
    state_path = directory / "state.json"
    state = json.loads(state_path.read_text()) if state_path.is_file() else None
    binding = {"root": str(root), "db": str(db), "binary": str(binary)}
    if state and state.get("binding") != binding:
        parser.error("State is bound to another root/database/binary path")
    # Serialize use of one state directory; parallel harnesses use separate records.
    lock = directory / "writer.lock"
    try:
        handle = lock.open("x")
    except FileExistsError:
        parser.error("Investigation is already in use; recover a stale writer.lock only after its process exits")
    handle.close()
    workflow = None
    try:
        trace_root = directory / "rpc"
        trace_root.mkdir(exist_ok=True)
        run = RunEvidence(trace_root / f"{len(list(trace_root.iterdir())):06d}")

        def call(operation, arguments):
            # Reserve room for the wrapper/event ID in the actual consumer output.
            arguments = {**arguments, "max_bytes": min(arguments.get("max_bytes", args.max_bytes), args.max_bytes - 512)}
            command = [str(binary), "--db", str(db), "--project-root", str(root), "--json", "call", operation,
                       "--args", json.dumps(arguments, ensure_ascii=False)]
            result = run.run(command, cwd=root, timeout=args.timeout)
            if result.returncode:
                raise RuntimeError((result.stderr or result.stdout).strip()[:2000])
            value = json.loads(result.stdout)
            if len(result.stdout.encode()) > arguments["max_bytes"]:
                raise ValueError("CLI response exceeded the real consumer budget")
            return value

        if args.rebind:
            if not state:
                raise ValueError("Rebind requires an existing investigation")
            parent = key(state)
            request = {k: v for k, v in state["request"].items() if k != "view"}
            state = None
        else:
            parent, request = None, {k: v for k, v in {"task": args.task, "target": args.target, "duty": args.duty,
                                                     "max_files": args.initial_files}.items() if v is not None}
        workflow = Investigation(call, directory, state)
        if state is None:
            if not request.get("task"):
                raise ValueError("A new investigation requires --task")
            workflow.start(request, args.max_bytes, binding, mode=args.mode,
                           compact_inventory=args.compact_inventory, verify_sources=not args.inspection_only)
            if parent:
                workflow.state["previous_revision"] = parent
        else:
            if request and any(state["request"].get(k) != v for k, v in request.items()):
                raise ValueError("Task changed; use a new investigation directory")
            workflow.state["budget"] = args.max_bytes
            workflow.check_current()
        if args.query:
            queries = json.loads(args.query.read_text())
            for query in queries if isinstance(queries, list) else [queries]:
                workflow.add_query(query)
        for path in args.verify_source:
            workflow.verify_source(path)
        if args.read_event:
            result = {"event_id": args.read_event, "response": workflow.read_event(args.read_event)}
        else:
            display_event = None
            if workflow.state["status"] != "invalidated":
                for _ in range(args.steps):
                    event_id = workflow.step()
                    if event_id is None:
                        break
                    display_event = event_id
                workflow.check_current()
            if args.assessment:
                workflow.assess(json.loads(args.assessment.read_text()))
            workflow.save()
            result = {"state_file": str(state_path), "case_id": workflow.state["case_id"],
                      "view_id": workflow.state["view_id"], "context_readiness": workflow.state["context_readiness"],
                      "verification": workflow.state["verification"], "pending_actions": len(workflow.state["queue"]),
                      "source_files_verified": sum(s["state"] == "digest_verified" for s in workflow.state["sources"].values()),
                      "blocker": workflow.state.get("blocker"), "worktree": workflow.state.get("worktree"),
                      "next_action": workflow.next_action() if workflow.state["queue"] else None,
                      "source_files_inspected": sum(bool(s["ranges"] or s.get("excerpts")) for s in workflow.state["sources"].values()),
                      "case_revision_count": len(workflow.state.get("case_revisions", [])),
                      "event_count": len(workflow.state["events"]),
                      "instruction": "Read state_file for requirements and event references. --read-event <id> delivers evidence; --assessment records cited harness judgments. Resume with this state directory. Fetched source is not proof of task completeness."}
            if display_event:
                last = next(e for e in workflow.delivered if e["id"] == display_event)
                observation = workflow.read_event(last["id"])
                trial = {**result, "event_id": last["id"], "response": observation}
                if len((json.dumps(trial, ensure_ascii=False, separators=(",", ":")) + "\n").encode()) <= args.max_bytes:
                    result = trial
                else:
                    # Keep collection separate from delivery when the wrapper does not fit.
                    next(e for e in workflow.state["events"] if e["id"] == display_event)["delivered_to_harness"] = False
                    result["event"] = last
        encoded = json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n"
        if len(encoded.encode()) > args.max_bytes:
            raise ValueError("Consumer envelope does not fit; increase --max-bytes to read this event")
        workflow.save()
        print(encoded, end="")
        return 2 if workflow.state["status"] == "invalidated" else 0
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        # Failed actions remain pending in the last committed revision. Raw RPC
        # evidence survives, including timeouts and malformed responses.
        print(json.dumps({"status": "blocked", "error": str(error), "state_file": str(state_path)}), file=sys.stderr)
        return 2
    finally:
        lock.unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())
