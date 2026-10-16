#!/usr/bin/env python3
"""Report (and optionally remove) stale agent-session artifacts.

Every harness session leaves probes, databases, logs and before/after JSON
under .agents/artifacts/. Nothing owns that ignored tree, so it grows without
bound. This script lists the top-level entries that are older than --days or
larger than --max-mb, plus every stray database file anywhere below, and
never touches tracked files (the retained records and the skill tree).

Report-only by default; `verify-lint` runs it that way and it always exits 0
there. Pass --delete to remove the listed untracked entries.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
DB_SUFFIXES = ('.db', '.db-wal', '.db-shm')


def tracked(root):
    try:
        out = subprocess.run(['git', 'ls-files', '-z', '.agents'], cwd=root,
                             capture_output=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return set()
    return {root / p for p in out.decode().split('\0') if p}


def size(path):
    if path.is_symlink() or path.is_file():
        return path.lstat().st_size
    return sum(p.lstat().st_size for p in path.rglob('*') if p.is_file() and not p.is_symlink())


def newest(path):
    if not path.is_dir() or path.is_symlink():
        return path.lstat().st_mtime
    times = [p.lstat().st_mtime for p in path.rglob('*')]
    return max(times, default=path.lstat().st_mtime)


def stale(root, days, max_mb, now):
    """Untracked top-level artifact entries (age or size) and stray databases."""
    base = root / '.agents/artifacts'
    if not base.is_dir():
        return []
    keep = tracked(root)
    holds_tracked = lambda p: p in keep or any(k.is_relative_to(p) for k in keep)
    found = {}
    for entry in sorted(base.iterdir()):
        if holds_tracked(entry):
            continue
        bytes_, age = size(entry), (now - newest(entry)) / 86400
        why = [w for w, hit in [(f'older than {days} days', age > days),
                                (f'larger than {max_mb} MB', bytes_ > max_mb * 2**20)] if hit]
        if why:
            found[entry] = (bytes_, why)
    for db in sorted(p for p in base.rglob('*') if p.name.endswith(DB_SUFFIXES)):
        if db not in keep and not any(db.is_relative_to(e) for e in found):
            found[db] = (db.lstat().st_size, ['stray database'])
    return [(p, b, why) for p, (b, why) in sorted(found.items())]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--days', type=int, default=14)
    parser.add_argument('--max-mb', type=int, default=50)
    parser.add_argument('--delete', action='store_true', help='remove the listed untracked entries')
    args = parser.parse_args(argv)
    entries = stale(ROOT, args.days, args.max_mb, time.time())
    total = sum(b for _, b, _ in entries)
    for path, bytes_, why in entries:
        print(f'{bytes_ / 2**20:9.1f} MB  {path.relative_to(ROOT)}  ({"; ".join(why)})')
        if args.delete:
            shutil.rmtree(path) if path.is_dir() and not path.is_symlink() else path.unlink()
    verb = 'removed' if args.delete else 'reclaimable (report only; pass --delete to remove)'
    print(f'artifacts: {len(entries)} entries, {total / 2**20:.1f} MB {verb}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
