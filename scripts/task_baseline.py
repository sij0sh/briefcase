"""Predeclared conventional rg/read baseline for Stage 1 comparisons.

Protocol v3 (v1 read every candidate to score it and v2 delivered the
full count listing, each exhausting the byte budget before ranking;
declared here before the scored run): extract task terms, run one
ripgrep count search piped through sort/head so only the top-32 ranked
file names reach the agent, then read those files in order until a
call/byte/time budget stops the run. No Briefcase index, graph, or
retained evidence is used.
"""
import re
import subprocess
import time
from pathlib import Path

STOP = {"the", "and", "for", "with", "from", "this", "that", "how", "what",
        "which", "does", "into", "through", "about", "can", "please", "would",
        "should", "could", "use", "using", "make", "change", "fix", "add", "new",
        "support", "called", "similar", "correctly", "when", "where", "find"}


def task_terms(task):
    terms = []
    for raw in re.split(r"[^A-Za-z0-9_]+", task.lower()):
        if len(raw) > 2 and raw not in STOP and raw not in terms:
            terms.append(raw)
    return terms[:32]


def rg_read_baseline(scenario, root, *, max_calls=300, max_bytes=2 * 1024 * 1024,
                     max_seconds=180, max_files=32):
    root = Path(root)
    started = time.monotonic()
    terms = task_terms(scenario["task"])
    calls, bytes_read = 0, 0
    limit = "complete"
    result = subprocess.run(["rg", "-c", "-i", "--no-messages", *sum((["-e", t] for t in terms), []), "."],
                            cwd=root, capture_output=True, text=True, timeout=max_seconds)
    calls += 1
    scored = []
    for line in result.stdout.splitlines():
        path, _, count = line.rpartition(":")
        if path and count.isdigit():
            scored.append((int(count), path[2:] if path.startswith("./") else path))
    candidates = scored
    scored.sort(key=lambda row: (-row[0], row[1]))
    # The shell pipeline delivers only the ranked head to the agent.
    bytes_read += sum(len(f"{path}:{count}\n".encode()) for count, path in scored[:max_files])
    read = []
    for _, relative in scored:
        if calls >= max_calls:
            limit = "calls"; break
        if len(read) >= max_files:
            break
        if time.monotonic() - started >= max_seconds:
            limit = "seconds"; break
        try:
            # A conventional read delivers a bounded first passage, not a
            # multi-megabyte dump of the top-ranked lock/generated file.
            data = (root / relative).read_bytes()[:65536]
        except OSError:
            continue
        calls += 1
        bytes_read += len(data)
        read.append(relative)
        if bytes_read >= max_bytes:
            limit = "bytes"; break
    expected = set(scenario.get("required_paths", []))
    return {"method": "rg/read predeclared baseline: one rg -l search, term-count ranking, ranked reads",
            "limit_reached": limit, "calls": calls, "bytes_read": bytes_read,
            "elapsed_seconds": round(time.monotonic() - started, 3),
            "candidate_files": len(candidates), "read_files": read,
            "recovered_required_paths": sorted(expected & set(read)),
            "missing_required_paths": sorted(expected - set(read))}
