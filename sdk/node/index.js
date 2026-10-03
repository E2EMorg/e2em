import {valid} from './schema.js';
import responseSchema from './response.schema.json' with {type:'json'};
import net from 'node:net';
import fs from 'node:fs/promises';
import { createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
const MAX_FRAME = 131072;
export class E2EMError extends Error {
  constructor(code) { super(code); this.code = code; this.action = 'review'; }
}
const proof = (secret, role, provider, principal, client, server) => createHmac('sha256', secret).update([role, provider, principal, client, server].join('\0') + '\0').digest('hex');
const equal = (a, b) => typeof a === 'string' && a.length === b.length && timingSafeEqual(Buffer.from(a), Buffer.from(b));
const snapshot = request => JSON.stringify(request);
const freeze = value => { if (value && typeof value === 'object') { for (const child of Object.values(value)) freeze(child); Object.freeze(value); } return value; };
export function utf16Span(text, start, end) {
  const raw = Buffer.from(text);
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start < 0 || start > end || end > raw.length) throw new E2EMError('INVALID_REQUEST');
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
  try { return [decoder.decode(raw.subarray(0, start)).length, decoder.decode(raw.subarray(0, end)).length]; }
  catch { throw new E2EMError('INVALID_REQUEST'); }
}
export function validateAssessment(value, request) {
  try {
    if (!valid(value,responseSchema.$defs.Assessment)) throw 0;
    const required = ['api_version','request_id','message_id','message_revision','status','action','versions','coverage','findings','reason_codes','duration_ms'];
    if (!value || required.some(k => !(k in value)) || Object.keys(value).some(k => !required.includes(k) && k !== 'error_code')) throw 0;
    if (value.api_version !== '0.1' || value.request_id !== request.request_id || value.message_id !== request.message.id || value.message_revision !== request.message.revision) throw 0;
    const selected = request.policy ?? request.policy_ref;
    if (['assessed','indeterminate'].includes(value.status) && (value.versions.policy_id !== selected.id || value.versions.policy_version !== selected.version)) throw 0;
    if (!['assessed','indeterminate','error','cancelled'].includes(value.status) || !['allow','warn','review','block'].includes(value.action)) throw 0;
    if (value.status !== 'assessed' && value.action !== 'review') throw 0;
    if (value.status === 'error' && typeof value.error_code !== 'string') throw 0;
    if (value.status === 'assessed' && (value.coverage.target_complete !== true || value.coverage.context_complete !== true || value.coverage.unevaluated_rules.length)) throw 0;
    if (!value.versions || !Array.isArray(value.reason_codes) || !Array.isArray(value.findings)) throw 0;
    const texts = new Map([[request.message.id, request.message.text], ...(request.context ?? []).map(t => [t.id, t.text])]);
    for (const finding of value.findings) {
      if (!['deterministic','model','custom_policy'].includes(finding.method)) throw 0;
      if (finding.score !== null && (typeof finding.score !== 'number' || !Number.isFinite(finding.score) || finding.score < 0 || finding.score > 1)) throw 0;
      for (const span of finding.spans) utf16Span(texts.get(span.message_id), span.start, span.end);
    }
    return value;
  } catch { throw new E2EMError('INTERNAL_ERROR'); }
}
export class Client {
  constructor(socket) {
    this.socket = socket; this.pending = new Map(); this.counter = 0; this.closed = false;
    this.buffer = Buffer.alloc(0); this.frames = []; this.waiters = [];
    socket.on('data', chunk => {
      this.buffer = Buffer.concat([this.buffer, chunk]);
      try {
        while (this.buffer.length >= 4) {
          const size = this.buffer.readUInt32BE(0);
          if (!size || size > MAX_FRAME) throw new E2EMError('INTERNAL_ERROR');
          if (this.buffer.length < size + 4) break;
          const frame = JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(this.buffer.subarray(4,size+4)));
          this.buffer = this.buffer.subarray(size+4);
          if (this.waiters.length) this.waiters.shift().resolve(frame);
          else if (this.receiving) this.dispatch(frame);
          else { if (this.frames.length >= 4) throw new E2EMError('INTERNAL_ERROR'); this.frames.push(frame); }
        }
      } catch { this.fail('INTERNAL_ERROR'); }
    });
    socket.on('error', () => this.fail('MODEL_UNAVAILABLE'));
    socket.on('close', () => this.fail('MODEL_UNAVAILABLE'));
  }
  fail(code) {
    this.closed = true; this.socket.destroy();
    for (const {reject,timer} of this.pending.values()) { clearTimeout(timer); reject(new E2EMError(code)); }
    this.pending.clear();
    for (const {reject,timer} of this.waiters) { clearTimeout(timer); reject(new E2EMError(code)); }
    this.waiters = [];
  }
  frame() {
    if (this.frames.length) return Promise.resolve(this.frames.shift());
    if (this.closed) return Promise.reject(new E2EMError('MODEL_UNAVAILABLE'));
    return new Promise((resolve,reject) => {
      const timer = setTimeout(() => { this.fail('DEADLINE_EXCEEDED'); },5000);
      this.waiters.push({resolve: v => {clearTimeout(timer); resolve(v);},reject,timer});
    });
  }
  send(value) {
    let bytes;
    try { bytes = Buffer.from(JSON.stringify(value)); }
    catch { throw new E2EMError('INVALID_REQUEST'); }
    if (!bytes.length || bytes.length > MAX_FRAME) throw new E2EMError('INVALID_REQUEST');
    const prefix = Buffer.alloc(4); prefix.writeUInt32BE(bytes.length);
    this.socket.write(Buffer.concat([prefix,bytes]));
  }
  static async open({socketPath, principal, secret, provider}) {
    try {
    if (process.platform === 'win32') {
      if (typeof socketPath !== 'string' || socketPath.length > 200 || !/^\\\\\.\\pipe\\e2em-[A-Za-z0-9-]+$/.test(socketPath)) throw new E2EMError('MODEL_UNAVAILABLE');
    } else for (const [path,isSocket] of [[socketPath.slice(0,socketPath.lastIndexOf('/')),false],[socketPath,true]]) {
      const stat = await fs.lstat(path);
      if (stat.uid !== process.getuid() || (stat.mode & 0o077) || (isSocket ? !stat.isSocket() : !stat.isDirectory())) throw new E2EMError('MODEL_UNAVAILABLE');
    }
    } catch { throw new E2EMError('MODEL_UNAVAILABLE'); }
    const socket = net.createConnection(socketPath);
    const client = new Client(socket);
    try {
      const nonce = randomBytes(32).toString('hex'); client.send({principal,nonce});
      const challenge = await client.frame();
      if (challenge.provider !== provider || typeof challenge.nonce !== 'string' || !/^[a-f0-9]{64}$/.test(challenge.nonce) || !equal(challenge.proof,proof(secret,'server',provider,principal,nonce,challenge.nonce))) throw new E2EMError('MODEL_UNAVAILABLE');
      client.send({proof:proof(secret,'client',provider,principal,nonce,challenge.nonce)});
      const authenticated = await client.frame();
      if (authenticated.authenticated !== true || authenticated.provider !== provider) throw new E2EMError('MODEL_UNAVAILABLE');
      client.provider = provider; client.receiving = true;
      for (const frame of client.frames.splice(0)) client.dispatch(frame);
      const capabilities = await client.capabilities();
      if (capabilities.api_version !== '0.1' || !capabilities.backend_ready) throw new E2EMError('MODEL_UNAVAILABLE');
      client.capabilityManifest = capabilities;
      return client;
    } catch (error) { client.close(); throw error instanceof E2EMError ? error : new E2EMError('MODEL_UNAVAILABLE'); }
  }
  static async discover(candidates, requiredDetectors, pinned) {
    const order = {native:0,project:1,embedded:2};
    for (const candidate of [...candidates].sort((a,b)=>order[a.kind]-order[b.kind])) {
      if (pinned && candidate.provider !== pinned) continue;
      try {
        const client = await Client.open(candidate);
        if (requiredDetectors.every(d=>client.capabilityManifest.detectors.includes(d))) return client;
        client.close();
      } catch {}
    }
    throw new E2EMError('MODEL_UNAVAILABLE');
  }
  dispatch(response) {
    if (!valid(response)) { this.fail('INTERNAL_ERROR'); return; }
    const pending = this.pending.get(response.call_id);
    if (pending) { clearTimeout(pending.timer); this.pending.delete(response.call_id); pending.resolve(response.reply); }
  }
  async call(operation, expected, timeout=5000) {
    if (this.closed) throw new E2EMError('MODEL_UNAVAILABLE');
    if (this.pending.size >= 8) throw new E2EMError('RESOURCE_EXHAUSTED');
    const call_id = String(++this.counter);
    const reply = await new Promise((resolve,reject) => {
      const timer = setTimeout(() => { this.pending.delete(call_id); reject(new E2EMError('DEADLINE_EXCEEDED')); },timeout);
      this.pending.set(call_id,{resolve,reject,timer});
      try { this.send({call_id,api_version:'0.1',operation}); }
      catch (error) { clearTimeout(timer); this.pending.delete(call_id); reject(error); }
    });
    if (reply?.kind === 'error') throw new E2EMError(reply.error_code ?? 'INTERNAL_ERROR');
    if (reply?.kind !== expected) throw new E2EMError('INTERNAL_ERROR');
    return reply;
  }
  async capabilities() { return (await this.call({op:'capabilities'},'capabilities')).capabilities; }
  async validatePolicy(policy) { return (await this.call({op:'validate_policy',policy},'policy')).policy_ref; }
  async assess(request, {signal} = {}) {
    if (signal?.aborted) throw new E2EMError('CANCELLED');
    const original = structuredClone(request);
    const abort = () => { this.cancel(original.request_id).catch(()=>{}); };
    signal?.addEventListener('abort',abort,{once:true});
    try {
      const reply = await this.call({op:'assess',request:original},'assessment',original.options?.deadline_ms ?? 1000);
      if (signal?.aborted) throw new E2EMError('CANCELLED');
      const result = validateAssessment(reply.assessment,original);
      const saved = snapshot(original);
      return freeze({...result, appliesTo: current => saved === snapshot(current)});
    } catch (error) { if (!this.closed) await this.cancel(original.request_id).catch(()=>{}); throw error; }
    finally { signal?.removeEventListener('abort',abort); }
  }
  async cancel(request_id) { return (await this.call({op:'cancel',request_id},'cancelled')).accepted; }
  close() { this.fail('MODEL_UNAVAILABLE'); }
}
