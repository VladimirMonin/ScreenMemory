import { timingSafeEqual } from 'node:crypto';
import { z } from 'zod';
import { SdkAdapter, SafeFailure } from './sdk-adapter.ts';

const MAX_FRAME = 4 * 1024 * 1024, MAX_IMAGE = 20 * 1024 * 1024;
const text = z.string().min(1).max(512);
const uuid = z.string().uuid();
const cache = z.enum(['F16', 'F32', 'Q8_0', 'Q4_0', 'Q4_1', 'Q5_0', 'Q5_1', 'IQ4_NL']);
const load = z.object({ intent_id: uuid, identifier: z.string().regex(/^sm-[0-9a-f-]{36}$/),
  model_key: text, context_length: z.number().int().min(1024).max(262144),
  eval_batch_size: z.number().int().min(1).max(2048), flash_attention: z.boolean(),
  offload_kv_cache_to_gpu: z.boolean(), k_cache_type: cache, v_cache_type: cache,
  ttl_seconds: z.number().int().min(1).max(86400).nullable() }).strict().refine(p => ['F16','F32'].includes(p.v_cache_type) || p.flash_attention);
const vision = z.object({ instance_reference: text, prompt: z.string().min(1).max(32768),
  schema: z.record(z.unknown()), max_tokens: z.number().int().min(1).max(4000),
  temperature: z.number().min(0).max(1), reasoning: z.enum(['backend_default', 'off', 'budget100']),
  width: z.number().int().min(1).max(2048), height: z.number().int().min(1).max(2048),
  max_side: z.union([z.literal(1024), z.literal(2048)]), sha256: z.string().regex(/^[0-9a-f]{64}$/) }).strict();
const empty = z.object({}).strict();
const methods = { inventory: empty, load, observe: z.object({ instance_reference: text }).strict(),
  unload: z.object({ instance_reference: text }).strict(), vision,
  cancel: z.object({ target: z.string().regex(/^[1-9][0-9]{0,15}$/) }).strict(),
  status: z.object({ target: z.string().regex(/^[1-9][0-9]{0,15}$/) }).strict(), shutdown: empty };
const request = z.object({ id: z.string().regex(/^[1-9][0-9]{0,15}$/), auth: z.string().regex(/^[0-9a-f]{64}$/),
  generation: uuid, method: z.enum(Object.keys(methods)), params: z.unknown(),
  deadline_ms: z.number().int().min(1).max(120000), image_length: z.number().int().min(0).max(MAX_IMAGE) }).strict();
let adapter, auth, generation, lastId = 0n, active = 0, mutation = false, poisoned = false;
const calls = new Map();
function fail() { process.stderr.write('BRIDGE_PROTOCOL_REFUSED\n'); process.exit(7); }
function output(value) {
  const data = Buffer.from(JSON.stringify(value));
  if (data.length > MAX_FRAME || process.stdout.writableLength > 2 * MAX_FRAME) return fail();
  const header = Buffer.alloc(4); header.writeUInt32LE(data.length);
  process.stdout.write(Buffer.concat([header, data]));
}
function schemaSafe(value, depth = 0) {
  if (depth > 16) return false;
  if (value && typeof value === 'object') {
    return Object.entries(value).every(([k, v]) => !['$ref', '$id', '$schema'].includes(k) && schemaSafe(v, depth + 1));
  }
  return true;
}
async function dispatch(raw, image) {
  if (!adapter) {
    const init = z.object({ protocol: z.literal('sm-sdk/1.1'), auth: z.string().regex(/^[0-9a-f]{64}$/),
      generation: uuid, endpoint: z.string().regex(/^ws:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}$/),
      api_token: z.string().max(8192) }).strict().safeParse(raw);
    if (!init.success || image.length || Number(new URL(init.data.endpoint).port) > 65535) return fail();
    auth = Buffer.from(init.data.auth, 'hex'); generation = init.data.generation;
    adapter = new SdkAdapter(init.data.endpoint, init.data.api_token);
    output({ id: '0', generation, ok: true, result: { protocol: 'sm-sdk/1.1', sdk: '2.0.0',
      unload: 'unsupported', reasoning: 'unsupported' } });
    return;
  }
  const parsed = request.safeParse(raw);
  if (!parsed.success) return fail();
  const r = parsed.data;
  if (!timingSafeEqual(auth, Buffer.from(r.auth, 'hex')) || r.generation !== generation || BigInt(r.id) <= lastId) return fail();
  lastId = BigInt(r.id);
  const params = methods[r.method].safeParse(r.params);
  if (!params.success || image.length !== r.image_length || (r.method !== 'vision' && image.length)) return fail();
  const reply = (ok, value) => output({ id: r.id, generation, ok, ...(ok ? { result: value } : { error: value }) });
  if (active >= 8) return reply(false, { code: 'OVERLOADED', settlement: 'settled' });
  if (r.method === 'cancel' || r.method === 'status') {
    const target = calls.get(params.data.target);
    if (r.method === 'cancel' && target?.controller) target.controller.abort();
    return reply(true, { state: target?.state ?? 'unknown', settlement: target?.state === 'settled' ? 'settled' : 'uncertain' });
  }
  if (r.method === 'shutdown') { await adapter.close().catch(() => {}); reply(true, {}); process.exit(0); }
  const mutates = ['load', 'vision', 'unload'].includes(r.method);
  if (mutates && (mutation || poisoned)) return reply(false, { code: poisoned ? 'RECONCILIATION_REQUIRED' : 'BUSY', settlement: poisoned ? 'uncertain' : 'settled' });
  active++; if (mutates) mutation = true;
  const controller = new AbortController(); const call = { controller, state: 'active' }; calls.set(r.id, call);
  let replied = false;
  const timeout = setTimeout(() => controller.abort(), r.deadline_ms);
  const hard = setTimeout(() => {
    replied = true; call.state = 'uncertain'; poisoned ||= mutates;
    reply(false, { code: 'DEADLINE_UNCERTAIN', settlement: 'uncertain' });
  }, r.deadline_ms + 2000);
  try {
    const p = params.data; let result;
    if (r.method === 'inventory') result = await adapter.inventory();
    else if (r.method === 'load') result = await adapter.load(p, controller.signal);
    else if (r.method === 'observe') result = await adapter.observe(p.instance_reference);
    else if (r.method === 'unload') result = await adapter.unload(p.instance_reference);
    else {
      const { createHash } = await import('node:crypto');
      if (!schemaSafe(p.schema) || !image.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10]))
          || image.length < 24 || image.readUInt32BE(16) !== p.width || image.readUInt32BE(20) !== p.height
          || Math.max(p.width, p.height) > p.max_side || createHash('sha256').update(image).digest('hex') !== p.sha256) throw new SafeFailure('IMAGE_INVALID');
      result = await adapter.vision(p, image.toString('base64'), controller.signal);
    }
    call.state = 'settled';
    if (!replied) reply(true, result);
  } catch (error) {
    const safe = error instanceof SafeFailure ? error : new SafeFailure('SDK_FAILURE', 'uncertain');
    call.state = safe.settlement; poisoned ||= mutates && safe.settlement === 'uncertain';
    if (!replied) reply(false, { code: safe.code, settlement: safe.settlement });
  } finally {
    clearTimeout(timeout); clearTimeout(hard); active--; if (mutates) mutation = false;
    call.controller = undefined;
    if (calls.size > 64) {
      for (const [id, value] of calls) {
        if (calls.size <= 64) break;
        if (!value.controller) calls.delete(id);
      }
    }
    image.fill(0);
  }
}
let buffer = Buffer.alloc(0), pending;
process.stdin.on('data', chunk => {
  if (buffer.length + chunk.length > MAX_FRAME + MAX_IMAGE + 4) return fail();
  buffer = Buffer.concat([buffer, chunk]);
  while (true) {
    if (!pending) {
      if (buffer.length < 4) break;
      const n = buffer.readUInt32LE(); if (n < 2 || n > MAX_FRAME) return fail();
      if (buffer.length < n + 4) break;
      let raw; try { raw = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(buffer.subarray(4, n + 4))); } catch { return fail(); }
      const length = raw.image_length ?? 0;
      if (!Number.isInteger(length) || length < 0 || length > MAX_IMAGE) return fail();
      buffer = buffer.subarray(n + 4); pending = { raw, length };
    }
    if (buffer.length < pending.length) break;
    const image = Buffer.from(buffer.subarray(0, pending.length)); buffer = buffer.subarray(pending.length);
    const { raw } = pending; pending = undefined;
    void dispatch(raw, image).catch(fail);
  }
});
process.stdin.on('end', () => { if (pending || buffer.length) fail(); else process.exit(0); });
process.on('uncaughtException', fail); process.on('unhandledRejection', fail);
