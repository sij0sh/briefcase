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
  assert.equal(commands[1][commands[1].indexOf('--wait-ready') + 1], '20');
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
  assert.equal(calls.length,2);
  assert.equal(JSON.parse(calls[1][calls[1].indexOf('--args')+1]).duty,'review');
  await events.get('agent_end')({},ctx);
  assert.equal(calls.length,2,'a settled turn must not check again');
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

test('post-task entries compare identified cases and keep dimension deltas', async () => {
  const events=new Map(), entries=[];let reads=0;
  adapter.default({registerTool(){},registerCommand(){},on:(name,handler)=>events.set(name,handler),
    appendEntry:(type,value)=>entries.push({type,value}),exec:async()=>{
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
  assert.equal(reads,2);
});

test('byte-budget notices recover dimension states from retained case metadata', async () => {
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
  assert.deepEqual(calls,['brief','explain','brief','explain']);
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
