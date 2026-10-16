#!/usr/bin/env python3
"""Score historical-lineage quality against labeled before/after fixtures.

For each active scenario: scan both snapshots, prepare correspondence, query
exact/ambiguous modes (no provider), run classify-history-lineage with the
given provider, then query likely/combined modes. Deterministic exactness
and provider-backed judgments are scored separately per the gold verdicts.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def digest(raw):
    return 'sha256:' + hashlib.sha256(raw).hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/briefcase')
    parser.add_argument('--gold', type=Path, default=ROOT / 'evaluation/lineage-gold.json')
    parser.add_argument('--provider', type=Path, required=True, help='Local provider configuration')
    parser.add_argument('--output', type=Path, default=ROOT / 'BUILD/lineage-evaluation')
    parser.add_argument('--max-bytes', type=int, default=65536)
    args = parser.parse_args()
    binary = args.binary.resolve()
    gold_bytes = args.gold.read_bytes()
    gold = json.loads(gold_bytes)
    args.output.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix='run-', dir=args.output.resolve()))
    (run / 'gold.json').write_bytes(gold_bytes)
    shutil.copy2(binary, run / 'briefcase')
    shutil.copy2(args.provider, run / 'provider.json')
    binary = run / 'briefcase'
    started = time.time()
    report = {'schema_version': '1.0', 'evaluation': 'lineage', 'status': 'running',
              'scope': gold['scope'], 'started_unix': started,
              'binary_digest': digest(binary.read_bytes()),
              'gold_digest': digest(gold_bytes), 'workload_digest': digest(gold_bytes),
              'scoring_digest': digest(Path(__file__).read_bytes()),
              'provider_digest': digest((run / 'provider.json').read_bytes()),
              'artifact_directory': str(run), 'bridge_retirement_authorized': False,
              'scenarios': [], 'skipped_scenarios': []}
    env = dict(os.environ, XDG_STATE_HOME=str(run / 'state'))

    def command(directory, name, argv, timeout=300):
        start = time.monotonic()
        result = subprocess.run(list(map(str, argv)), capture_output=True, text=True, timeout=timeout, env=env)
        elapsed = time.monotonic() - start
        (directory / (name + '.stderr')).write_text(result.stderr)
        if result.returncode != 0:
            write(directory / (name + '.json'), {'exit_code': result.returncode, 'stderr': result.stderr[:2000]})
            return None, elapsed, result.stderr.strip()
        value = json.loads(result.stdout)
        write(directory / (name + '.json'), value)
        return value, elapsed, ''

    try:
        for scenario in gold.get('scenarios', gold.get('cases', [])):
            if scenario.get('status', 'active') != 'active':
                report['skipped_scenarios'].append({'id': scenario['id'], 'reason': 'not active'})
                continue
            entry = {'id': scenario['id'], 'kind': scenario['kind'], 'verdict': scenario['verdict'],
                     'provider_errors': 0, 'usage': None}
            directory = run / scenario['id']
            directory.mkdir()
            fixture = (ROOT / scenario['fixture']).resolve()
            # Labels are bound to fixture bytes.
            for labeled in scenario['files']:
                actual = digest((ROOT / labeled['path']).read_bytes())
                if actual != labeled['content_digest']:
                    raise ValueError(f"{scenario['id']}: fixture drift for {labeled['path']}")
            database = directory / 'architecture.db'
            architecture = [binary, '--json', '--db', database]
            before, _, err = command(directory, 'scan-before',
                [*architecture, 'scan', fixture / 'before', '--label', 'base', '--repository-id', scenario['id'], '--syntax-only'])
            changed, _, err2 = command(directory, 'scan-changed',
                [*architecture, 'scan', fixture / 'changed', '--label', 'changed', '--repository-id', scenario['id'], '--syntax-only'])
            if before is None or changed is None:
                entry['provider_errors'] += 1
                entry['scan_error'] = (err or err2)[:500]
                report['scenarios'].append(entry)
                continue
            base_args = ['--base', 'base', '--view', 'changed']
            exact, _, _ = command(directory, 'lineage-exact', [*architecture, 'history-lineage', *base_args, '--mode', 'exact'])
            ambiguous, _, _ = command(directory, 'lineage-ambiguous', [*architecture, 'history-lineage', *base_args, '--mode', 'ambiguous'])
            entry['exact_findings'] = finding_pairs(exact) if exact else []
            entry['ambiguous_groups'] = len(ambiguous['details']) if ambiguous else 0
            classified, seconds, err = command(directory, 'classify',
                [*architecture, 'classify-history-lineage', *base_args, '--provider', run / 'provider.json',
                 '--max-bytes', str(args.max_bytes)], timeout=300)
            entry['classify_seconds'] = seconds
            entry['usage'] = parse_usage(directory / 'classify.stderr')
            if classified is None:
                if 'No uncertain lineage candidates' in err:
                    entry['abstained'] = True
                    entry['no_candidates'] = True
                    entry['claim_ids'] = []
                else:
                    entry['provider_errors'] += 1
                    entry['classify_error'] = err[:500]
                    report['scenarios'].append(entry)
                    continue
            else:
                entry['abstained'] = classified['abstained']
                entry['claim_ids'] = sorted(classified['claim_ids'])
                entry['no_candidates'] = False
                entry['output_bytes'] = retained_output_bytes(database, classified['inference_id'])
            likely, _, _ = command(directory, 'lineage-likely', [*architecture, 'history-lineage', *base_args, '--mode', 'likely'])
            combined, _, _ = command(directory, 'lineage-combined', [*architecture, 'history-lineage', *base_args, '--mode', 'combined'])
            entry['likely_findings'] = finding_pairs(likely) if likely else []
            entry['combined_classes'] = sorted({d.get('certainty_class') for d in combined['details'].values()}) if combined else []
            score(entry, scenario)
            report['scenarios'].append(entry)
        summarize(report)
        report['status'] = 'measured'
        report['finished_unix'] = time.time()
    except Exception as error:
        report.update(status='failed', error=str(error))
        raise
    finally:
        write(run / 'report.json', report)
        write(args.output / 'report.json', report)
        print(json.dumps({'status': report['status'], 'report': str(args.output / 'report.json')}))


def finding_pairs(answer):
    return sorted((d.get('from'), d.get('to'), d.get('predicate'), d.get('certainty_class'))
                  for d in answer['details'].values())


def retained_output_bytes(database, inference_id):
    import sqlite3
    with sqlite3.connect(database) as sql:
        row = sql.execute("SELECT body FROM records WHERE id=?", (inference_id,)).fetchone()
        if row is None:
            return None
        artifact_id = json.loads(row[0])['output_artifact_id']
        blob = sql.execute('SELECT bytes FROM payloads WHERE artifact_id=?', (artifact_id,)).fetchone()
        return len(blob[0]) if blob else None


def parse_usage(stderr_path):
    try:
        for line in stderr_path.read_text().splitlines():
            line = line.strip()
            if line.startswith('{') and 'provider_usage' in line:
                return json.loads(line)
    except OSError:
        pass
    return None


def score(entry, scenario):
    verdict = scenario['verdict']
    expected = scenario['expected']
    exact = [(f, t) for (f, t, p, c) in entry['exact_findings'] if p == 'RENAMED_FROM']
    likely = [(f, t) for (f, t, p, c) in entry['likely_findings'] if p == 'LIKELY_SAME_ENTITY']
    entry['exact_false_positive'] = False
    entry['likely_true_positive'] = False
    entry['likely_false_positive'] = False
    entry['appropriate_abstention'] = False
    if verdict == 'exact_required':
        pair = (expected['before_key'], expected['after_key'])
        entry['exact_required_met'] = pair in exact
        endpoints = set(pair)
        entry['likely_false_positive'] = any(f in endpoints or t in endpoints for (f, t) in likely)
    elif verdict == 'claim_acceptable':
        pair = (expected['before_key'], expected['after_key'])
        entry['exact_false_positive'] = pair in exact
        if likely == [pair]:
            entry['likely_true_positive'] = True
        elif not likely and entry['abstained']:
            entry['appropriate_abstention'] = True
        else:
            entry['likely_false_positive'] = True
    elif verdict in ('claim_false_positive', 'abstention_required'):
        participants = set(expected.get('participants', []))
        entry['exact_false_positive'] = any(f in participants and t in participants for (f, t) in exact)
        entry['likely_false_positive'] = any(f in participants or t in participants for (f, t) in likely)
        if not likely and entry['abstained']:
            entry['appropriate_abstention'] = True
    entry['ambiguity_preserved'] = entry['ambiguous_groups'] > 0 or verdict == 'exact_required'


def summarize(report):
    measured = [c for c in report['scenarios'] if 'exact_false_positive' in c]
    exact_fp = sum(c['exact_false_positive'] for c in measured)
    exact_required = [c for c in measured if c['verdict'] == 'exact_required']
    acceptable = [c for c in measured if c['verdict'] == 'claim_acceptable']
    fp_scenarios = [c for c in measured if c['verdict'] in ('claim_false_positive', 'abstention_required')]
    tp = sum(c['likely_true_positive'] for c in acceptable)
    fp = sum(c['likely_false_positive'] for c in measured)
    abstentions = sum(1 for c in measured if c.get('abstained'))
    split_merge = [c for c in measured if c['kind'] in ('split', 'merge')]
    total_tokens = 0
    for c in measured:
        usage = (c.get('usage') or {}).get('provider_usage', {})
        total_tokens += usage.get('total_tokens', 0) or 0
    report['summary'] = {
        'measured_scenarios': len(measured),
        'exact_required_met': sum(c.get('exact_required_met', False) for c in exact_required),
        'exact_required_total': len(exact_required),
        'exact_false_positives': exact_fp,
        'likely_true_positives': tp,
        'likely_false_positives': fp,
        'likely_precision': tp / (tp + fp) if (tp + fp) else None,
        'likely_recall': tp / len(acceptable) if acceptable else None,
        'abstention_rate': abstentions / len(measured) if measured else None,
        'appropriate_abstentions': sum(c['appropriate_abstention'] for c in measured),
        'ambiguity_preserved_scenarios': sum(c['ambiguity_preserved'] for c in measured),
        'split_merge_binary_claim_rate': sum(c['likely_false_positive'] for c in split_merge) / len(split_merge) if split_merge else None,
        'provider_errors': sum(c['provider_errors'] for c in report['scenarios']),
        'mean_classify_seconds': sum(c.get('classify_seconds', 0) for c in measured) / len(measured) if measured else None,
        'mean_output_bytes': sum(c.get('output_bytes', 0) for c in measured) / len(measured) if measured else None,
        'total_tokens': total_tokens,
        'token_note': 'provider child stderr is dropped by design, so token counts are unavailable; retained output bytes are the cost proxy',
    }


if __name__ == '__main__':
    main()
