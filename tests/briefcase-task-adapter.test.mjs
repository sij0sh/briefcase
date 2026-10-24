// Pure adapter tests. No installer, binary, optional Typebox package or toolchain.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';

const dataUrl = text => 'data:text/javascript;base64,' + Buffer.from(text).toString('base64');
const schema = dataUrl('export const Type = new Proxy({}, {get: (_, name) => (...args) => ({name, args})});');
const source = stripTypeScriptTypes(readFileSync(new URL('../extensions/briefcase-pi.ts', import.meta.url), 'utf8'))
  .replace('from "typebox"', `from ${JSON.stringify(schema)}`);
const adapter = await import(dataUrl(source));

function reviewContext(t) {
  const previous = process.env.BRIEFCASE_REVIEW_CONTEXT;
  process.env.BRIEFCASE_REVIEW_CONTEXT = '1';
  t.after(() => {
    if (previous === undefined) delete process.env.BRIEFCASE_REVIEW_CONTEXT;
    else process.env.BRIEFCASE_REVIEW_CONTEXT = previous;
  });
}

test('legacy oversized responses retain an executable restart, including source bytes', () => {
  const legacy = JSON.parse(adapter.formatOperation({response: {
    continuation_token: 'report:abc@5', results: ['x'.repeat(70000)]}}, 2000));
  assert.deepEqual(legacy.continue_with, {operation:'page', arguments:{token:'report:abc@0'}});
  assert.match(legacy.instruction, /continue/);
  const result = JSON.parse(adapter.formatOperation({kind:'explain',report_id:'task-case:a',ref:'/candidates/40',
    source_offset:4000,text:'é'.repeat(10000),next_source_offset:24000},1800));
  assert.deepEqual(result.continue_with,{operation:'explain',arguments:{report_id:'task-case:a',ref:'/candidates/40',source_offset:4000}});
});

test('coding duties are exposed in agent guidance without adding tool families', () => {
  const tools = [];
  adapter.default({registerTool: t => tools.push(t), registerCommand() {}, on() {}});
  assert.deepEqual(tools.map(t => t.name), ['context', 'briefcase']);
  const tool = tools.find(t => t.name === 'briefcase');
  assert.match(tool.description, /brief/);
  assert.match(tool.promptGuidelines.join(' '), /not_covered/);
  assert.match(tool.promptGuidelines.join(' '), /occurrences/);
});

test('oversized literal pages keep pins and rewind the delivery cursor', () => {
  const result = {kind: 'occurrences', view_id: 'view:1', page: {offset: 100, returned: 20, total: 150},
    items: Array.from({length: 20}, (_, i) => ({start_byte: i, detail: 'é'.repeat(1000)})),
    continue_with: {operation: 'occurrences', arguments: {view: 'view:1', text: 'needle', offset: 120}}};
  const encoded = adapter.formatOperation(result, 1400);
  assert.ok(Buffer.byteLength(encoded) <= 1400);
  const notice = JSON.parse(encoded);
  assert.equal(notice.view_id, 'view:1');
  assert.equal(notice.page.returned, 0);
  assert.equal(notice.page.next_offset, 100);
  assert.equal(notice.continue_with.arguments.offset, 100);
  assert.match(notice.instruction, /No result rows/);
});

test('oversized briefs retain a task report ID and a section continuation', () => {
  const result = {kind: 'brief', id: 'task-brief:abc', view_id: 'view:4',
    slice: {test: ['x'.repeat(10000)]},
    continue_with: {operation: 'page', arguments: {report_id: 'task-brief:abc', section: '/slice/test', offset: 10}}};
  const notice = JSON.parse(adapter.formatOperation(result, 1600));
  assert.equal(notice.report_id, 'task-brief:abc');
  assert.equal(notice.map_id, undefined);
  assert.equal(notice.continue_with.operation, 'page');
  assert.equal(notice.continue_with.arguments.offset, 0);
});

test('degraded task reads skip waits for unrelated semantic work', async () => {
  const commands = [];
  const pi = {exec: async (_program, args) => {
    commands.push(args); return {code: 0, stdout: JSON.stringify({kind: 'occurrences'}), stderr: ''};
  }};
  await adapter.execRead(pi, 'occurrences', {text: 'needle'}, '/work');
  await adapter.execRead(pi, 'policy', {component: 'demo'}, '/work');
  assert.equal(commands[0][commands[0].indexOf('--wait-ready') + 1], '0');
  assert.equal(commands[1][commands[1].indexOf('--wait-ready') + 1], '0');
});

test('the complete adapter envelope stays within its UTF-8 budget', () => {
  const result = {kind: 'neighbors', view_id: 'view:1', page: {offset: 0, total: 50},
    items: Array.from({length: 50}, () => ({from: 'é'.repeat(100), to: '目标'.repeat(100)})),
    continue_with: {operation: 'neighbors', arguments: {target: 'a', offset: 50, view: 'view:1'}}};
  const response = adapter.presentToolResult(result, 'operation', {operation: 'neighbors'}, 2048);
  assert.ok(Buffer.byteLength(JSON.stringify(response)) <= 2048);
  const content = JSON.parse(response.content[0].text);
  assert.equal(content.page.next_offset, 0);
});

test('opaque report cursors rewind to the first undelivered row', () => {
  const result = {kind:'page',report_id:'context-report:abc',section:'/items',page:{offset:11,returned:20,total:50},
    items:Array.from({length:20},()=>({text:'é'.repeat(3000)})),
    continue_with:{operation:'page',arguments:{report_id:'context-report:abc',cursor:JSON.stringify({report_id:'context-report:abc',section:'/items',offset:31})}}};
  const notice=JSON.parse(adapter.formatOperation(result,1800));
  assert.equal(notice.continue_with.operation,'page');
  assert.equal(notice.continue_with.arguments.offset,11);
  assert.equal(notice.continue_with.arguments.section,'/items');
  assert.equal(notice.continue_with.arguments.cursor,undefined);
});

test('prompt context and post-task checks never submit another agent prompt', async () => {
  const events=new Map(), calls=[], entries=[], notices=[];
  const pi={registerTool(){},registerCommand(){},on:(name,handler)=>events.set(name,handler),
    appendEntry:(type,value)=>entries.push({type,value}),
    sendUserMessage(){throw new Error('must not continue the agent');},
    exec:async (_program,args)=>{calls.push(args);return {code:0,stdout:JSON.stringify({kind:'brief',id:'task-brief:1',report_id:'task-brief:1',coverage:{not_covered_areas:['static_calls']}}),stderr:''};}};
  adapter.default(pi);
  const ctx={cwd:'/repo',ui:{notify:(...args)=>notices.push(args)}};
  const result=await events.get('before_agent_start')({prompt:'review src/store.rs'},ctx);
  assert.equal(result.message.customType,'briefcase-context');
  assert.equal(result.message.display,false);
  assert.match(result.message.content,/untrusted data/);
  assert.equal(result.systemPrompt,undefined);
  await events.get('agent_end')({},ctx);
  assert.equal(entries[0].type,'briefcase-completeness');
  assert.equal(entries[0].value.prompt_report_id,'task-brief:1');
  assert.deepEqual(entries[0].value.not_covered_areas,['static_calls']);
  assert.equal(calls.length,1);
  assert.equal(calls[0][calls[0].indexOf('call')+1], 'agent_context');
  assert.equal(JSON.parse(calls[0][calls[0].indexOf('--args')+1]).limit, undefined);
  assert.equal(entries[0].value.dimension_comparison,'not_requested');
  await events.get('agent_end')({},ctx);
  assert.equal(calls.length,1,'a settled turn must not check again');
});

test('automatic context failures are non-blocking and can be disabled', async t => {
  const events=new Map(), notices=[];let calls=0;
  adapter.default({registerTool(){},registerCommand(){},on:(name,handler)=>events.set(name,handler),
    exec:async()=>{calls++;return {code:1,stdout:'',stderr:'provider unavailable'};}});
  const ctx={cwd:'/repo',ui:{notify:(...args)=>notices.push(args)}};
  assert.equal(await events.get('before_agent_start')({prompt:'debug storage'},ctx),undefined);
  assert.equal(calls,1);assert.match(notices[0][0],/context unavailable/);
  const previous=process.env.BRIEFCASE_CONTEXT;t.after(()=>{if(previous===undefined)delete process.env.BRIEFCASE_CONTEXT;else process.env.BRIEFCASE_CONTEXT=previous;});
  process.env.BRIEFCASE_CONTEXT='0';
  await events.get('before_agent_start')({prompt:'debug storage'},ctx);
  assert.equal(calls,1);
});


test('truncated search text keeps a restart cursor in its header', () => {
  const text=adapter.formatSearch({kind:'search',report_id:'context-report:search',view_id:'view:1',
    page:{offset:0,returned:2,total:3},items:[{path:'src/a.rs',source_kind:'code',rank:1,line_range:{start:1,end:2},text:'é'.repeat(10000)}],
    continue_with:{operation:'page',arguments:{report_id:'context-report:search',cursor:JSON.stringify({report_id:'context-report:search',section:'/items',offset:2})}}},1800);
  assert.ok(Buffer.byteLength(text)<=1800);
  assert.match(text,/context-report:search/);
  assert.match(text,/"offset":0/);
  assert.match(text,/"operation":"page"/);
});

const completeness = (overrides = {}) => ({anchors:'established',slice:'established',neighborhood:'partial',
  history:'unknown',sessions:'unknown',gap_inventory:'established',verification:'not_required',...overrides});

test('case dimension comparisons distinguish unchanged, changed and unavailable states', () => {
  assert.deepEqual(adapter.compareCompleteness(completeness(), completeness()), {});
  assert.deepEqual(adapter.compareCompleteness(completeness(), completeness({history:'established',verification:'unknown'})), {
    history:{before:'unknown',after:'established'},verification:{before:'not_required',after:'unknown'},
  });
  assert.equal(adapter.compareCompleteness(undefined, completeness()), undefined);
  assert.equal(adapter.compareCompleteness({}, completeness()), undefined);
});

test('explicit post-task review compares identified cases and keeps dimension deltas', async t => {
  reviewContext(t);
  const events=new Map(), entries=[];let reads=0;
  adapter.default({registerTool(){},registerCommand(){},on:(name,handler)=>events.set(name,handler),
    appendEntry:(type,value)=>entries.push({type,value}),exec:async(_program,args)=>{
      if(args[args.indexOf('call')+1]==='worktree') {
        reads++;
        assert.equal(JSON.parse(args[args.indexOf('--args')+1]).case_id,'task-case:before');
        return {code:0,stdout:JSON.stringify({kind:'worktree',report_id:'context-report:changes',alignment:'changed',changed_paths:2,outside_initial_slice:1}),stderr:''};
      }
      const first=reads++===0;
      return {code:0,stdout:JSON.stringify({kind:'brief',id:first?'task-case:before':'task-case:after',
        case_id:first?'task-case:before':'task-case:after',coverage:{completeness:completeness(first?{}:{sessions:'partial'})}}),stderr:''};
    }});
  const ctx={cwd:'/repo',ui:{notify(){}}};
  await events.get('before_agent_start')({prompt:'debug src/a.rs'},ctx);
  await events.get('agent_end')({},ctx);
  assert.equal(entries[0].value.prompt_case_id,'task-case:before');
  assert.equal(entries[0].value.case_id,'task-case:after');
  assert.equal(entries[0].value.dimension_comparison,'compared');
  assert.deepEqual(entries[0].value.dimension_deltas,{sessions:{before:'unknown',after:'partial'}});
  assert.equal(reads,3);
  assert.equal(entries[0].value.worktree.outside_initial_slice,1);
});

test('explicit review can recover dimension states from retained case metadata', async t => {
  reviewContext(t);
  const events=new Map(), calls=[], entries=[];
  adapter.default({registerTool(){},registerCommand(){},on:(name,handler)=>events.set(name,handler),
    appendEntry:(type,value)=>entries.push(value),exec:async (_program,args)=>{
      const operation=args[args.indexOf('call')+1];calls.push(operation);
      const result=operation==='explain'?{kind:'explain',entry:completeness()}:
        {kind:'brief',truncated:true,id:'task-case:budget',case_id:'task-case:budget',report_id:'task-case:budget'};
      return {code:0,stdout:JSON.stringify(result),stderr:''};
    }});
  const ctx={cwd:'/repo',ui:{notify(){}}};
  await events.get('before_agent_start')({prompt:'debug src/a.rs'},ctx);
  await events.get('agent_end')({},ctx);
  assert.deepEqual(calls,['agent_context','explain','agent_context','explain','worktree']);
  assert.deepEqual(entries[0].dimension_deltas,{});
  assert.equal(entries[0].dimension_comparison,'compared');
});

test('oversized new briefs preserve their case ID', () => {
  const result={kind:'brief',case_id:'task-case:new',id:'task-case:new',report_id:'task-case:new',
    outcome:'unknown',outcome_scope:'retained_evidence',slice:{impl:[{text:'é'.repeat(10000)}]}};
  const notice=JSON.parse(adapter.formatOperation(result,1800));
  assert.equal(notice.case_id,'task-case:new');assert.equal(notice.report_id,'task-case:new');
  assert.equal(notice.outcome,'unknown');
  assert.equal(notice.outcome_scope,'retained_evidence');assert.equal(notice.continue_with.arguments.section,'/slice/impl');
});


test('oversized manifests rewind candidate delivery without dropping case pins', () => {
  const result={kind:'agent_context',case_id:'task-case:manifest',report_id:'task-case:manifest',
    files:[{path:'src/a.rs',detail:'é'.repeat(10000)}],
    continue_with:{operation:'page',arguments:{report_id:'task-case:manifest',cursor:JSON.stringify({report_id:'task-case:manifest',section:'/candidates',offset:9})}}};
  const notice=JSON.parse(adapter.formatOperation(result,1800));
  assert.equal(notice.case_id,'task-case:manifest');
  assert.equal(notice.continue_with.arguments.section,'/candidates');
  assert.equal(notice.continue_with.arguments.offset,0);
});

test('automatic context budgets are configurable and include the framing text', async t => {
  const previous=process.env.BRIEFCASE_CONTEXT_MAX_BYTES;
  t.after(()=>{if(previous===undefined)delete process.env.BRIEFCASE_CONTEXT_MAX_BYTES;else process.env.BRIEFCASE_CONTEXT_MAX_BYTES=previous;});
  process.env.BRIEFCASE_CONTEXT_MAX_BYTES='12000';assert.equal(adapter.contextBudget(),12000);
  const events=new Map(),calls=[];
  adapter.default({registerTool(){},registerCommand(){},on:(name,handler)=>events.set(name,handler),
    exec:async (_program,args)=>{calls.push(args);return {code:0,stdout:JSON.stringify({kind:'agent_context',case_id:'task-case:budget',
      coverage:{completeness:completeness()},files:[]}),stderr:''};}});
  const injected=await events.get('before_agent_start')({prompt:'inspect storage'},{cwd:'/repo'});
  assert.ok(Buffer.byteLength(injected.message.content)<=12000);
  const args=JSON.parse(calls[0][calls[0].indexOf('--args')+1]);
  assert.ok(args.max_bytes<12000&&args.max_bytes>11000);
  for(const invalid of ['bad','0','1000000','4096.5']) {process.env.BRIEFCASE_CONTEXT_MAX_BYTES=invalid;assert.equal(adapter.contextBudget(),8000);}
});

test('automatic context preserves compact manifests with more than twenty supplemental files', async () => {
  const files=Array.from({length:24},(_,index)=>({path:`src/part_${index}.rs`,role:'impl',rank:index+2,
    why:'dependency: imports',ref:`/slice/impl/${index+1}`}));
  const manifest={schema_version:'2.0',kind:'agent_context',case_id:'task-case:compact',report_id:'task-case:compact',
    anchor:{target:'src/lib.rs',status:'resolved',path:'src/lib.rs',ref:'/slice/impl/0'},files,
    fetch_source:{operation:'explain',arguments:{report_id:'task-case:compact'}},
    coverage:{completeness:completeness()},freshness:{state:'unknown',view_count:6}};
  const events=new Map();
  adapter.default({registerTool(){},registerCommand(){},on:(name,handler)=>events.set(name,handler),
    exec:async()=>({code:0,stdout:JSON.stringify(manifest),stderr:''})});
  const injected=await events.get('before_agent_start')({prompt:'inspect storage'},{cwd:'/repo'});
  assert.ok(Buffer.byteLength(injected.message.content)<=8000);
  const delivered=JSON.parse(injected.message.content.slice(injected.message.content.indexOf('{')));
  assert.deepEqual(delivered,manifest);
});

test('normal completion uses the original Case and workspace without another context or metadata read', async () => {
  const events = new Map(), calls = [], entries = [];
  adapter.default({registerTool(){}, registerCommand(){}, on:(name,handler)=>events.set(name,handler),
    appendEntry:(type,value)=>entries.push(value), exec:async (_program,args,options)=>{
      const operation=args[args.indexOf('call')+1];
      const arguments_=JSON.parse(args[args.indexOf('--args')+1]);
      calls.push({operation, arguments_, cwd:options.cwd});
      const result=operation==='worktree'
        ? {kind:'worktree',report_id:'context-report:live',alignment:'changed',changed_paths:2,outside_initial_slice:1}
        : {kind:'agent_context',truncated:true,case_id:'task-case:original',report_id:'task-case:original'};
      return {code:0,stdout:JSON.stringify(result),stderr:''};
    }});
  await events.get('before_agent_start')({prompt:'scale overlapping jobs'},{cwd:'/original'});
  await events.get('agent_end')({},{cwd:'/other'});
  assert.deepEqual(calls.map(c=>c.operation),['agent_context','worktree']);
  assert.equal(calls[1].arguments_.case_id,'task-case:original');
  assert.equal(calls[1].cwd,'/original');
  assert.equal(entries[0].case_id,'task-case:original');
  assert.equal(entries[0].dimension_comparison,'not_requested');
  assert.equal(entries[0].worktree.outside_initial_slice,1);
});

test('compact native freshness preserves degraded fallback and read errors never trigger refresh', async () => {
  const calls=[];
  const pi={exec:async (_program,args)=>{calls.push(args);return {code:0,stderr:'',
    stdout:JSON.stringify({kind:'agent_context',view_id:'view:old',freshness:{fallback:true,pending_or_interrupted:true}})};}};
  assert.equal((await adapter.execRead(pi,'agent_context',{task:'inspect'},'/repo')).degraded,true);
  await assert.rejects(adapter.execRead({exec:async (_program,args)=>{
    calls.push(args); return {code:1,stdout:'',stderr:'CORRUPT: source digest mismatch'};
  }},'explain',{report_id:'task-case:old',ref:'/slice/impl/0'},'/repo'),/source digest mismatch/);
  assert.equal(calls.length,2);
  assert.ok(calls.every(args=>args[args.indexOf('--wait-ready')+1]==='0'));
  assert.ok(calls.every(args=>args[args.indexOf('call')+1]!=='sync'));
});

test('oversized manifests preserve scoped coverage, a pinned gap, and truthful omissions', () => {
  const gaps=[{area:'capture_path',ref:'/not_covered/7',path:'src/jobs.py',reason:'Current source is missing'}];
  const manifest={kind:'agent_context',case_id:'task-case:gaps',report_id:'task-case:gaps',
    outcome:'partial',outcome_scope:'retained_evidence',files:[{path:'a.py',detail:'é'.repeat(10000)}],
    findings:[{claim:'advisory',detail:'x'.repeat(10000)}],not_covered:gaps,
    coverage:{completeness:completeness(),delivery:{files:{total:9},findings:{total:4},not_covered:{total:8}}},
    fetch_source:{operation:'explain',arguments:{report_id:'task-case:gaps'}}};
  const encoded=adapter.formatOperation(manifest,2200);
  const notice=JSON.parse(encoded);
  assert.ok(Buffer.byteLength(encoded)<=2200);
  assert.deepEqual(notice.coverage.completeness,completeness());
  assert.deepEqual(notice.not_covered,gaps);
  assert.deepEqual(notice.coverage.delivery.files,{total:9,returned:0,omitted:9});
  assert.deepEqual(notice.coverage.delivery.findings,{total:4,returned:0,omitted:4});
  assert.deepEqual(notice.coverage.delivery.not_covered,{total:8,returned:1,omitted:7});
  assert.equal(notice.fetch_source.arguments.report_id,'task-case:gaps');
  delete manifest.coverage.delivery.findings;
  manifest.sections={'/findings':4};
  assert.deepEqual(JSON.parse(adapter.formatOperation(manifest,2200)).coverage.delivery.findings,
    {total:4,returned:0,omitted:4},'legacy manifests can recover totals from retained section counts');
});
