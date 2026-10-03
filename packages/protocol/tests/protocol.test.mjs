import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { validateApplication, validateApplicationRequest, validateApplicationResponse, validateBridge, validateErrorCode } from '../src/index.ts';
const root = new URL('../../../', import.meta.url);
const fixture = async name => JSON.parse(await readFile(new URL(`tests/fixtures/protocol/${name}.json`, root), 'utf8'));
const schema = async name => JSON.parse(await readFile(new URL(`schemas/${name}.schema.json`, root), 'utf8'));
const clone = value => structuredClone(value);
const status = await fixture('valid-status');
const control = await fixture('valid-control');
const parity = await fixture('parity-cases');
const responses = await fixture('response-cases');
const messages = await fixture('safe-messages');
const requestId = status.request_id;
const error = { request_id: requestId, kind: 'error', error: { code: 'INVALID_REQUEST', safe_message: 'Invalid.', retryable: false } };
const result = { request_id: requestId, kind: 'result', result: { type: 'ai_policy', value: { revision: '1', enabled: false } } };

function assertBad(run, value, reason) {
  const before = clone(value);
  const answer = run(value);
  assert.deepEqual(answer, { ok: false, reason }); // never leak payload or Ajv error paths
  assert.deepEqual(value, before); // never coerce, remove or mutate caller input
}

test('S04 processing shared positive-negative fixtures reject wire authority', async () => {
  for (const item of await fixture('processing-execution-responses')) {
    if (item.valid) assert.equal(validateApplicationResponse(item.frame).ok, true);
    else assertBad(validateApplicationResponse, item.frame, 'invalid_application');
    for (const tokens of item.reject_generated_tokens ?? []) {
      const frame = clone(item.frame);
      frame.result.value.value.probe_result.generated_tokens = tokens;
      assertBad(validateApplicationResponse, frame, 'invalid_application');
    }
  }
  for (const item of await fixture('processing-cases')) {
    const request = { ...status, ...item.value };
    if (item.valid) assert.equal(validateApplicationRequest(request).ok, true);
    else assertBad(validateApplicationRequest, request, 'invalid_method');
  }
});

test('S01-TS-DTO-01 shared Rust/schema/TS requests and two-pass method validation', () => {
  for (const req of [status, control]) assert.equal(validateApplicationRequest(req).ok, true);
  for (const item of parity.valid) {
    const req = { ...(item.base === 'status' ? status : control), ...item };
    delete req.base; delete req.id;
    assert.equal(validateApplicationRequest(req).ok, true, item.id);
  }
  for (const item of parity.invalid) {
    const req = { ...(item.base === 'status' ? status : control), ...item };
    delete req.base; delete req.id;
    assertBad(validateApplicationRequest, req, ['preview-refuses-path-authority', 'metadata-refuses-principal-authority', 'unknown-method', 'unknown-param', 'revision-overflow', 'revision-leading-zero', 'revision-number', 'revision-negative', 'import-unknown-authority', 'import-unknown-profile', 'export-background-overflow', 'export-background-negative', 'export-background-wrong-length'].includes(item.id) ? 'invalid_method' : 'invalid_application');
    if (item.id === 'unknown-method' || item.id === 'unknown-param') assert.equal(validateApplication(req).ok, true);
  }
  const probe = { fixture: 'synthetic_menu_v1', profile_id: 'x'.repeat(128), allow_model_load: false, consent_id: requestId };
  const auth = { fixture: 'embedding_text_v1', profile_id: 'fixture', allow_load: false, lifetime_seconds: 300 };
  for (const [method, params] of [
    ['system.doctor', { include_runtime_inventory: false }],
    ['runtime.probe', probe],
    ['system.authorize_probe', auth],
    ['system.request_shutdown', {}],
  ]) assert.equal(validateApplicationRequest({ ...status, method, params }).ok, true, method);
  for (const params of [{ ...auth, lifetime_seconds: 0 }, { ...auth, lifetime_seconds: 301 }, { ...auth, profile_id: 'x'.repeat(129) }, { ...probe, consent_id: 'NOT-UUID' }]) {
    assertBad(validateApplicationRequest, { ...status, method: 'lifetime_seconds' in params ? 'system.authorize_probe' : 'runtime.probe', params }, 'invalid_method');
  }
  for (const bad of [null, false, 'secret', [], { credential: 'secret' }]) {
    assertBad(validateApplicationRequest, bad, 'invalid_application');
  }
});

test('S01-TS-DTO-02 exclusive responses, registry, optional null, UTC and identifier boundaries', () => {
  for (const response of [result, error]) assert.equal(validateApplicationResponse(response).ok, true);
  assertBad(validateApplicationResponse, { ...error, result: result.result }, 'invalid_application');
  assertBad(validateApplicationResponse, { ...result, error: error.error }, 'invalid_application');
  assertBad(validateApplicationResponse, { ...error, error: { ...error.error, credential: 'secret' } }, 'invalid_application');
  assertBad(validateApplicationResponse, { ...error, error: { ...error.error, code: 'UNKNOWN' } }, 'invalid_application');
  assertBad(validateApplicationResponse, { ...result, result: { type: 'probe_consent', value: { consent_id: requestId, expires_at: '2026-02-29T00:00:00Z', policy_revision: '1' } } }, 'invalid_application');
  const consent = { ...result, result: { type: 'probe_consent', value: { consent_id: requestId, expires_at: '2024-02-29T00:00:00Z', policy_revision: '18446744073709551615' } } };
  assert.equal(validateApplicationResponse(consent).ok, true);
  for (const expires_at of ['2024-02-29T00:00:00+00:00', '2024-02-29T00:00:00Z\n']) {
    assertBad(validateApplicationResponse, { ...consent, result: { ...consent.result, value: { ...consent.result.value, expires_at } } }, 'invalid_application');
  }
  const doctor = { ...result, result: { type: 'doctor', value: { report_id: requestId, host: {
    host_instance_id: requestId, library_id: status.library_id, build_id: 'a'.repeat(128), protocol: status.protocol, state: 'ready', schema_version: 0, ai_enabled: false, bridge_state: 'unavailable', available_methods: ['x'.repeat(96)]
  }, checks: [{ check_id: 'fixture', outcome: 'NOT_RUN' }] } } };
  assert.equal(validateApplicationResponse(doctor).ok, true);
  const badDoctor = clone(doctor); badDoctor.result.value.checks[0].duration_ms = null;
  assertBad(validateApplicationResponse, badDoctor, 'invalid_application');
  for (const field of ['build_id', 'available_methods']) {
    const bad = clone(doctor); bad.result.value.host[field] = field === 'build_id' ? 'a'.repeat(129) : ['x'.repeat(97)];
    assertBad(validateApplicationResponse, bad, 'invalid_application');
  }
  assert.equal(validateErrorCode('METHOD_NOT_SUPPORTED').ok, true);
  assertBad(validateErrorCode, 'SECRET', 'invalid_error_code');
});

test('S01-TS-DTO-05 shared Rust/Ajv/schema response parity and calendar mutation', async () => {
  for (const item of responses.valid) {
    assert.equal(validateApplicationResponse(clone(item.frame)).ok, true, item.id);
  }
  for (const item of responses.invalid) {
    assertBad(validateApplicationResponse, clone(item.frame), 'invalid_application');
  }
  // Removing date-time verification must make the shared non-leap-day negative
  // pass the mutant; the unchanged production validator must still reject it.
  const require = createRequire(import.meta.url);
  const Ajv2020 = require('ajv/dist/2020').default;
  const addFormats = require('ajv-formats').default;
  const mutated = clone(await schema('application-v1'));
  assert.equal(mutated.$defs.UtcTime.format, 'date-time');
  delete mutated.$defs.UtcTime.format;
  const ajv = new Ajv2020({ strict: true });
  addFormats(ajv);
  ajv.addFormat('uint8', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 255 });
  ajv.addFormat('uint16', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 65535 });
  ajv.addFormat('uint32', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 4294967295 });
  ajv.addFormat('uint64', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n < 2 ** 64 });
  const withoutCalendar = ajv.compile(mutated);
  const invalidLeapDay = responses.invalid.find(item => item.id === 'consent-not-leap-year').frame;
  assert.equal(validateApplicationResponse(clone(invalidLeapDay)).ok, false);
  assert.equal(withoutCalendar(clone(invalidLeapDay)), true);
  const withoutUnion = clone(await schema('application-v1'));
  assert.equal(withoutUnion.$defs.Response.oneOf[0].additionalProperties, false);
  withoutUnion.$defs.Response.oneOf[0].additionalProperties = true;
  const both = responses.invalid.find(item => item.id === 'both-on-result').frame;
  assert.equal(validateApplicationResponse(clone(both)).ok, false);
  assert.equal(ajv.compile(withoutUnion)(clone(both)), true);
});

test('S01-TS-DTO-03 SafeMessage shared C0 DEL C1 trailing LF Unicode and scalar length', () => {
  for (const item of messages.invalid) assertBad(validateApplicationResponse, { ...error, error: { ...error.error, safe_message: item.safe_message } }, 'invalid_application');
  for (const item of messages.valid) assert.equal(validateApplicationResponse({ ...error, error: { ...error.error, safe_message: item.safe_message } }).ok, true, item.id);
  for (const len of [512, 513]) {
    const message = '😀'.repeat(len);
    assert.equal(validateApplicationResponse({ ...error, error: { ...error.error, safe_message: message } }).ok, len === 512);
  }
});

test('S01-TS-DTO-04 bridge is separate; no application method implication', () => {
  const req = { protocol: { major: 1, minor: 0 }, request_id: requestId, fixture: 'synthetic_menu_v1', profile_id: 'a'.repeat(128), allow_model_load: false };
  const response = { kind: 'result', request_id: requestId, checks: [{ check_id: 'fixture', outcome: 'BLOCKED' }] };
  assert.equal(validateBridge(req).ok, true);
  assert.equal(validateBridge(response).ok, true);
  assert.equal(validateBridge({ kind: 'error', request_id: requestId, error: { code: 'UNAVAILABLE', retryable: true } }).ok, true);
  for (const bad of [{ ...req, credential: 'secret' }, { ...req, profile_id: 'x'.repeat(129) }, { ...req, fixture: 'foreign' }, { ...response, error: { code: 'UNAVAILABLE', retryable: true } }, { ...req, protocol: { major: 65536, minor: 0 } }]) {
    assertBad(validateBridge, bad, 'invalid_bridge');
  }
});

test('S01-TS-GEN-02 explicit temp generation and drift fail without touching sources', async () => {
  const sandbox = await mkdtemp(join(tmpdir(), 'screenmemory-contract-'));
  const generated = join(sandbox, 'fresh');
  const script = new URL('../scripts/generate.mjs', import.meta.url);
  const run = (...args) => spawnSync(process.execPath, [fileURLToPath(script), ...args], { encoding: 'utf8' });
  try {
    assert.equal(run('--output', generated).status, 0);
    assert.equal(run('--check', generated).status, 0);
    const target = join(generated, 'application-v1.ts');
    await writeFile(target, `${await readFile(target, 'utf8')}\n// deliberate drift\n`);
    const drift = run('--check', generated);
    assert.equal(drift.status, 1);
    assert.match(drift.stderr, /Generated TypeScript differs/);
    assert.equal(run('--check').status, 0); // tracked source was never modified
  } finally {
    await rm(sandbox, { recursive: true, force: true });
  }
});

test('S01-TS-MUT-01 in-memory removal of Rust SafeMessage guard fails shared negative cases', async () => {
  const require = createRequire(import.meta.url);
  const Ajv2020 = require('ajv/dist/2020').default;
  const addFormats = require('ajv-formats').default;
  const original = await schema('application-v1');
  const mutated = clone(original);
  assert.deepEqual(mutated.$defs.SafeMessage.not, { pattern: '[\\u0000-\\u001F\\u007F-\\u009F]' });
  delete mutated.$defs.SafeMessage.not;
  const ajv = new Ajv2020({ strict: true });
  addFormats(ajv);
  ajv.addFormat('uint8', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 255 });
  ajv.addFormat('uint16', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 65535 });
  ajv.addFormat('uint32', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 4294967295 });
  ajv.addFormat('uint64', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n < 2 ** 64 });
  const unguarded = ajv.compile(mutated);
  for (const item of messages.invalid) {
    const candidate = { ...error, error: { ...error.error, safe_message: item.safe_message } };
    assert.equal(validateApplicationResponse(candidate).ok, false, item.id);
    assert.equal(unguarded(candidate), true, item.id);
  }
});
