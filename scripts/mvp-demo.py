#!/usr/bin/env python3
"""Exercise the compiled product against independent gold, retaining its real CLI outputs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/briefcase')
    parser.add_argument('--output', type=Path, default=ROOT / 'BUILD/mvp-demo')
    parser.add_argument('--workload-repo', type=Path, help='Read-only workload assessment of another Rust checkout')
    parser.add_argument('--large-modules', type=int, default=0, help='Additional supported synthetic workload, 2..256 modules')
    args = parser.parse_args()
    binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix='run-', dir=args.output.resolve()))
    binary_digest = 'sha256:' + hashlib.sha256(binary.read_bytes()).hexdigest()
    db = run / 'architecture.db'
    measures = []
    analyzer = os.environ.get('BRIEFCASE_RUST_ANALYZER', 'rust-analyzer')

    def command(name, *arguments, database=db):
        argv = [str(binary), '--json', '--db', str(database), *map(str, arguments)]
        metrics_file = run / (name + '-resources.json')
        if Path('/usr/bin/time').is_file():
            argv = ['/usr/bin/time', '-f', '{"wall_seconds":%e,"peak_rss_kib":%M}', '-o', str(metrics_file), *argv]
        start = time.monotonic()
        process = subprocess.run(argv, capture_output=True, text=True, timeout=180)
        if process.returncode:
            raise RuntimeError(f'{name}: {process.stderr}')
        output = json.loads(process.stdout)
        (run / (name + '.json')).write_text(json.dumps(output, indent=2) + '\n')
        stats = json.loads(metrics_file.read_text()) if metrics_file.exists() else {}
        measures.append({'operation': name, 'elapsed_seconds': round(time.monotonic() - start, 4), **stats})
        return output

    policy = ROOT / 'evaluation/sources/policy/architecture.json'
    base = command('base', 'scan', ROOT / 'evaluation/sources/rust/base', '--repository-id', 'demo', '--label', 'base', '--policy', policy, '--analyzer', analyzer)
    changed = command('changed', 'scan', ROOT / 'evaluation/sources/rust/changed', '--repository-id', 'demo', '--label', 'changed', '--policy', policy, '--analyzer', analyzer)
    assert base['diagnostics']['coverage']['resolution_complete'] is True
    assert changed['diagnostics']['coverage']['resolution_complete'] is True
    dependencies = command('dependencies', 'dependencies', '--view', 'changed')
    expected = json.loads((ROOT / 'evaluation/gold/source-expectations.json').read_text())['rust']
    pairs = sorted([[d['from'], d['to']] for d in dependencies['details'].values() if d.get('predicate') == 'DEPENDS_ON'])
    assert pairs == expected['changed_imports'], pairs
    comparison = command('compare', 'compare', '--base', 'base', '--view', 'changed')
    assert sorted([[d['from'], d['to']] for d in comparison['details'].values() if d['change'] == 'added']) == expected['added_imports']
    assert len([d for d in dependencies['details'].values() if d.get('predicate') == 'IN_CYCLE']) == 2
    assert command('base-policy', 'policy', '--view', 'base')['response']['results'][0]['conclusion'] == 'pass'
    violation = command('changed-policy', 'policy', '--view', 'changed')
    assert violation['response']['results'][0]['conclusion'] == 'violation'
    handle = violation['response']['results'][0]['explanation_handle']
    explanation = command('explain', 'explain', handle)
    assert any('use crate::orders::audit' in s['text'] for a in explanation['support'] for s in a.get('sources', []))
    # Compare retained support snippets and retained-source recall with independent labels.
    structure = command('recall-structure', 'dependencies', '--view', 'changed', '--include-structure')
    support_sources = []
    for i, row in enumerate(structure['response']['results']):
        proof = command(f'recall-proof-{i}', 'explain', row['explanation_handle'])
        support_sources.extend({'path': s['artifact']['path'], 'text': s['text']}
                               for a in proof['support'] for s in a.get('sources', []))
    questions = json.loads((ROOT / 'evaluation/gold/recall-questions.json').read_text())
    recall = {'scope': questions['scope'], 'questions': []}
    for question in questions['questions']:
        packet = command('recall-' + question['id'], 'call', 'recall', '--args',
                         json.dumps({'text': question['bridge_query'], 'view': 'changed', 'limit': 25, 'history': False}))
        recalled_sources = [{'path': item['path'], 'text': item['text']} for item in packet['items']]
        def hits(sources):
            return sum(any(s['path'] == expected['path'] and expected['text'] in s['text']
                           for s in sources) for expected in question['required_sources'])
        result = {'id': question['id'], 'question': question['question'],
                  'required_snippets': len(question['required_sources']),
                  'recall_recalled': hits(recalled_sources), 'architecture_recalled': hits(support_sources),
                  'recall_items': len(packet['items'])}
        assert result['recall_recalled'] == result['required_snippets'], result
        assert result['architecture_recalled'] == result['required_snippets'], result
        recall['questions'].append(result)
    recall['limitations'] = ['Two labeled fixture questions; no real-repository recall or precision claim.',
                             'Architecture uses named structure queries and source proofs; recall uses lexical search over retained text.',
                             'No remote model or paid embedding.']
    (run / 'recall-comparison.json').write_text(json.dumps(recall, indent=2) + '\n')
    command('rebuild', 'rebuild', '--view', 'changed')
    rebuilt = command('rebuilt-dependencies', 'dependencies', '--view', 'changed')
    assert sorted(rebuilt['details'].values(), key=lambda d: json.dumps(d, sort_keys=True)) == sorted(dependencies['details'].values(), key=lambda d: json.dumps(d, sort_keys=True))
    command('recover', 'recover')
    mcp_input = '\n'.join(json.dumps(r) for r in [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': '2025-03-26'}},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {'name': 'architecture_policy', 'arguments': {'view': 'changed'}}},
    ]) + '\n'
    mcp = subprocess.run([str(binary), '--db', str(db), 'mcp'], input=mcp_input, capture_output=True, text=True, check=True, timeout=30)
    responses = [json.loads(line) for line in mcp.stdout.splitlines()]
    assert responses[1]['result']['structuredContent']['response']['results'][0]['conclusion'] == 'violation'
    (run / 'mcp.json').write_text(json.dumps(responses, indent=2) + '\n')
    # Read the committed ledger, then exercise the public correction boundary.
    with sqlite3.connect(f'file:{db}?mode=ro', uri=True) as sql:
        bundle = json.loads(sql.execute("SELECT bundle FROM views WHERE id=?", (changed['view_id'],)).fetchone()[0])
    keys = {e['id']: e['canonical_key'] for e in bundle['entity_versions']}
    reverse = next(a for a in bundle['assertions'] if a['proposition']['predicate'] == 'IMPORTS' and keys[a['proposition']['subject_id']] == 'payments' and keys[a['proposition']['object']['entity_version_id']] == 'orders')
    command('retract', 'retract', reverse['id'], '--reason', 'demo correction')
    command('rebuild-after-retraction', 'rebuild', '--view', 'changed')
    assert command('corrected-policy', 'policy', '--view', 'changed')['response']['results'][0]['conclusion'] == 'unknown'
    history = command('historical-explanation', 'explain', handle)
    assert any(a['currently_retracted'] for a in history['support'])
    historical_dependencies = command('historical-dependencies', 'dependencies', '--view', 'changed', '--historical')
    assert any(d.get('from') == 'payments' and d.get('to') == 'orders' for d in historical_dependencies['details'].values())
    assessment = {'fixture': {'files': base['diagnostics']['coverage']['attempted'], 'assertions': base['assertion_count'], 'resolution_complete': True, 'database_bytes_after_workflow': db.stat().st_size}}
    if args.workload_repo:
        scan = command('workload-scan', 'scan', args.workload_repo, '--repository-id', 'workload', '--label', 'workload', '--analyzer', analyzer, database=run / 'workload.db')
        answer = command('workload-dependencies', 'dependencies', '--view', 'workload', database=run / 'workload.db')
        assessment['workload_repo'] = {'source_root': str(args.workload_repo.resolve()), 'coverage': scan['diagnostics']['coverage'], 'assertions': scan['assertion_count'], 'outcome': answer['response']['outcome'], 'result_count': len(answer['response']['results']), 'database_bytes': (run / 'workload.db').stat().st_size, 'limitation_count': len(scan['diagnostics']['limitations'])}
    if args.large_modules:
        count = args.large_modules
        if not 2 <= count <= 256:
            raise ValueError('--large-modules must be 2..256')
        workload = run / 'large-source'
        (workload / 'src').mkdir(parents=True)
        (workload / 'Cargo.toml').write_text('[package]\nname="briefcase-large"\nversion="0.1.0"\nedition="2021"\n')
        (workload / 'src/lib.rs').write_text(''.join(f'pub mod m{i:03};\n' for i in range(count)))
        for i in range(count):
            (workload / f'src/m{i:03}.rs').write_text(f'use crate::m{(i+1)%count:03}::entry as next;\npub fn entry() -> u32 {{ 1 }}\n')
        scan = command('large-scan', 'scan', workload, '--repository-id', 'large-workload', '--label', 'large', '--analyzer', analyzer, database=run / 'large.db')
        assert scan['diagnostics']['coverage']['resolution_complete'] is True
        answer = command('large-dependencies', 'dependencies', '--view', 'large', '--limit', '1000', database=run / 'large.db')
        assert answer['response']['outcome'] == 'complete'
        pairs = {(d['from'], d['to']) for d in answer['details'].values() if d.get('predicate') == 'DEPENDS_ON'}
        assert pairs == {(f'm{i:03}', f'm{(i+1)%count:03}') for i in range(count)}
        assert len([d for d in answer['details'].values() if d.get('predicate') == 'IN_CYCLE']) == count
        handle = next(r['explanation_handle'] for r in answer['response']['results']
                      if answer['details'][r['id']].get('predicate') == 'IN_CYCLE')
        proof = command('large-explain', 'explain', handle, '--limit', '10', database=run / 'large.db')
        assert len(proof['support']) <= 10 and proof['omitted_support_count'] > 0
        assessment['largest_supported_fixture'] = {'modules': count, 'files': scan['diagnostics']['coverage']['attempted'], 'assertions': scan['assertion_count'], 'resolution_complete': True, 'dependency_rows': len(answer['response']['results']), 'database_bytes': (run / 'large.db').stat().st_size}
    budgets = {'scan_seconds': 90, 'query_seconds': 20, 'peak_rss_kib': 262144,
               'database_bytes_per_measured_workload': 67108864,
               'scope': 'Local debug-build regression ceilings on these evaluated workloads; not percentile SLAs or an arbitrary-repository guarantee.'}
    for measure in measures:
        ceiling = budgets['scan_seconds'] if measure['operation'] in ('base', 'changed', 'workload-scan', 'large-scan') else budgets['query_seconds']
        assert measure['elapsed_seconds'] <= ceiling, measure
        assert measure.get('peak_rss_kib', 0) <= budgets['peak_rss_kib'], measure
    for workload in assessment.values():
        assert workload.get('database_bytes', workload.get('database_bytes_after_workflow', 0)) <= budgets['database_bytes_per_measured_workload'], workload
    report = {'status': 'passed', 'scope': 'Actual Rust product workflow and storage, independent authored gold; selected Rust imports only', 'binary_digest': binary_digest, 'artifact_directory': str(run), 'database': str(db), 'measures': measures, 'workloads': assessment, 'recall': recall, 'budgets': budgets, 'verified': ['native imports', 'two-revision comparison', 'cycles', 'policy pass/violation/unknown', 'retained source proof', 'CLI/MCP agreement', 'projection rebuild', 'current retraction closure', 'historical explanation and dependencies', 'labeled source recall comparison'], 'unit_test_gates': 'scripts/verify-mvp additionally runs batch/replay/kill recovery, schemas and inherited recall regressions'}
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
