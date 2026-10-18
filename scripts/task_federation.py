"""Thin two-or-more repository coordinator over each repository's own pins.

Every repository keeps its own root, database and view. The coordinator calls
the same local operations, keeps a version vector, and joins repositories only
through declared package dependencies or explicit contract bindings. Name
similarity is never an edge. An unavailable repository is a reported boundary.
"""
import hashlib
import json
import re
import tomllib

from run_evidence import atomic_json, digest

MANIFESTS = ("package.json", "Cargo.toml", "pyproject.toml", "go.mod")
DEPENDENCY_FIELDS = ("dependencies", "devDependencies", "peerDependencies", "optionalDependencies")


def qualified(repository, view, entry):
    """Identical relative paths in different repositories never collide."""
    return {"repository": repository, "view_id": view, "path": entry["path"],
            "artifact_id": entry.get("artifact_id"), "digest": entry.get("digest"),
            "ref": f"{repository}@{view}:{entry['path']}"}


def package_name(requirement):
    return re.split(r"[\s<>=!~;\[(]", requirement.strip(), maxsplit=1)[0].lower().replace("_", "-")


def manifest_facts(path, text):
    """(provided package names with versions, declared dependencies name -> spec)."""
    name = path.rsplit("/", 1)[-1]
    provides, requires = {}, {}
    if name == "package.json":
        data = json.loads(text)
        if data.get("name"):
            provides[data["name"]] = data.get("version")
        for field in DEPENDENCY_FIELDS:
            requires.update({k: str(v) for k, v in data.get(field, {}).items()})
    elif name == "Cargo.toml":
        data = tomllib.loads(text)
        if data.get("package", {}).get("name"):
            provides[data["package"]["name"]] = data["package"].get("version")
        for field in ("dependencies", "dev-dependencies", "build-dependencies"):
            for key, value in data.get(field, {}).items():
                requires[value.get("package", key) if isinstance(value, dict) else key] = json.dumps(value) if isinstance(value, dict) else str(value)
    elif name == "pyproject.toml":
        project = tomllib.loads(text).get("project", {})
        if project.get("name"):
            provides[package_name(project["name"])] = project.get("version")
        for requirement in project.get("dependencies", []):
            requires[package_name(requirement)] = requirement
    elif name == "go.mod":
        module = re.search(r"^module\s+(\S+)", text, re.M)
        if module:
            provides[module.group(1)] = None
        for block in re.findall(r"^require\s*\((.*?)^\)", text, re.M | re.S) + re.findall(r"^require\s+([^(\n].*)$", text, re.M):
            for line in block.splitlines():
                parts = line.split("//")[0].split()
                if len(parts) >= 2:
                    requires[parts[0]] = parts[1]
    return provides, requires


class Federation:
    def __init__(self, workspace, caller, directory, budget=24000):
        """caller(repository_entry) -> call(operation, arguments) bound to that root/database."""
        ids = [repo["id"] for repo in workspace.get("repositories", [])]
        if len(ids) < 2 or len(set(ids)) != len(ids):
            raise ValueError("A federation needs at least two uniquely identified repositories")
        self.workspace, self.caller, self.directory, self.budget = workspace, caller, directory, budget
        self.calls, self.events = {}, []
        self.state = {"schema_version": "1.0", "kind": "task_federation", "version_vector": {}, "edges": [],
                      "contracts": [], "consumer_evidence": [], "boundaries": [], "manifests": {}}

    def call(self, repository, operation, arguments):
        result = self.calls[repository](operation, {**arguments, "max_bytes": self.budget})
        self.events.append({"repository": repository, "operation": operation, "arguments": arguments})
        if not isinstance(result, dict):
            raise ValueError("An evidence operation must return an object")
        return result

    def boundary(self, repository, reason, inspection):
        self.state["boundaries"].append({"repository": repository, "reason": reason, "required_inspection": inspection})

    def pin(self, repo, task):
        repository = repo["id"]
        try:
            self.calls[repository] = self.caller(repo)
            case = self.call(repository, "agent_context", {"task": task, **({"target": repo["target"]} if repo.get("target") else {})})
            status = self.call(repository, "status", {"summary": True})
        except (OSError, RuntimeError, ValueError) as error:
            self.calls.pop(repository, None)
            self.boundary(repository, "Repository is unavailable: " + str(error)[:400],
                          "Capture this repository locally and rerun; its consumers and contracts are not covered.")
            return False
        if case.get("view_id") != status.get("current_view") or status.get("pending_or_interrupted"):
            raise ValueError(f"{repository}: capture changed while pinning; rerun after capture settles")
        tree = self.call(repository, "worktree", {"case_id": case["case_id"], "limit": 1})
        self.state["version_vector"][repository] = {"view_id": case["view_id"], "case_id": case["case_id"],
            "index_id": case["index_id"], "revision": tree.get("coverage", {}).get("inventory", {}).get("capture_head"),
            "tree_digest": tree.get("tree_digest"), "alignment": tree.get("alignment"), "root": str(repo.get("root"))}
        if tree.get("alignment") != "matches":
            self.boundary(repository, "Live source differs from the pinned capture",
                          "Sync this repository and rerun before treating its evidence as current.")
        return True

    def read(self, repository, path):
        """Whole pinned file through the index, digest-verified, or None if absent."""
        pin = self.state["version_vector"][repository]
        ref = "/files/" + path.replace("~", "~0").replace("/", "~1")
        chunks, offset = [], 0
        while True:
            try:
                page = self.call(repository, "explain", {"report_id": pin["index_id"], "ref": ref, "source_offset": offset})
            except RuntimeError as error:
                if "Unknown report reference" in str(error) and offset == 0:
                    return None
                raise
            if page.get("view_id") not in (None, pin["view_id"]) or page.get("source_offset") != offset:
                raise ValueError(f"{repository}:{path}: source page left the pin or skipped bytes")
            body = page.get("text", "").encode("utf-8")
            chunks.append(body)
            if page.get("next_source_offset") is None:
                break
            if not body or page["next_source_offset"] != offset + len(body):
                raise ValueError(f"{repository}:{path}: non-contiguous source pages")
            offset = page["next_source_offset"]
        raw = b"".join(chunks)
        if digest(raw) != page["entry"]["digest"]:
            raise ValueError(f"{repository}:{path}: delivered bytes do not match the retained digest")
        return raw.decode("utf-8"), qualified(repository, pin["view_id"], page["entry"])

    def occurrences(self, repository, text, limit=1000):
        pin, rows, offset = self.state["version_vector"][repository], [], 0
        while offset is not None and len(rows) < limit:
            page = self.call(repository, "occurrences", {"view": pin["view_id"], "text": text, "offset": offset,
                                                          "limit": min(200, limit - len(rows))})
            if page["page"]["offset"] != offset:
                raise ValueError(f"{repository}: occurrence page resumed at another offset")
            rows.extend({**qualified(repository, pin["view_id"], row), "line": row.get("line")} for row in page["items"])
            offset = page["page"].get("next_offset")
        return rows, offset

    def run(self, task):
        repositories = self.workspace["repositories"]
        pinned = [repo["id"] for repo in repositories if self.pin(repo, task)]
        provides, requires = {}, {}
        for repository in pinned:
            for path in self.workspace.get("manifests", {}).get(repository, MANIFESTS):
                found = self.read(repository, path)
                if found is None:
                    continue
                text, ref = found
                offered, needed = manifest_facts(path, text)
                self.state["manifests"].setdefault(repository, []).append({**ref, "provides": offered, "requires": needed})
                provides.update({name: (repository, version, ref) for name, version in offered.items()})
                requires.setdefault(repository, []).extend((name, spec, ref) for name, spec in needed.items())
        for consumer, needs in sorted(requires.items()):
            for name, spec, ref in needs:
                if name in provides and provides[name][0] != consumer:
                    provider, version, provider_ref = provides[name]
                    self.state["edges"].append({"kind": "declared_dependency", "package": name, "provider": provider,
                        "consumer": consumer, "declared_requirement": spec, "provider_version": version,
                        "evidence": [provider_ref, ref]})
        for contract in self.workspace.get("contracts", []):
            # Explicit service/API bindings: provider schema bytes plus the terms consumers must use.
            if contract["provider"] not in pinned:
                self.boundary(contract["provider"], "Contract provider is not pinned", "Capture the provider before relying on " + contract["path"])
                continue
            found = self.read(contract["provider"], contract["path"])
            if found is None:
                raise ValueError(f"Declared contract {contract['provider']}:{contract['path']} is absent from its pin")
            self.state["contracts"].append({**found[1], "terms": contract.get("terms", []), "consumers": contract.get("consumers", [])})
            for consumer in contract.get("consumers", []):
                self.state["edges"].append({"kind": "explicit_contract", "package": contract["path"], "provider": contract["provider"],
                                            "consumer": consumer, "evidence": [found[1]]})
        for edge in self.state["edges"]:
            consumer = edge["consumer"]
            if consumer not in pinned:
                self.boundary(consumer, f"Declared consumer of {edge['provider']} is unavailable",
                              f"Inspect {consumer} for uses of {edge['package']} before claiming impact coverage.")
                continue
            terms = [edge["package"]] if edge["kind"] == "declared_dependency" else next(
                c["terms"] for c in self.state["contracts"] if c["path"] == edge["package"] and c["repository"] == edge["provider"])
            for term in terms:
                rows, remaining = self.occurrences(consumer, term)
                self.state["consumer_evidence"].append({"provider": edge["provider"], "consumer": consumer, "term": term,
                    "occurrences": rows, "complete": remaining is None,
                    "continue_with": None if remaining is None else {"repository": consumer, "operation": "occurrences",
                        "arguments": {"view": self.state["version_vector"][consumer]["view_id"], "text": term, "offset": remaining}}})
        for repo in repositories:
            if repo["id"] not in pinned and not any(b["repository"] == repo["id"] for b in self.state["boundaries"]):
                self.boundary(repo["id"], "Unavailable", "Capture and rerun")
        # Pins are rechecked: a changed view invalidates every join above.
        for repository in pinned:
            status = self.call(repository, "status", {"summary": True})
            if status.get("current_view") != self.state["version_vector"][repository]["view_id"]:
                raise ValueError(f"{repository}: source view changed during federation; rerun")
        self.state["compatibility"] = {"state": "declared_only", "declared_set": self.workspace.get("compatibility_set"),
            "interpretation": "A vector of snapshots is not a coordinated release; declared requirements are not verified compatibility."}
        self.state["coverage"] = "partial" if self.state["boundaries"] else "all_declared_repositories_pinned"
        self.state["vector_digest"] = digest(json.dumps(self.state["version_vector"], sort_keys=True).encode())
        self.save()
        return self.state

    def save(self):
        atomic_json(self.directory / "federation.json", self.state)
        atomic_json(self.directory / "events.json", self.events)
