import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import {Client,E2EMError,utf16Span,validateAssessment,assess,presets} from './index.js';
import {messageRequest} from './message.js';
import path from 'node:path';
import net from 'node:net';
import {EventEmitter} from 'node:events';
const cases = JSON.parse(await fs.readFile(new URL('../../tests/conformance/assessments.json',import.meta.url),'utf8'));
const policyFailures = JSON.parse(await fs.readFile(new URL('../../tests/conformance/policy-failures.json',import.meta.url),'utf8'));
const policyReports = JSON.parse(await fs.readFile(new URL('../../tests/conformance/policy-reports.json',import.meta.url),'utf8'));
const policy = JSON.parse(await fs.readFile(new URL('../../tests/conformance/email-policy.json',import.meta.url),'utf8'));
test('Unicode bytes convert to UTF16 without splitting characters',()=>{
 assert.deepEqual(utf16Span('🙂 alex@example.test',5,22),[3,20]);
 assert.deepEqual(utf16Span('\ufeff🙂 alex@example.test',8,25),[4,21]);
 assert.throws(()=>utf16Span('🙂 x',1,5),E2EMError);
});
test('unserializable requests fail with typed errors', async()=>{
 const socket = new EventEmitter(); socket.write=()=>{}; socket.destroy=()=>{};
 const client = new Client(socket);
 try { await assert.rejects(client.call({value:1n},'assessment'),error=>error.code === 'INVALID_REQUEST'); }
 finally {client.close();}
});
test('malformed and stale assessments never allow' ,()=>{
 assert.throws(()=>validateAssessment({status:'error',action:'allow'},cases[0].request),E2EMError);
});
test('message-only defaults cover every preset, optional context, and custom text',()=>{
 const draft = messageRequest('🙂 Original draft');
 assert.deepEqual(draft.policy.rules.map(rule=>rule.category),presets.map(preset=>preset.id));
 assert(draft.policy.rules.every(rule=>rule.context_requirement === (rule.category === 'spam.repeat' ? 'supplied_window' : 'target_only')));
 const contextual = messageRequest('🙂 Original draft',{context:['Earlier message','Reply'],customPolicies:'Keep project details private.'});
 assert.equal(contextual.policy.rules.length,presets.length+1);
 assert.deepEqual(contextual.context.map(turn=>turn.text),['Earlier message','Reply']);
 assert.equal(contextual.policy.rules.at(-1).policy_text,'Keep project details private.');
 const custom = messageRequest('Hello',{policies:[],customPolicies:'Custom text',context:'Earlier message'});
 assert.equal(custom.policy.rules.length,1);
 assert.equal(custom.policy.rules[0].match,'policy_text');
 assert.throws(()=>messageRequest('Hello',{context:[42]}));
});
if (process.argv.includes('--live')) {
 test('message API loads app settings, uses defaults, and preserves draft snapshots',async()=>{
  const socketPath = process.argv[process.argv.indexOf('--live')+1];
  const configPath = path.join(path.dirname(socketPath),'node-message-app.json');
  await fs.writeFile(configPath,JSON.stringify({socket_path:socketPath,principal:'node',secret:'b'.repeat(64),provider:'test-provider'}),{mode:0o600});
  try {
   const report = await assess('Hello',{app:'node',configPath});
   assert.equal(report.request.policy.rules.length,presets.length);
   assert.equal(report.status,'indeterminate');
   assert.deepEqual(report.unevaluated,presets.filter(preset=>preset.directions.includes('outgoing') && preset.id !== 'pii.email').map(preset=>preset.id));
   const client = await Client.connect({app:'node',configPath});
   try {
    const selected = await client.assess('Contact alex@example.test',{policies:'pii.email',context:'Earlier message'});
    assert.equal(selected.action,'warn');
    assert(selected.appliesTo(selected.request));
    const edited = structuredClone(selected.request); edited.context[0].text = 'Changed context';
    assert(!selected.appliesTo(edited));
    const custom = await client.assess('Hello',{policies:[],customPolicies:'Keep project details private.',context:['Earlier message']});
    assert.equal(custom.status,'indeterminate');
    assert.deepEqual(custom.unevaluated,['custom-1']);
    assert.deepEqual(custom.scores,{});
    assert(!JSON.stringify(custom).includes('Keep project details private.'));
   } finally {client.close();}
   await fs.chmod(configPath,0o644);
   await assert.rejects(Client.connect({app:'node',configPath}),E2EMError);
  } finally {await fs.unlink(configPath);}
 });
 test('Windows pipe adapter restricts names and preserves authentication',async()=>{
  const platform = Object.getOwnPropertyDescriptor(process,'platform');
  const connect = net.createConnection;
  const socketPath = process.argv[process.argv.indexOf('--live')+1];
  let calls = 0;
  try {
   Object.defineProperty(process,'platform',{value:'win32'});
   net.createConnection = () => {calls++; return connect(socketPath);};
   const connection = {principal:'node',secret:'b'.repeat(64),provider:'test-provider'};
   for (const path of [String.raw`\\remote\pipe\e2em-app`,String.raw`\\.\pipe\other`,String.raw`\\.\pipe\e2em-`,String.raw`\\.\pipe\e2em-app/remote`])
    await assert.rejects(Client.open({...connection,socketPath:path}),E2EMError);
   assert.equal(calls,0);
   const client = await Client.open({...connection,socketPath:String.raw`\\.\pipe\e2em-app`});
   try { assert.equal((await client.assess(cases[0].request)).action,'warn'); } finally {client.close();}
   await assert.rejects(Client.open({...connection,socketPath:String.raw`\\.\pipe\e2em-app`,provider:'imposter'}),E2EMError);
  } finally {Object.defineProperty(process,'platform',platform);net.createConnection=connect;}
 });
 test('Node reference application passes shared live fixtures',async()=>{
  const socketPath = process.argv[process.argv.indexOf('--live')+1];
  const connection = {socketPath,principal:'node',secret:'b'.repeat(64),provider:'test-provider'};
  const client = await Client.open(connection);
  try {
   for (const fixture of cases) {
    const result = await client.assess(fixture.request);
    assert.equal(result.status,fixture.expected.status); assert.equal(result.action,fixture.expected.action);
    assert.deepEqual(result.findings.flatMap(f=>f.spans.map(s=>[s.start,s.end])),fixture.expected.spans);
    assert(result.appliesTo(fixture.request));
    const edited = structuredClone(fixture.request); edited.message.revision = 'new'; assert(!result.appliesTo(edited));
    const {appliesTo,...wire} = result;
    assert.equal(validateAssessment(wire,fixture.request),wire);
    assert.throws(()=>validateAssessment({...wire,message_revision:'old'},fixture.request),E2EMError);
    assert.throws(()=>validateAssessment({...wire,versions:{...wire.versions,policy_version:'old'}},fixture.request),E2EMError);
   }
   for (const fixture of policyFailures) await assert.rejects(client.validatePolicy(fixture.policy),error=>error.code === fixture.error_code);
   const ref = await client.validatePolicy(policy); const request = structuredClone(cases[0].request); delete request.policy; request.policy_ref = ref;
   assert.equal((await client.assess(request)).action,'warn');
   for (const fixture of policyReports) {
    const experimental = structuredClone(fixture.policy); experimental.id = fixture.name;
    const policy_ref = await client.validatePolicy(experimental);
    const draft = structuredClone(cases[0].request); delete draft.policy; draft.policy_ref = policy_ref;
    const result = await client.assess(draft);
    assert.equal(result.status, fixture.expected.status);
    assert.equal(result.action, fixture.expected.action);
    assert.deepEqual(result.coverage.unevaluated_rules, fixture.expected.unevaluated_rules);
    assert.deepEqual(result.reason_codes, fixture.expected.reason_codes);
    assert.deepEqual(result.findings, []);
   }
   assert.equal(await client.cancel('missing'),false);
   const controller = new AbortController(); controller.abort();
   await assert.rejects(client.assess(request,{signal:controller.signal}),error=>error.code === 'CANCELLED');
   const nativeSocket = process.argv[process.argv.indexOf('--native-socket')+1];
   const native = await Client.discover([{...connection,kind:'project'},{...connection,kind:'native',socketPath:nativeSocket,provider:'native-provider'}],['pii.email']);
   try {
     assert.equal(native.provider,'native-provider');
     assert.notEqual(native.capabilityManifest.provider,client.capabilityManifest.provider);
     await assert.rejects(native.assess(request),error=>error.code === 'POLICY_NOT_FOUND');
     request.policy_ref = await native.validatePolicy(policy);
     assert.equal((await native.assess(request)).action,'warn');
   } finally {native.close();}
   await assert.rejects(Client.discover([{...connection,kind:'project'}],['abuse.threat']),E2EMError);
  } finally { client.close(); }
 });
}
