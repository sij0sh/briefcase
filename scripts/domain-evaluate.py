#!/usr/bin/env python3
"""Measure BELONGS_TO_DOMAIN inference quality on labeled real repositories.

Runs the existing classification workflow unmodified (baseline), then packet
ablations, and scores provider outputs against human labels. Boundary
validation and semantic truth are reported separately: this script measures
semantic quality only. It never uses a model to create labels.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
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
    parser.add_argument('--gold', type=Path, default=ROOT / 'evaluation/domain-gold.json')
    parser.add_argument('--provider', type=Path, required=True, help='Local provider configuration')
    parser.add_argument('--output', type=Path, default=ROOT / 'BUILD/domain-evaluation')
    parser.add_argument('--repeats', type=int, default=3, help='Repeated runs per component for stability')
    parser.add_argument('--budgets', type=int, nargs='*', default=[4096, 16384],
                        help='max_bytes ablation points (baseline uses the largest)')
    parser.add_argument('--max-assertions', type=int, nargs='*', default=[4, 16],
                        help='max_assertions ablation points (baseline uses the largest)')
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
    report = {'schema_version': '1.0', 'evaluation': 'domain', 'status': 'running',
              'scope': gold['scope'], 'started_unix': started,
              'binary_digest': digest(binary.read_bytes()),
              'gold_digest': digest(gold_bytes), 'workload_digest': digest(gold_bytes),
              'scoring_digest': digest(Path(__file__).read_bytes()),
              'provider_digest': digest((run / 'provider.json').read_bytes()),
              'artifact_directory': str(run), 'bridge_retirement_authorized': False,
              'expansion_decision': 'NOT_EVALUATED', 'expansion_reasons': [],
              'repositories': [], 'skipped_repositories': []}
    env = dict(os.environ, XDG_STATE_HOME=str(run / 'state'))

    def command(directory, name, argv, timeout=300):
        start = time.monotonic()
        result = subprocess.run(list(map(str, argv)), capture_output=True, text=True, timeout=timeout, env=env)
        elapsed = time.monotonic() - start
        (directory / (name + '.stderr')).write_text(result.stderr)
        if result.returncode != 0:
            write(directory / (name + '.json'), {'exit_code': result.returncode, 'stderr': result.stderr[:2000]})
            return None, elapsed
        value = json.loads(result.stdout)
        write(directory / (name + '.json'), value)
        return value, elapsed

    try:
        for repo in gold['repositories']:
            if repo.get('status', 'active') != 'active' or not repo.get('root'):
                report['skipped_repositories'].append({'id': repo['id'], 'status': repo.get('status', 'planned'),
                    'reason': 'repository is not bound yet'})
                continue
            directory = run / repo['id']
            directory.mkdir()
            root = (ROOT / repo['root']).resolve()
            database = directory / 'architecture.db'
            architecture = [binary, '--json', '--db', database]
            taxonomy = directory / 'taxonomy.json'
            write(taxonomy, {'schema_version': '1.0', 'version': repo.get('taxonomy_version', 'gold-v1'),
                             'domains': repo['taxonomy']})
            scan_args = ['scan', root, '--label', 'captured', '--repository-id', repo['id'], *repo.get('scan_args', [])]
            if 'policy' in repo:
                policy = directory / 'policy.json'
                write(policy, repo['policy'])
                scan_args += ['--policy', policy]
            scan, scan_seconds = command(directory, 'scan', [*architecture, *scan_args])
            if scan is None:
                report['skipped_repositories'].append({'id': repo['id'], 'status': 'scan_failed',
                    'reason': 'scan did not complete'})
                continue
            with sqlite3.connect(database) as sql:
                bundle = json.loads(sql.execute('SELECT bundle FROM views WHERE id=?', (scan['view_id'],)).fetchone()[0])
            entities = {e['id']: e for e in bundle['entity_versions']}
            declarations = {}
            for a in bundle['assertions']:
                if a['class'] == 'declaration' and a['proposition']['predicate'] == 'BELONGS_TO_DOMAIN':
                    declarations[entities[a['proposition']['subject_id']]['canonical_key']] = \
                        entities[a['proposition']['object']['entity_version_id']]['canonical_key']
            result = {'id': repo['id'], 'scan_seconds': scan_seconds, 'components': []}
            for component in repo['components']:
                key = component['component']
                accepted = set(component['accepted_domains'])
                entry = {'component': key, 'accepted_domains': sorted(accepted),
                         'acceptable_abstention': component['acceptable_abstention'],
                         'has_declaration': component['has_declaration'],
                         'declared_domain': declarations.get(key),
                         'baseline': None, 'repeats': [], 'ablations': [], 'provider_errors': 0}
                baseline_budget = max(args.budgets)
                baseline_assertions = max(args.max_assertions)
                # Baseline: existing workflow unmodified. Repeats measure stability.
                predictions = []
                for attempt in range(args.repeats):
                    value, seconds = command(directory, f"classify-{key}-{attempt}",
                        [*architecture, 'classify', '--view', 'captured', '--component', key,
                         '--provider', run / 'provider.json', '--taxonomy', taxonomy,
                         '--max-assertions', str(baseline_assertions), '--max-bytes', str(baseline_budget)])
                    if value is None:
                        entry['provider_errors'] += 1
                        continue
                    predicted = classify_prediction(database, value)
                    predictions.append(predicted)
                    record = {'latency_seconds': seconds, 'abstained': value['abstained'],
                              'predicted_domains': predicted, 'output_bytes': len(json.dumps(value))}
                    entry['repeats'].append(record)
                if entry['repeats']:
                    entry['baseline'] = entry['repeats'][0]
                    entry['stability'] = {
                        'runs': len(predictions),
                        'distinct_predictions': len({json.dumps(p, sort_keys=True) for p in predictions}),
                        'stable': len({json.dumps(p, sort_keys=True) for p in predictions}) == 1}
                # Budget ablations: smaller evidence budgets, single run each.
                for budget in sorted(set(args.budgets))[:-1] or []:
                    value, seconds = command(directory, f"classify-{key}-bytes-{budget}",
                        [*architecture, 'classify', '--view', 'captured', '--component', key,
                         '--provider', run / 'provider.json', '--taxonomy', taxonomy,
                         '--max-assertions', str(baseline_assertions), '--max-bytes', str(budget)])
                    if value is None:
                        entry['provider_errors'] += 1
                        continue
                    entry['ablations'].append({'variant': f'max_bytes={budget}', 'latency_seconds': seconds,
                        'abstained': value['abstained'], 'predicted_domains': classify_prediction(database, value)})
                for count in sorted(set(args.max_assertions))[:-1] or []:
                    value, seconds = command(directory, f"classify-{key}-assertions-{count}",
                        [*architecture, 'classify', '--view', 'captured', '--component', key,
                         '--provider', run / 'provider.json', '--taxonomy', taxonomy,
                         '--max-assertions', str(count), '--max-bytes', str(baseline_budget)])
                    if value is None:
                        entry['provider_errors'] += 1
                        continue
                    entry['ablations'].append({'variant': f'max_assertions={count}', 'latency_seconds': seconds,
                        'abstained': value['abstained'], 'predicted_domains': classify_prediction(database, value)})
                entry['ablation_status'] = {'source_only': 'not_supported',
                    'source_plus_dependencies': 'not_supported',
                    'routing_text_excluded_from_evidence': 'not_supported',
                    'graph_neighbor_evidence': 'not_supported',
                    'retrieval_ranked_evidence': 'not_supported',
                    'note': 'Evidence-budget ablations run above. Packet-composition ablations need producer packet variants; epistemic authority is unchanged in all runs.'}
                result['components'].append(entry)
            summarize(result)
            report['repositories'].append(result)
        report['status'] = 'measured'
        report['finished_unix'] = time.time()
        gate = gold.get('expansion_gate', {})
        reasons = []
        measured = [c for r in report['repositories'] for c in r['components'] if c.get('baseline')]
        if not measured:
            reasons.append('no labeled components measured yet')
        for field in ['minimum_acceptable_precision', 'acceptable_abstention_behavior', 'stability_threshold',
                      'maximum_false_disagreement_rate', 'cost_latency_envelope']:
            if gate.get(field) is None:
                reasons.append(f'expansion gate has no approved {field}')
        report['expansion_reasons'] = reasons
        report['expansion_decision'] = 'NOT_EVALUATED' if reasons else 'EVALUATED'
    except Exception as error:
        report.update(status='failed', error=str(error))
        raise
    finally:
        write(run / 'report.json', report)
        write(args.output / 'report.json', report)
        print(json.dumps({'status': report['status'], 'report': str(args.output / 'report.json')}))


def classify_prediction(database, value):
    # Read predicted domains from retained claims, not from provider stdout.
    # Claims and their domain entities live in source overlays merged at load.
    with sqlite3.connect(database) as sql:
        row = sql.execute('SELECT bundle FROM views WHERE id=?', (value['view_id'],)).fetchone()
        overlays = sql.execute(
            "SELECT o.bundle FROM overlays o JOIN view_overlays v ON o.id=v.overlay_id WHERE v.view_id=? AND o.scope='source'",
            (value['view_id'],)).fetchall()
    if row is None:
        return []
    bundles = [json.loads(row[0])] + [json.loads(body) for (body,) in overlays]
    entities = {}
    for bundle in bundles:
        entities.update({e['id']: e for e in bundle['entity_versions']})
    domains = []
    for bundle in bundles:
        for a in bundle['assertions']:
            if a['id'] in value.get('claim_ids', []) and a['proposition']['object'].get('entity_version_id') in entities:
                domains.append(entities[a['proposition']['object']['entity_version_id']]['canonical_key'])
    return sorted(set(domains))


def summarize(result):
    measured = [c for c in result['components'] if c.get('baseline')]
    if not measured:
        result['summary'] = {'measured_components': 0}
        return
    exact = 0
    abstentions = 0
    appropriate_abstentions = 0
    false_confident = 0
    disagreements = 0
    correct_disagreements = 0
    for c in measured:
        predicted = set(c['baseline']['predicted_domains'])
        accepted = set(c['accepted_domains'])
        if c['baseline']['abstained'] or not predicted:
            abstentions += 1
            if c['acceptable_abstention']:
                appropriate_abstentions += 1
        elif predicted == accepted or (predicted and predicted <= accepted):
            exact += 1
        else:
            false_confident += 1
        declared = c.get('declared_domain')
        if declared and predicted and declared not in predicted:
            disagreements += 1
            if accepted and declared not in accepted:
                correct_disagreements += 1
    result['summary'] = {
        'measured_components': len(measured),
        'exact_classification_accuracy': exact / len(measured),
        'abstention_rate': abstentions / len(measured),
        'appropriate_abstention_rate': appropriate_abstentions / abstentions if abstentions else None,
        'false_confident_assignment_rate': false_confident / len(measured),
        'claim_declaration_disagreements': disagreements,
        'correct_disagreement_rate': correct_disagreements / disagreements if disagreements else None,
        'provider_errors': sum(c['provider_errors'] for c in measured),
        'mean_latency_seconds': sum(c['baseline']['latency_seconds'] for c in measured) / len(measured),
        'unstable_components': sum(1 for c in measured if not c.get('stability', {}).get('stable', True)),
    }


if __name__ == '__main__':
    main()
