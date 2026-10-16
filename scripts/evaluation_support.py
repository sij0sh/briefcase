"""Small subprocess, digest and same-unit fact helpers for evaluations."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import time


def digest(raw):
    return 'sha256:' + hashlib.sha256(raw).hexdigest()


class Briefcase:
    def __init__(self, binary, db, *, cwd=None, timeout=600):
        self.binary, self.db = binary, db
        self.cwd, self.timeout = cwd, timeout
        self.last_stdout, self.last_stderr, self.last_seconds = b'', b'', 0.0

    def run(self, *args, json_output=True):
        command = [str(self.binary), '--db', str(self.db)] + (['--json'] if json_output else []) + list(args)
        start = time.monotonic()
        self.last_stdout, self.last_stderr = b'', b''
        try:
            result = subprocess.run(command, cwd=self.cwd, capture_output=True, timeout=self.timeout)
        except subprocess.TimeoutExpired as error:
            self.last_stdout, self.last_stderr = error.stdout or b'', error.stderr or b''
            raise RuntimeError(f'{args[0]}: deadline exceeded ({self.timeout}s)') from error
        finally:
            self.last_seconds = time.monotonic() - start
        self.last_stdout, self.last_stderr = result.stdout, result.stderr
        if result.returncode != 0:
            raise RuntimeError(f'{args[0]}: {result.stderr.decode("utf-8", errors="replace").strip()[:400]}')
        return json.loads(result.stdout) if json_output else result.stdout.decode('utf-8')

    def call(self, operation, arguments, *, fallback=False):
        extra = ['--fallback-published'] if fallback else []
        return self.run('call', operation, '--args', json.dumps(arguments, ensure_ascii=False), *extra)


def warn_if_stale(binary, root):
    """Evaluation must measure the code under test, not an older build."""
    built = Path(binary).stat().st_mtime
    sources = [p for d in ['src', 'extensions', 'contracts'] for p in (Path(root) / d).rglob('*') if p.is_file()]
    newer = [p for p in sources if p.stat().st_mtime > built]
    if newer:
        print(f'warning: {binary} is older than {len(newer)} source file(s) (e.g. {newer[0].relative_to(root)}); '
              'rebuild with `cargo build --locked --bins` or pass --binary', file=sys.stderr)
    return newer


def json_bytes(value):
    """Complete compact UTF-8 JSON plus the CLI's one trailing LF."""
    return len(json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode('utf-8')) + 1


def token(t):
    return re.compile(r'(?<![A-Za-z0-9_])' + re.escape(t) + r'(?![A-Za-z0-9_])')


def present(fact, units):
    groups = [[token(t) for t in group] for group in fact['all']]
    return any(all(any(p.search(u) for p in group) for group in groups) for u in units)
