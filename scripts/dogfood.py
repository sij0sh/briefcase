#!/usr/bin/env python3
"""Exercise canonical context reads on this repository in an isolated database.

Writes retained brief/search/status answers and a question summary. These are
smoke checks, not the missing original evaluation corpus or measured quality.
Optional session imports require --sessions. No external provider is called.
"""
import argparse
import json
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
QUESTIONS = [
    {'id':'brief','question':'review service dispatch','operation':'brief',
     'arguments':{'task':'review src/architecture/service.rs','duty':'review','max_bytes':24576},
     'expect':{'contains':['task-context-v1','not_covered','src/architecture/service.rs']}},
    {'id':'search','question':'find retained context text','operation':'search',
     'arguments':{'text':'retained','history':False,'max_bytes':24576},
     'expect':{'contains':['report_id','items','retained']}},
    {'id':'status','question':'inspect retained inventory','operation':'status',
     'arguments':{'summary':True,'max_bytes':24576},'expect':{'contains':['inventory']}},
]
HARNESSES = ['pi', 'codex', 'claude-code']
from evaluation_support import Briefcase as BaseTrace, digest, json_bytes, warn_if_stale


class Briefcase(BaseTrace):
    def __init__(self, binary, db):
        super().__init__(binary, db, cwd=ROOT, timeout=3600)


def answer(trace, q):
    try:
        result = trace.call(q['operation'], q['arguments'])
    except RuntimeError as e:
        return {'id': q['id'], 'question': q['question'], 'status': 'error', 'error': str(e)[:400]}
    # Facts and budgets cover the complete response, not just payload.text.
    text = json.dumps(result, separators=(',', ':'), ensure_ascii=False)
    raw = getattr(trace, 'last_stdout', None)
    answer_bytes = len(raw) if isinstance(raw, bytes) and raw else json_bytes(result)
    caps = [q['expect'].get('max_answer_bytes'), q.get('arguments', {}).get('max_bytes')]
    cap = min((n for n in caps if isinstance(n, int)), default=None)
    missing = [f for f in q['expect'].get('contains', []) if f not in text]
    present = [f for f in q['expect'].get('excludes', []) if f in text]
    over = cap is not None and answer_bytes > cap
    return {'id': q['id'], 'question': q['question'], 'operation': q['operation'],
            'status': 'pass' if not missing and not present and not over else 'fail',
            'missing': missing, 'unexpected': present, 'answer_bytes': answer_bytes, 'byte_count_basis': 'wire' if raw else 'compact_json_lf',
            'over_bytes': over}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    # The just-built debug binary is what tests and the Pi extension run; a
    # release build can lag the tree and measure code nobody ships.
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/briefcase')
    parser.add_argument('--out', type=Path, default=ROOT / 'evaluation/dogfood')
    parser.add_argument('--workdir', type=Path, help='keep the database here')
    parser.add_argument('--sessions', action='append', choices=HARNESSES, default=[],
                        help="import this machine's sessions of a harness for the repository "
                             "(repeatable; private; off by default)")
    parser.add_argument('--questions', type=Path, help='optional authored JSON question list; defaults to current context smoke cases')
    parser.add_argument('--pi-sessions', action='store_true', help='same as --sessions pi')
    args = parser.parse_args()
    warn_if_stale(args.binary, ROOT)
    harnesses = sorted(set(args.sessions) | ({'pi'} if args.pi_sessions else set()))
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory() as tmp:
        work = args.workdir or Path(tmp)
        work.mkdir(parents=True, exist_ok=True)
        db = work / 'dogfood.db'
        for stale in work.glob('dogfood.db*'):
            stale.unlink()
        trace = Briefcase(binary, db)
        captured = trace.run('capture-text', str(ROOT))
        for harness in harnesses:
            trace.run('sessions', 'import-local', '--harness', harness, '--repository-root', str(ROOT))
        args.out.mkdir(parents=True, exist_ok=True)
        questions = json.loads(args.questions.read_text()) if args.questions else QUESTIONS
        if isinstance(questions, dict):
            questions = questions['questions']
        results = []
        for q in questions:
            result = answer(trace, q)
            results.append(result)
        for operation, arguments in [('brief',QUESTIONS[0]['arguments']),('search',QUESTIONS[1]['arguments']),('status',QUESTIONS[2]['arguments'])]:
            (args.out / f'{operation}.json').write_text(json.dumps(trace.call(operation, arguments), indent=2, ensure_ascii=False) + '\n')
        if harnesses:
            (args.out / 'session-activity.json').write_text(json.dumps(trace.call('session_activity', {}), indent=2) + '\n')
    passed = sum(r['status'] == 'pass' for r in results)
    rates = {'passed': passed, 'failed': sum(r['status'] == 'fail' for r in results),
             'errors': sum(r['status'] == 'error' for r in results),
             'max_answer_bytes': max((r.get('answer_bytes', 0) for r in results), default=0)}
    summary = {
        'schema_version': '1.0',
        'kind': 'dogfood_baseline',
        'binary_digest': digest(binary.read_bytes()),
        'question_digest': digest(json.dumps(questions, sort_keys=True).encode()),
        'view': captured.get('view_id'),
        'pi_sessions_imported': 'pi' in harnesses,
        'sessions_imported': harnesses,
        'questions': {'passed': passed, 'total': len(results), 'rates': rates, 'results': results},
    }
    (args.out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(f"rates: errors={rates['errors']} failed={rates['failed']} max_answer_bytes={rates['max_answer_bytes']}")
    print(f"questions {passed}/{len(results)}; wrote {args.out}")
    for r in results:
        if r['status'] != 'pass':
            print(f"  {r['id']} {r['status']}: missing={r.get('missing')} unexpected={r.get('unexpected')} {r.get('error', '')}")
    return 0 if passed == len(results) else 1


if __name__ == '__main__':
    sys.exit(main())
