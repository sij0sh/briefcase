// Run with: node --test tests/briefcase-pi.test.mjs (after `cargo build`).
// Loads the installed (rendered) extension with a mock Pi API.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { EventEmitter } from 'node:events';
import * as childProcess from 'node:child_process';
import { stripTypeScriptTypes } from 'node:module';

const dataUrl = source => 'data:text/javascript;base64,' + Buffer.from(source).toString('base64');
const binary = process.env.BRIEFCASE_TEST_BINARY
  ?? new URL('../target/debug/briefcase', import.meta.url).pathname;
const git = process.env.PATH.split(path.delimiter).map(p => path.join(p, 'git')).find(p => fs.existsSync(p));
const TOKEN = 'zanzibarquux';

function environment(t, values) {
  const previous = Object.fromEntries(Object.keys(values).map(key => [key, process.env[key]]));
  for (const [key, value] of Object.entries(values)) {
    if (value === undefined) delete process.env[key]; else process.env[key] = value;
  }
  t.after(() => {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
  });
}

async function fixture(t, { real = false } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'briefcase-pi-test-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const cwd = path.join(root, 'repo'), agentDir = path.join(root, 'agent'), emptyPath = path.join(root, 'bin');
  for (const dir of [path.join(cwd, '.git'), path.join(cwd, 'src'), emptyPath]) fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(cwd, 'Cargo.toml'), '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n');
  fs.writeFileSync(path.join(cwd, 'src/lib.rs'), 'pub fn entry() {}\n');
  fs.writeFileSync(path.join(cwd, 'README.md'), `# Demo\n\nThe ${TOKEN} credential rotates nightly.\n`);
  // No rust-analyzer on PATH: the automatic profile is a syntax-only capture.
  if (git) {
    fs.symlinkSync(git, path.join(emptyPath, 'git'));
    assert.equal(childProcess.spawnSync(git, ['init', '-q'], { cwd }).status, 0);
  }
  environment(t, { BRIEFCASE_DB: undefined, BRIEFCASE_SYNC: undefined, BRIEFCASE_SYNC_TIMEOUT: undefined,
    BRIEFCASE_BIN: undefined, PI_CODING_AGENT_DIR: agentDir,
    CODEX_HOME: path.join(root, 'codex'), CLAUDE_CONFIG_DIR: path.join(root, 'claude'), MUSE_HOME: path.join(root, 'muse'), PATH: emptyPath });

  const install = childProcess.spawnSync(binary, ['install', '--agent', 'pi', '--agent-dir', agentDir], { encoding: 'utf8' });
  assert.equal(install.status, 0, install.stderr);
  assert.equal(JSON.parse(install.stdout).status, 'wired');
  const extension = path.join(agentDir, 'extensions', 'briefcase-pi.ts');

  const launches = [], statuses = [], children = [];
  const spawn = (program, args, options) => {
    assert.equal(program, process.env.BRIEFCASE_BIN || fs.realpathSync(binary));
    launches.push({ program, args, options });
    if (real) {
      const child = childProcess.spawn(program, args, options);
      let output = '';
      child.stdout.on('data', data => { output += data; });
      child.on('close', () => { try { const pid = JSON.parse(output).pid; if (pid) t.after(() => { try { process.kill(pid); } catch {} }); } catch {} });
      return child;
    }
    const child = new EventEmitter();
    child.stdout = new EventEmitter(); child.stderr = new EventEmitter();
    child.unref = () => {}; child.kill = () => {};
    children.push(child);
    return child;
  };
  const key = `briefcasePiTest${Math.random().toString().slice(2)}`;
  globalThis[key] = spawn; t.after(() => delete globalThis[key]);
  const schema = dataUrl('export const Type = new Proxy({}, { get: (_target, name) => (...args) => ({ name, args }) });');
  const source = stripTypeScriptTypes(fs.readFileSync(extension, 'utf8'))
    .replace('from "node:child_process"', `from ${JSON.stringify(dataUrl(`export const spawn = globalThis[${JSON.stringify(key)}];`))}`)
    .replace('from "typebox"', `from ${JSON.stringify(schema)}`);
  const module = await import(dataUrl(source));
  const events = new Map(), tools = new Map(), commands = new Map();
  const execs = [], notifications = [];
  let execResult;
  module.default({
    registerTool: value => tools.set(value.name, value),
    registerCommand: (name, value) => commands.set(name, value),
    on: (name, handler) => events.set(name, handler),
    exec: async (program, args, options) => {
      execs.push(args);
      if (execResult) return execResult;
      const result = childProcess.spawnSync(program, args, { cwd: options.cwd, encoding: 'utf8', timeout: options.timeout });
      return { stdout: result.stdout, stderr: result.stderr, code: result.status, killed: result.signal !== null };
    },
  });
  const ctx = { cwd, hasUI: true, ui: { setStatus: (_key, text) => statuses.push(text),
    notify: (text, severity) => notifications.push({ text, severity }) } };
  return {
    cwd, agentDir, extension, launches, children, statuses, tools, commands, execs, notifications,
    setExecResult: result => { execResult = result; },
    command: name => commands.get('briefcase').handler(name, ctx),
    start: (reason = 'startup') => events.get('session_start')({ reason }, ctx),
    toolResult: (toolName, isError = false) => events.get('tool_result')?.({ toolName, isError }, ctx),
    settled: () => events.get('agent_settled')?.({}, ctx),
    shutdown: () => events.get('session_shutdown')({}, ctx),
    call: (name, params) => tools.get(name).execute('test', params, undefined, undefined, ctx),
    finish: (child, outcome, code = 0) => { if (outcome) child.stdout.emit('data', JSON.stringify(outcome)); child.emit('close', code); },
  };
}

test('factory registers two compact tools without starting processes', async t => {
  const f = await fixture(t);
  assert.deepEqual([...f.tools.keys()].sort(), ['briefcase', 'context']);
  assert.equal(f.tools.get('briefcase').label, 'Briefcase');
  assert.equal(f.launches.length, 0);
  const description = f.tools.get('briefcase').description;
  assert.match(description, /operation describe/);
  assert.ok(description.length < 600);
  assert.ok(!description.includes('Operations:'));
  assert.match(f.tools.get('context').description, /Git commits/);
  assert.match(f.tools.get('context').description, /agent sessions are searched only when/);
  assert.match(fs.readFileSync(f.extension, 'utf8'), /briefcase-pi-digest: sha256:/);
  const guidance = f.tools.get('briefcase').promptGuidelines.join('\n');
  assert.match(guidance, /brief/);
  assert.match(guidance, /page/);
  assert.match(f.tools.get('context').promptGuidelines.join('\n'), /operation brief/);
});

test('every session start ensures the monitor; BRIEFCASE_SYNC=0 disables bootstrap', async t => {
  const f = await fixture(t);
  await f.start('reload');
  assert.equal(f.launches.length, 1);
  f.finish(f.children[0], { monitor: 'already-running' });
  await f.start();
  assert.deepEqual(f.launches[1].args, ['--json', 'monitor', 'ensure', '--timeout', '120']);
  assert.equal(f.launches[1].options.cwd, f.cwd);
  assert.equal(f.launches[1].options.detached, true);
  assert.equal(f.statuses.at(-1), 'Briefcase: starting monitor');
  f.finish(f.children[1], { monitor: 'started' });
  assert.equal(f.statuses.at(-1), 'Briefcase: monitoring');
  process.env.BRIEFCASE_SYNC = '0';
  await f.start('new');
  assert.equal(f.launches.length, 2);
});

test('tool results and settled turns never decide freshness or import sessions', async t => {
  const f = await fixture(t);
  await f.start();
  f.finish(f.children[0], { monitor: 'started' });
  for (const tool of ['edit', 'write', 'bash', 'powershell', 'new-editor']) {
    await f.toolResult(tool); await f.toolResult(tool, true); await f.settled();
  }
  assert.equal(f.launches.length, 1);
  assert.ok(!f.launches.some(l => l.args.includes('sync-request') || l.args.includes('import-local')));
});

test('BRIEFCASE_BIN overrides the installed CLI binary', async t => {
  const f = await fixture(t);
  const override = path.join(f.agentDir, 'custom-briefcase');
  process.env.BRIEFCASE_BIN = override;
  await f.start();
  assert.equal(f.launches[0].program, override);
  f.finish(f.children[0], { monitor: 'already-running' });
});

test('monitor failure is reported and another session can retry bootstrap', async t => {
  const f = await fixture(t);
  await f.start();
  f.finish(f.children[0], null, 1);
  assert.equal(f.statuses.at(-1), 'Briefcase: monitor failed');
  await f.start('new');
  assert.equal(f.launches.length, 2);
  f.finish(f.children[1], { monitor: 'already-running' });
  assert.equal(f.statuses.at(-1), 'Briefcase: monitoring');
});

test('manual sync preserves structured nonzero outcomes and uses their severity', async t => {
  const f = await fixture(t);
  for (const [sync, severity, code] of [
    ['needs_configuration', 'warning', 3], ['locked', 'warning', 3],
    ['failed', 'error', 1], ['timeout', 'error', 3], ['refreshed', 'info', 0],
  ]) {
    const diagnostic = `tsconfig.json: ${sync}`;
    f.setExecResult({ code, stdout: JSON.stringify({ sync, [sync === 'failed' ? 'error' : 'message']: diagnostic }),
      stderr: '', killed: false });
    await f.command('sync');
    assert.equal(f.notifications.at(-1).severity, severity);
    assert.match(f.notifications.at(-1).text, new RegExp(diagnostic));
    assert.match(f.notifications.at(-1).text, new RegExp(`sync: ${sync}`));
  }
});

test('query failures retain JSON diagnostics, stderr fallback and cancellation', async t => {
  const f = await fixture(t);
  for (const [result, expected] of [
    [{ code: 1, stdout: JSON.stringify({ error: 'structured query failure' }), stderr: '' }, /structured query failure/],
    [{ code: 1, stdout: JSON.stringify({ message: 'structured message' }), stderr: 'less useful' }, /structured message/],
    [{ code: 1, stdout: JSON.stringify({ sync: 'needs_configuration', message: 'query remains a failure' }), stderr: '' }, /query remains a failure/],
    [{ code: 1, stdout: 'invalid', stderr: 'legacy stderr' }, /legacy stderr/],
    [{ code: 7, stdout: '', stderr: '' }, /exited with code 7/],
    [{ code: 0, stdout: 'invalid', stderr: '' }, /not JSON/],
    [{ code: 1, stdout: JSON.stringify({ message: 'ignored after cancellation' }), stderr: '', killed: true }, /cancelled or exceeded/],
  ]) {
    f.setExecResult({ killed: false, ...result });
    await assert.rejects(f.call('briefcase', { operation: 'status' }), expected);
  }
  f.setExecResult({ code: 1, stdout: '', stderr: 'legacy sync error', killed: false });
  await f.command('sync');
  assert.deepEqual(f.notifications.at(-1), { text: 'legacy sync error', severity: 'error' });
});

test('manual status shows failed sync diagnostics alongside a readable publication', async t => {
  const f = await fixture(t);
  f.setExecResult({ code: 0, killed: false, stderr: '', stdout: JSON.stringify({
    readiness: { state: 'ready', last_sync: { sync: 'needs_configuration', message: 'tsconfig.json is missing' } },
  }) });
  await f.command('status');
  assert.deepEqual(f.notifications.at(-1), {
    text: 'Briefcase status: ready; last sync: needs_configuration (tsconfig.json is missing)', severity: 'warning',
  });
  // Background jobs already parse JSON on nonzero exits and keep a concise status.
  await f.start();
  f.finish(f.children[0], { monitor: 'already-running' });
  assert.equal(f.statuses.at(-1), 'Briefcase: monitoring');
});

test('real sync, context recall and trace operations through the CLI', async t => {
  const f = await fixture(t, { real: true });
  await f.start();
  const deadline = Date.now() + 60_000;
  while (!fs.existsSync(path.join(f.cwd, '.briefcase', 'briefcase.db.sync.json')) && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  while (Date.now() < deadline) {
    const result = childProcess.spawnSync(binary, ['--json', 'status'], { cwd: f.cwd, encoding: 'utf8' });
    if (result.status === 0 && JSON.parse(result.stdout).readiness?.state === 'ready') break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.ok(fs.existsSync(path.join(f.cwd, '.briefcase', 'briefcase.db')));

  const recalled = await f.call('context', { query: TOKEN });
  const text = recalled.content[0].text;
  assert.match(text, /--- README\.md:1-3 \(documentation, rank 1\)/);
  assert.match(text, new RegExp(TOKEN));
  assert.match(text, /untrusted/);
  assert.deepEqual(recalled.details.items[0], { path: 'README.md', lines: { start: 1, end: 3 } });
  assert.ok(f.execs.at(-1).includes('--wait-ready'));

  const status = await f.call('briefcase', { operation: 'status' });
  assert.match(status.content[0].text, /"current_view"/);
  const discovered = await f.call('briefcase', { operation: 'describe', arguments: { operation: 'reachability' } });
  assert.match(discovered.content[0].text, /"required"/);
  assert.match(discovered.content[0].text, /"from"/);
  await assert.rejects(f.call('briefcase', { operation: 'scan', arguments: {} }), /UNKNOWN_OPERATION/);
  await assert.rejects(f.call('briefcase', { operation: 'reachability', arguments: { from: 'a' } }), /to is required/);
  await assert.rejects(f.call('briefcase', { operation: 'dependencies', arguments: { sql: 'DROP TABLE records' } }), /Unknown argument sql/);

  const brief = await f.call('briefcase', { operation: 'brief', arguments: {task: 'review src/lib.rs', target: 'src/lib.rs', max_bytes: 16000} });
  const packet = JSON.parse(brief.content[0].text);
  assert.equal(packet.kind, 'brief');
  await assert.rejects(f.call('briefcase', {operation: 'map'}), /UNKNOWN_OPERATION/);
});

test('a database-less repository captures text on its first search', async t => {
  const f = await fixture(t);
  const discovered = await f.call('briefcase', { operation: 'describe', arguments: { query: 'what calls this function' } });
  assert.match(discovered.content[0].text, /neighbors/);
  const result = await f.call('context', {query: TOKEN});
  assert.match(result.content[0].text, new RegExp(TOKEN));
  assert.ok(fs.existsSync(path.join(f.cwd, '.briefcase', 'briefcase.db')));
});

test('recall formatting labels commit excerpts by hash and file', async t => {
  const f = await fixture(t);
  const source = stripTypeScriptTypes(fs.readFileSync(f.extension, 'utf8'))
    .replace('from "node:child_process"', `from ${JSON.stringify(dataUrl('export const spawn = () => {};'))}`)
    .replace('from "typebox"', `from ${JSON.stringify(dataUrl('export const Type = new Proxy({}, { get: () => () => ({}) });'))}`);
  const { formatSearch } = await import(dataUrl(source));
  const text = formatSearch({ view_id: 'view:1', snapshot: { id: 's', capture_state: 'complete' },
    coverage: { not_included: ['agent session history'] },
    items: [{ rank: 1, source_kind: 'commit', commit: '0123456789abcdef0123', committed_at: '2026-01-01T00:00:00Z',
              file: 'src/store.rs', path: 'git:0123456789abcdef0123', text: 'commit 01234567 Flush\n' }] });
  assert.match(text, /--- commit 0123456789ab 2026-01-01T00:00:00Z src\/store\.rs \(commit, rank 1\)/);
  assert.match(text, /file and commit text/);
});

test('oversized structured output fails closed with a valid JSON notice', async t => {
  const f = await fixture(t);
  const source = stripTypeScriptTypes(fs.readFileSync(f.extension, 'utf8'))
    .replace('from "node:child_process"', `from ${JSON.stringify(dataUrl('export const spawn = () => {};'))}`)
    .replace('from "typebox"', `from ${JSON.stringify(dataUrl('export const Type = new Proxy({}, { get: () => () => ({}) });'))}`);
  const { formatOperation } = await import(dataUrl(source));
  const oversized = {
    response: { continuation_token: 'report:abc@5', results: ['x'.repeat(70_000)] },
    details: {},
  };
  const text = formatOperation(oversized);
  assert.ok(text.length < 60_000, `notice must stay bounded, got ${text.length}`);
  const notice = JSON.parse(text);
  assert.equal(notice.truncated, true);
  assert.equal(notice.continuation_token, 'report:abc@5');
  assert.match(notice.instruction, /continue/);
  assert.ok(notice.total_bytes > 70_000);
});

test('recall formatting labels session hits and stale freshness', async t => {
  const f = await fixture(t);
  const source = stripTypeScriptTypes(fs.readFileSync(f.extension, 'utf8'))
    .replace('from "node:child_process"', `from ${JSON.stringify(dataUrl('export const spawn = () => {};'))}`)
    .replace('from "typebox"', `from ${JSON.stringify(dataUrl('export const Type = new Proxy({}, { get: () => () => ({}) });'))}`);
  const { formatSearch, isPendingFreshness } = await import(dataUrl(source));
  const text = formatSearch({ view_id: 'view:2', snapshot: { id: 's', capture_state: 'complete' },
    freshness: { view_id: 'view:2', stale: true, pending_or_interrupted: false },
    coverage: { not_included: ['files outside the capture'], terms: { matched: ['payments'], unmatched: ['zqxwkv'] } },
    items: [{ rank: 1, source_kind: 'session', session: 'view:9', timestamp: '2026-10-12T10:00:02Z',
              harness: { name: 'pi' }, path: 'session:view:9', text: 'Fix the payments rounding bug.' }] });
  assert.match(text, /--- session view:9 2026-10-12T10:00:02Z \(pi\) \(session, rank 1\)/);
  assert.match(text, /Freshness: this view may lag current files/);
  assert.match(text, /No retained match for: zqxwkv/);
  assert.equal(isPendingFreshness('error: FRESHNESS: A newer scan is pending/interrupted; select a prior view'), true);
  assert.equal(isPendingFreshness('error: ARGUMENTS: Unknown argument sql'), false);
});

test('/briefcase import retains local Pi sessions through the CLI', async t => {
  const f = await fixture(t);
  await f.commands.get('briefcase').handler('import', { cwd: f.cwd, hasUI: true,
    ui: { setStatus: () => {}, notify: () => {} } });
  const argv = f.execs.at(-1);
  assert.ok(argv.includes('import-local'), JSON.stringify(argv));
  assert.ok(argv.includes('cited'), JSON.stringify(argv));
});

test('all Pi presentation includes headers, metadata, warnings and UTF-8 in the cap', async t => {
  const f = await fixture(t);
  const source = stripTypeScriptTypes(fs.readFileSync(f.extension, 'utf8'))
    .replace('from "node:child_process"', `from ${JSON.stringify(dataUrl('export const spawn = () => {};'))}`)
    .replace('from "typebox"', `from ${JSON.stringify(dataUrl('export const Type = new Proxy({}, { get: () => () => ({}) });'))}`);
  const { formatOperation, formatSearch, presentToolResult } = await import(dataUrl(source));
  const freshness = { captured_at: '2026-10-13T00:00:00Z', age_seconds: 30, pending_or_interrupted: true,
    fallback: { used: true, pinned_view: 'view:64' }, dirty_hit_paths: { state: 'checked', paths: ['src/é.rs'], overlap_count: 1 } };
  const recall = formatSearch({ view_id: 'view:64', snapshot: { id: 'snapshot:1', capture_state: 'complete' }, freshness,
    items: [{ path: 'src/é.rs', source_kind: 'code', line_range: { start: 1, end: 2 }, text: 'é😀'.repeat(10000) }] }, 5500);
  assert.ok(Buffer.byteLength(recall, 'utf8') <= 5500);
  assert.match(recall, /age 30s/);
  assert.match(recall, /published view:64/);
  assert.match(recall, /src\/é.rs/);
  assert.match(recall, /omitted/);
  assert.ok(!recall.includes('\ufffd'));
  const report = formatOperation({ report_id: 'context-report:1', format: 'llm', outcome: 'partial', text: 'é😀'.repeat(10000), freshness }, 5500);
  assert.ok(Buffer.byteLength(report, 'utf8') <= 5500);
  assert.equal(JSON.parse(report).freshness.fallback.used, true);
  const notice = JSON.parse(formatOperation({ pinned_views: ['view:64'], response: { id: 'report:1', results: [{ explanation_handle: 'explain:report:1:0' }] },
    metadata: '😀'.repeat(10000), freshness }, 5500));
  assert.equal(notice.report_id, 'report:1');
  assert.equal(notice.explanation_handle, 'explain:report:1:0');
  assert.deepEqual(notice.pinned_views, ['view:64']);
  const wrapped = presentToolResult({ pinned_views: ['view:64'], response: { id: 'report:1', results: [] }, text: 'é"\\😀'.repeat(10000) },
    'operation', { operation: 'retrieve', metadata: 'x'.repeat(1000) }, 5500);
  assert.ok(Buffer.byteLength(JSON.stringify(wrapped),'utf8') <= 5500);
  assert.equal(wrapped.details.response_budget.tool_response_bytes, Buffer.byteLength(JSON.stringify(wrapped),'utf8'));
  assert.ok(JSON.parse(wrapped.content[0].text));
});

test('Pi delegates fallback to the Rust catalog without injecting allow_partial', async t => {
  const f = await fixture(t);
  childProcess.spawnSync(binary, ['--db', path.join(f.cwd,'.briefcase/briefcase.db'), 'scan', f.cwd, '--syntax-only'], { encoding: 'utf8' });
  const failed = childProcess.spawnSync(binary, ['--db', path.join(f.cwd,'.briefcase/briefcase.db'), 'scan', f.cwd, '--analyzer', path.join(f.cwd,'missing-analyzer')], { encoding: 'utf8' });
  assert.notEqual(failed.status, 0);
  const answer = await f.call('briefcase', { operation: 'retrieve', arguments: { text: 'entry', max_bytes: 5500 } });
  const result = JSON.parse(answer.content[0].text);
  assert.equal(result.freshness.fallback.used, true);
  assert.equal(result.freshness.fallback.argument, 'view');
  assert.equal(result.freshness.fallback.pinned_view, 'view:1');
  assert.ok(Buffer.byteLength(JSON.stringify(answer),'utf8') <= 5500);
  assert.ok(f.execs.at(-1).includes('--fallback-published'));
  assert.ok(!JSON.parse(f.execs.at(-1)[f.execs.at(-1).indexOf('--args')+1]).allow_partial);
  await assert.rejects(f.call('briefcase', { operation: 'retrieve', arguments: { text: 'entry', sql: 'DROP' } }), /Unknown argument sql/);
});
