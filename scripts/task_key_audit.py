"""Exact contract/configuration/state-key join audit for missed required paths.

Authored paths are used only here, after retrieval. A shared literal is an
association between two files, never proof that either reads or writes it.
"""
from collections import defaultdict
import os
from pathlib import Path
import re

from task_loss_boundary import retained_loss_report

LITERAL = re.compile(r"""(["'`])([^"'`\s\\]{4,120})\1""")
SPECIFIER = re.compile(r"""^\s*(import|export)\b.*\bfrom\b|^\s*}\s*from\s|^\s*import\s+["']|\brequire\(|\bimport\(""")
SOURCE_SUFFIXES = (".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts", ".json", ".jsonc", ".yaml", ".yml", ".toml", ".rs", ".py")
SKIP_DIRECTORIES = {"node_modules", ".git", ".repos", "dist", "build", "target", ".briefcase"}


def key_like(literal):
    """Technical keys: punctuated identifiers, paths, URLs or mixed-case names."""
    if not any(c.isalpha() for c in literal):
        return False
    # Relative specifiers and template placeholders are not exact keys.
    if literal.startswith(("./", "../")) or "${" in literal:
        return False
    return any(c in literal for c in ":./_-") or (literal != literal.lower() and literal != literal.upper())


def literals(text):
    found = set()
    for line in text.splitlines():
        if SPECIFIER.search(line):
            continue
        for match in LITERAL.finditer(line):
            if key_like(match.group(2)):
                found.add(match.group(2))
    return found


def literal_index(root):
    """Exact quoted literal -> repository files containing it."""
    files, index = {}, defaultdict(set)
    for directory, children, names in os.walk(root):
        children[:] = sorted(c for c in children if c not in SKIP_DIRECTORIES)
        for name in names:
            if not name.endswith(SOURCE_SUFFIXES):
                continue
            path = Path(directory, name)
            relative = path.relative_to(root).as_posix()
            try:
                text = path.read_text(encoding="utf-8")
            except (UnicodeDecodeError, OSError):
                continue
            found = literals(text)
            files[relative] = found
            for literal in found:
                index[literal].add(relative)
    return files, index


def audit(database, baseline, scenarios, root, specificity=8):
    loss = retained_loss_report(database, baseline, scenarios)
    files, index = literal_index(root)
    by_id = {row["id"]: row for row in baseline["scenarios"]}
    rows, totals = [], defaultdict(int)
    for scenario, case in zip(scenarios, loss["scenarios"]):
        selected = {path for path, row in case["paths"].items() if row["selected"]}
        selected |= set(by_id[scenario["id"]].get("selected_paths") or [])
        anchors = {scenario["target"]} if scenario.get("target") else set()
        partners = selected | anchors
        missed = {}
        for path, row in case["paths"].items():
            if row["first_loss_stage"] == "manifest_delivered":
                continue
            shared = []
            for literal in sorted(files.get(path, ())):
                holders = index[literal] - {path}
                joined = sorted(holders & partners)
                if joined and len(index[literal]) <= specificity:
                    shared.append({"key": literal, "partners": joined, "repository_files": len(index[literal])})
            verdict = "exact_key_join_available" if shared else ("no_exact_key_join" if path in files else "not_read")
            totals[verdict] += 1
            missed[path] = {"first_loss_stage": row["first_loss_stage"], "lexical_rank": row["lexical_rank"],
                "candidate_rank": row["candidate_rank"], "score": row["score"], "verdict": verdict,
                "shared_keys": shared[:12], "shared_key_count": len(shared)}
        rows.append({"id": scenario["id"], "partners": sorted(partners), "missed": missed})
    return {"specificity_limit": specificity, "verdict_counts": dict(sorted(totals.items())),
        "first_loss_stage_counts": loss["first_loss_stage_counts"], "scenarios": rows,
        "interpretation": "An exact shared literal of bounded repository frequency is a candidate join between a missed path and a selected or anchor path. It is association only: it does not establish a read, write, call or runtime dependency."}
