"""Reference harness workflow over Briefcase's existing pinned read protocol.

The record contains references and progress. Raw responses remain in the run's
event log. Reading bytes never automatically supports a task-level conclusion.
"""
import hashlib
import json
from pathlib import Path

from run_evidence import atomic_json, digest


def key(value):
    return digest(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


MODES = ("manifest_first", "exhaustive")


def normalize_mode(mode):
    value = (mode or "manifest-first").replace("-", "_")
    if value not in MODES:
        raise ValueError(f"Unknown investigation mode: {mode!r} (expected manifest-first or exhaustive)")
    return value


def manifest_source_ref(manifest, row):
    """Resolve one manifest row's pinned source ref (v2 shared or v1 per-row)."""
    if isinstance(row.get("fetch"), dict):
        ref = row["fetch"].get("arguments", {}).get("ref")
        if isinstance(ref, str):
            return ref
    if isinstance(row.get("ref"), str):
        return row["ref"]
    return None


class Investigation:
    def __init__(self, call, directory, state=None):
        self.call, self.directory = call, Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.state = state or {}
        self.delivered = []

    def event(self, operation, arguments):
        result = self.call(operation, arguments)
        if not isinstance(result, dict):
            raise ValueError("An evidence operation must return an object")
        sequence = len(self.state.setdefault("events", []))
        event = {"sequence": sequence, "operation": operation, "arguments": arguments, "response": result}
        event_id = key(event)
        name = f"events/{sequence:06d}-{event_id.removeprefix('sha256:')}.json"
        atomic_json(self.directory / name, event)
        self.state["events"].append({"id": event_id, "file": name, "operation": operation, "delivered_to_harness": False})
        self.delivered.append({"id": event_id, "file": str(self.directory / name)})
        return result, event_id

    def enqueue(self, operation, arguments, kind="evidence", path=None, front=False):
        arguments = dict(arguments)
        # A budget is transport state, not the identity of a retrieval action.
        arguments.pop("max_bytes", None)
        action = {"operation": operation, "arguments": arguments, "kind": kind}
        if path is not None:
            action["path"] = path
        action_id = key(action)
        if action_id not in self.state.setdefault("scheduled", []):
            self.state["scheduled"].append(action_id)
            queue = self.state.setdefault("queue", [])
            # A started file finishes before the next one starts, so rank order
            # decides which sources are complete when the budget runs out.
            queue.insert(1 if front and queue else len(queue), {"id": action_id, **action})

    def source(self, path, report, reference):
        if path in self.state.setdefault("sources", {}):
            return
        self.state["sources"][path] = {"report_id": report, "ref": reference, "ranges": [], "state": "queued"}
        self.enqueue("explain", {"report_id": report, "ref": reference, "source_offset": 0}, "source", path)

    def start(self, arguments, budget=8000, binding=None, mode=None):
        if self.state:
            raise ValueError("Start requires a new investigation directory")
        mode = normalize_mode(mode)
        self.state = {"schema_version": "1.0", "kind": "task_investigation", "request": dict(arguments),
                      "budget": budget, "events": [], "queue": [], "scheduled": [], "sources": {},
                      "requirements": {}, "gaps": [], "status": "open", "binding": binding, "mode": mode}
        manifest, _ = self.event("agent_context", {**arguments, "max_bytes": budget})
        for field, prefix in (("case_id", "task-case:"), ("view_id", "view:"), ("index_id", "task-index:")):
            if not isinstance(manifest.get(field), str) or not manifest[field].startswith(prefix):
                raise ValueError(f"Initial context is missing a pinned {field}")
            self.state[field] = manifest[field]
        self.state["verification"] = {"state": "pending" if manifest.get("coverage", {}).get("completeness", {}).get("verification") == "unknown" else "not_required",
                                      "interpretation": "Test execution is separate from context readiness; no tests are run by this workflow."}
        for name, description in (("implementation", "Explain the intended behavior and relevant source constraints"),
                                  ("consumers", "Account for affected interfaces, callers and consumers"),
                                  ("gaps", "Resolve or justify task-local remaining evidence boundaries")):
            self.state["requirements"][name] = {"status": "open", "reason": description, "evidence": []}
        report = self.state["case_id"]
        anchor = manifest.get("anchor") or {}
        rows = ([anchor] if anchor.get("path") else []) + list(manifest.get("files") or [])
        self.state["manifest"] = {"anchor_path": anchor.get("path"),
                                   "paths": [row["path"] for row in rows if isinstance(row, dict) and row.get("path")]}
        if mode == "manifest_first":
            # Selected evidence first: anchor, then supplemental files in
            # manifest order. Candidate/gap traversal is enqueued after.
            for row in rows:
                if not isinstance(row, dict) or not row.get("path"):
                    continue
                ref = manifest_source_ref(manifest, row)
                if ref is not None:
                    self.source(row["path"], report, ref)
        for ref, kind in (("/coverage/artifact_roles", "roles"), ("/coverage/required_roles", "required_roles")):
            self.enqueue("explain", {"report_id": report, "ref": ref}, kind)
        self.enqueue("page", {"report_id": report, "section": "/not_covered", "offset": 0, "limit": 20}, "gaps")
        self.enqueue("page", {"report_id": report, "section": "/candidates", "offset": 0, "limit": 20}, "candidates")
        target = arguments.get("target")
        if target and target not in (".", "./", "/"):
            self.enqueue("neighbors", {"index_id": self.state["index_id"], "view": self.state["view_id"],
                                       "target": target, "direction": "both", "depth": 1, "limit": 20}, "neighbors")
        self.check_current()
        self.save()
        return self.state

    def check_current(self):
        # Control-plane reads must fit their pin fields even when the
        # consumer/manifest budget is the 8 KB routing budget.
        control_bytes = max(self.state["budget"], 65536)
        status, _ = self.event("status", {"summary": True, "max_bytes": control_bytes})
        # A budget notice that omits the pin fields leaves freshness unknown, not changed.
        if not isinstance(status.get("current_view"), str) or not isinstance(status.get("pending_or_interrupted"), bool):
            raise ValueError("Status omitted current_view/pending state; increase --max-bytes and resume")
        if status.get("current_view") != self.state["view_id"] or status.get("pending_or_interrupted"):
            self.state["status"] = "invalidated"
            self.state["blocker"] = "Source pin changed or capture is pending. Finish capture and rebind the investigation."
            return False
        if self.state.get("retracted_count") not in (None, status.get("retracted_count")):
            self.state["status"] = "invalidated"
            self.state["blocker"] = "Retractions changed. Rebind before continuing."
            return False
        self.state["retracted_count"] = status.get("retracted_count")
        current, event_id = self.event("worktree", {"case_id": self.state["case_id"], "limit": 1,
                                                   "max_bytes": control_bytes})
        self.state["worktree"] = {k: current.get(k) for k in ("report_id", "tree_digest", "alignment", "head_alignment", "changed_paths", "outside_initial_slice")}
        previous = self.state.get("tree_digest")
        if current.get("alignment") == "changed" or not current.get("tree_digest") or previous not in (None, current.get("tree_digest")):
            self.state["status"] = "invalidated"
            self.state["blocker"] = "Live source differs or its coverage is unknown. Inspect the worktree report, sync current source, and rebind."
            self.state["requirements"]["scope"] = {"status": "blocked", "reason": self.state["blocker"], "evidence": [event_id]}
            return False
        self.state["tree_digest"] = current["tree_digest"]
        if current.get("alignment") == "matches":
            self.state["requirements"]["scope"] = {"status": "supported", "reason": "All capturable source bytes match the pinned view", "evidence": [event_id]}
        else:
            # Unknown alignment (e.g. no Git in a frozen export) is not a
            # change: traversal may continue, but scope stays unproven.
            self.state["requirements"]["scope"] = {"status": "open", "reason": f"Worktree alignment is {current.get('alignment')}; live-tree equality is not established", "evidence": [event_id]}
        return True

    def validate_pin(self, result):
        if result.get("historical_packet") or result.get("historical_index"):
            raise ValueError("Evidence was retracted or its index is historical; rebind")
        if result.get("view_id") not in (None, self.state["view_id"]):
            raise ValueError("Evidence belongs to another source view")
        if result.get("index_id") not in (None, self.state["index_id"]):
            raise ValueError("Evidence belongs to another relationship index")
        if result.get("freshness", {}).get("pending_or_interrupted"):
            raise ValueError("Source capture changed during the investigation")

    def step(self):
        if self.state.get("status") == "invalidated":
            raise ValueError(self.state["blocker"])
        if not self.state["queue"]:
            return None
        action = self.state["queue"][0]
        args = {**action["arguments"], "max_bytes": self.state["budget"]}
        if action["kind"] == "source":
            args["source_bytes"] = min(4000, max(4, self.state["budget"] // 2))
        else:
            # Inventory pages must fit at least one row; the manifest budget
            # alone can stall T3-scale gap/candidate pages at zero rows.
            args["max_bytes"] = max(self.state["budget"], 65536)
        result, event_id = self.event(action["operation"], args)
        self.validate_pin(result)
        if args.get("report_id") and result.get("report_id") != args["report_id"]:
            raise ValueError("Evidence belongs to another report")
        if result.get("truncated") and "items" not in result and "text" not in result:
            raise ValueError("No evidence fits this budget; increase --max-bytes and resume the same action")
        kind = action["kind"]
        if kind == "source":
            self.consume_source(action, result, event_id)
        elif kind in ("roles", "required_roles"):
            if "entry" not in result:
                raise ValueError("Task policy metadata was omitted by the budget")
            self.state[kind] = result["entry"]
            if "roles" in self.state and "required_roles" in self.state:
                for role in self.state["required_roles"]:
                    captured = self.state["roles"].get(role, {}).get("captured_files")
                    self.state["requirements"].setdefault("role:" + role, {
                        "status": "not_applicable" if captured == 0 else "open",
                        "reason": "No files of this role in the matched capture" if captured == 0 else "Inspect task-relevant " + role + " evidence",
                        "evidence": [event_id] if captured == 0 else []})
        else:
            items = result.get("items")
            if not isinstance(items, list) or not isinstance(result.get("page"), dict):
                raise ValueError("Evidence page is missing rows or pagination metadata")
            page = result["page"]
            expected_offset = action["arguments"].get("offset", 0)
            if action["arguments"].get("cursor"):
                expected_offset = json.loads(action["arguments"]["cursor"])["offset"]
            if type(page.get("offset")) is not int or page["offset"] != expected_offset:
                raise ValueError("Page resumed at a different offset")
            if page.get("returned") != len(items):
                raise ValueError("Page counts do not match delivered rows")
            next_offset = page.get("next_offset")
            if next_offset is not None:
                if type(next_offset) is not int or next_offset != page["offset"] + len(items) or not items:
                    raise ValueError("Non-contiguous or stalled continuation")
                continuation = result.get("continue_with")
                if not isinstance(continuation, dict):
                    raise ValueError("Missing executable continuation")
                self.enqueue(continuation["operation"], continuation["arguments"], kind)
            if kind == "gaps":
                self.state["gaps"].extend({"event": event_id, "area": row.get("area"), "dimension": row.get("dimension"),
                                           "actionable": row.get("actionable", True)} for row in items)
            if kind == "candidates":
                for index, row in enumerate(items, start=page["offset"]):
                    self.source(row["path"], self.state["case_id"], row.get("source_ref", f"/candidates/{index}"))
            if kind in ("neighbors", "occurrences", "search"):
                paths = set()
                for row in items:
                    paths.update(row.get("from_files", [])); paths.update(row.get("to_files", []))
                    if row.get("path") and row.get("source_kind") not in ("session", "commit"):
                        paths.add(row["path"])
                for path in sorted(paths):
                    escaped = path.replace("~", "~0").replace("/", "~1")
                    self.source(path, self.state["index_id"], "/files/" + escaped)
        self.state["queue"].pop(0)
        self.state["status"] = "open"
        self.save()
        return event_id

    def consume_source(self, action, result, event_id):
        path, expected = action["path"], action["arguments"].get("source_offset", 0)
        source = self.state["sources"][path]
        entry, text = result.get("entry", {}), result.get("text")
        if (entry.get("path") != path or not isinstance(text, str)
                or result.get("report_id") != action["arguments"]["report_id"]
                or result.get("ref") != action["arguments"]["ref"]
                or type(result.get("source_offset")) is not int or result["source_offset"] != expected
                or type(entry.get("bytes")) is not int or not isinstance(entry.get("digest"), str)
                or not isinstance(entry.get("artifact_id"), str)):
            raise ValueError("Unpinned, omitted or malformed source page")
        identity = {k: entry.get(k) for k in ("artifact_id", "digest", "bytes")}
        if "identity" in source and source["identity"] != identity:
            raise ValueError("Source identity changed between pages")
        source["identity"] = identity
        end = expected + len(text.encode("utf-8"))
        next_offset = result.get("next_source_offset")
        if (end > entry["bytes"] or result.get("omitted_bytes") != entry["bytes"] - end
                or (end < entry["bytes"] and (not text or type(next_offset) is not int or next_offset != end))
                or (end == entry["bytes"] and next_offset is not None)):
            raise ValueError("Non-contiguous or stalled source bytes")
        source["ranges"].append({"start": expected, "end": end, "event": event_id})
        if next_offset is not None:
            self.enqueue("explain", {**action["arguments"], "source_offset": next_offset}, "source", path, front=True)
            source["state"] = "partial"
        else:
            hasher = hashlib.sha256()
            offset = 0
            by_id = {event["id"]: event for event in self.state["events"]}
            for span in source["ranges"]:
                if span["start"] != offset:
                    raise ValueError("Stored source ranges are not contiguous")
                event = json.loads((self.directory / by_id[span["event"]]["file"]).read_text())
                if key(event) != span["event"]:
                    raise ValueError("Stored source event was modified")
                hasher.update(event["response"]["text"].encode("utf-8")); offset = span["end"]
            if "sha256:" + hasher.hexdigest() != entry["digest"]:
                raise ValueError("Delivered source digest does not match retained bytes")
            source["state"] = "digest_verified"

    def add_query(self, query):
        operation, args = query["operation"], dict(query.get("arguments", {}))
        if operation not in ("occurrences", "neighbors", "search"):
            raise ValueError("Additional investigations use occurrences, neighbors or search")
        if args.get("view", self.state["view_id"]) != self.state["view_id"]:
            raise ValueError("Query differs from the investigation source pin")
        args["view"] = self.state["view_id"]
        if operation == "neighbors":
            if args.get("index_id", self.state["index_id"]) != self.state["index_id"]:
                raise ValueError("Query differs from the investigation index")
            args["index_id"] = self.state["index_id"]
        self.enqueue(operation, args, operation)

    def assess(self, assessment):
        known = {event["id"] for event in self.state["events"] if event.get("delivered_to_harness")}
        for name, claim in assessment.get("requirements", {}).items():
            if name == "scope" or name not in self.state["requirements"]:
                raise ValueError("Unknown or automatically controlled requirement: " + name)
            status, reason, evidence = claim.get("status"), claim.get("reason", "").strip(), claim.get("evidence", [])
            if status not in ("supported", "open", "blocked", "not_applicable") or not reason:
                raise ValueError("Every assessment needs a valid status and reason")
            if not isinstance(evidence, list) or any(ref not in known for ref in evidence) or (status == "supported" and not evidence):
                raise ValueError("Supported claims need evidence from this investigation")
            self.state["requirements"][name] = {"status": status, "reason": reason, "evidence": evidence, "authority": "harness_assessment"}

    def read_event(self, event_id):
        event = next((e for e in self.state["events"] if e["id"] == event_id), None)
        if event is None:
            raise ValueError("Unknown event in this investigation")
        value = json.loads((self.directory / event["file"]).read_text())
        if key(value) != event_id:
            raise ValueError("Stored evidence was modified")
        event["delivered_to_harness"] = True
        return value["response"]

    def save(self):
        state = self.state
        state["retrieval"] = "pending" if state["queue"] else "enumerated_initial_scope"
        ready = not state["queue"] and all(r["status"] in ("supported", "not_applicable") for r in state["requirements"].values())
        state["context_readiness"] = "blocked" if state["status"] == "invalidated" else "ready" if ready else "open"
        state["readiness_scope"] = "Explicit task requirements within the pinned retained corpus; harness assessments are not proof of universal completeness"
        revision = key(state)
        atomic_json(self.directory / "revisions" / (revision.removeprefix("sha256:") + ".json"), state)
        atomic_json(self.directory / "state.json", state)
        return revision


def run_bounded_investigation(call, directory, scenario, *, mode="manifest-first",
                              max_calls=300, max_bytes=2 * 1024 * 1024, max_seconds=180, budget=8000):
    """Run one investigation under aggregate call/byte/time stopping limits.

    ``limit_reached`` names the first exhausted budget, or ``complete`` when
    the queue drained. Offline source-integrity verification is not part of
    this cost; it is measured separately by the evaluator.
    """
    import time as _time
    stats = {"calls": 0, "bytes": 0}

    def counted(operation, arguments):
        result = call(operation, arguments)
        stats["calls"] += 1
        stats["bytes"] += len((json.dumps(result, ensure_ascii=False) + "\n").encode("utf-8"))
        return result

    started = _time.monotonic()
    flow = Investigation(counted, directory)
    request = {"task": scenario["task"]}
    for name in ("target", "duty", "view"):
        if name in scenario:
            request[name] = scenario[name]
    flow.start(request, budget, mode=mode)
    limit = "complete"
    while flow.state["queue"]:
        if stats["calls"] >= max_calls:
            limit = "calls"; break
        if stats["bytes"] >= max_bytes:
            limit = "bytes"; break
        if _time.monotonic() - started >= max_seconds:
            limit = "seconds"; break
        flow.step()
    else:
        limit = "complete"
    if limit == "complete" and flow.state["queue"]:
        limit = "calls" if stats["calls"] >= max_calls else "bytes" if stats["bytes"] >= max_bytes else "seconds"
    inspected = {path for path, src in flow.state.get("sources", {}).items() if src.get("ranges")}
    expected = set(scenario.get("required_paths", []))
    manifest_paths = set(flow.state.get("manifest", {}).get("paths", []))
    return {"mode": normalize_mode(mode), "limit_reached": limit, "calls": stats["calls"],
            "bytes": stats["bytes"], "elapsed_seconds": round(_time.monotonic() - started, 3),
            "inspected_files": len(inspected),
            "manifest_required_paths": sorted(expected & manifest_paths),
            "recovered_required_paths": sorted(expected & inspected),
            "missing_required_paths": sorted(expected - inspected),
            "recovered_beyond_manifest": sorted((expected & inspected) - manifest_paths)}
