"""Reproducible implementation identities and exact subprocess evidence."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import time


def digest(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def implementation(root):
    root = Path(root).resolve()
    files = [root / name for name in ("Cargo.toml", "Cargo.lock")]
    for name in ("src", "extensions", "contracts", "scripts", "tools", "tests", "examples"):
        files.extend(p for p in (root / name).rglob("*") if p.is_file()
                     and "__pycache__" not in p.parts and p.suffix != ".pyc")
    entries = [[p.relative_to(root).as_posix(), digest(p.read_bytes())]
               for p in sorted(set(files)) if p.is_file()]
    return {"sha256": digest(json.dumps(entries, separators=(",", ":")).encode()), "files": entries}


def provenance(root, binary):
    def command(*args):
        try:
            result = subprocess.run(args, cwd=root, capture_output=True, text=True, timeout=10)
            return {"exit_code": result.returncode, "stdout": result.stdout.strip(), "stderr": result.stderr.strip()}
        except (OSError, subprocess.TimeoutExpired) as error:
            return {"state": "unavailable", "reason": str(error)}
    binary = Path(binary)
    return {"implementation": implementation(root), "binary": str(binary.resolve()),
            "binary_sha256": digest(binary.read_bytes()) if binary.is_file() else None,
            "git": {"head": command("git", "rev-parse", "HEAD"), "status": command("git", "status", "--porcelain=v1")},
            "toolchain": {name: command(name, "--version") for name in ("cargo", "rustc", "python3", "node")},
            "source_binary_binding": "unverified_without_matching_build_receipt"}


def check_build_receipt(receipt, source, binary):
    if (receipt.get("build", {}).get("exit_code") != 0
            or receipt.get("implementation_sha256") != source["sha256"]
            or receipt.get("binary_sha256") != digest(Path(binary).read_bytes())):
        raise ValueError("Build receipt does not bind this executable to the current implementation")
    return {"state": "matching_build_receipt", "receipt_sha256": digest(json.dumps(receipt, sort_keys=True).encode())}


class RunEvidence:
    """One immutable directory per run. Each invocation keeps exact raw streams."""
    def __init__(self, directory):
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        if any(self.directory.iterdir()):
            raise ValueError(f"Evidence directory must be empty: {self.directory}")
        self.sequence = 0
        self.lock = threading.Lock()

    def run(self, command, *, cwd=None, timeout=60):
        with self.lock:
            self.sequence += 1
            stem = self.directory / f"{self.sequence:06d}"
        record = {"command": [str(x) for x in command], "cwd": str(cwd) if cwd else None,
                  "started_at_unix": time.time(), "timeout_seconds": timeout}
        started = time.monotonic()
        try:
            result = subprocess.run(command, cwd=cwd, capture_output=True, timeout=timeout)
            stdout, stderr = result.stdout, result.stderr
            record["exit_code"] = result.returncode
        except (OSError, subprocess.TimeoutExpired) as error:
            stdout, stderr = getattr(error, "stdout", None) or b"", getattr(error, "stderr", None) or b""
            record.update(state="error", error=str(error))
            raise
        finally:
            record["elapsed_seconds"] = time.monotonic() - started
            stem.with_suffix(".stdout").write_bytes(stdout)
            stem.with_suffix(".stderr").write_bytes(stderr)
            record.update(stdout_sha256=digest(stdout), stderr_sha256=digest(stderr))
            stem.with_suffix(".json").write_text(json.dumps(record, indent=2) + "\n")
        return subprocess.CompletedProcess(command, result.returncode, stdout.decode("utf-8"), stderr.decode("utf-8", errors="replace"))

    def record(self, command, request, reply, *, cwd=None, timeout=60, started_at, elapsed, state=None,
               stderr=None, exit_code=None):
        """Retain one request/reply exchanged with a long-lived process."""
        with self.lock:
            self.sequence += 1
            stem = self.directory / f"{self.sequence:06d}"
        stdin, stdout = request.encode("utf-8"), reply.encode("utf-8")
        record = {"command": [str(x) for x in command], "cwd": str(cwd) if cwd else None, "transport": "serve",
                  "started_at_unix": started_at, "timeout_seconds": timeout, "elapsed_seconds": elapsed,
                  "stdin_sha256": digest(stdin), "stdout_sha256": digest(stdout)}
        if state:
            record["state"] = state
        if exit_code is not None:
            record["exit_code"] = exit_code
        if stderr is not None:
            raw_errors = stderr.encode("utf-8")
            stem.with_suffix(".stderr").write_bytes(raw_errors)
            record["stderr_sha256"] = digest(raw_errors)
        stem.with_suffix(".stdin").write_bytes(stdin)
        stem.with_suffix(".stdout").write_bytes(stdout)
        stem.with_suffix(".json").write_text(json.dumps(record, indent=2) + "\n")


def atomic_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + f".{os.getpid()}.tmp")
    try:
        temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)
