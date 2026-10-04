import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import test from 'node:test';
import Ajv2020 from 'ajv/dist/2020.js';
import addFormats from 'ajv-formats';
test('S06 shared closed search request fixtures Rust/Ajv parity', () => {
 const file = name => JSON.parse(readFileSync(fileURLToPath(new URL(`../../../${name}`, import.meta.url)), 'utf8'));
 const ajv = new Ajv2020({ strict: true }); addFormats(ajv); ajv.addFormat('uint8', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 255 }); ajv.addFormat('uint64', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n < 2 ** 64 }); ajv.addFormat('uint16', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 65535 }); ajv.addFormat('uint32', { type: 'number', validate: n => Number.isInteger(n) && n >= 0 && n <= 4294967295 });
 const validate = ajv.compile(file('schemas/application-methods-v1.schema.json'));
 for (const c of file('tests/fixtures/protocol/search-cases.json')) assert.equal(validate(c.value), c.valid, c.id);
});
