// S01-T06d parity: the numbers the window shows are owned by the Rust crates. This check
// reads the real owners and fails on drift instead of trusting a duplicated literal, so a
// renamed protocol version or a shortened capability list cannot silently reach the window.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const repositoryFile = (relative) => readFileSync(new URL(`../../../${relative}`, import.meta.url), 'utf8');
const guiSource = readFileSync(new URL('../src/model.ts', import.meta.url), 'utf8');

const manifest = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
assert.equal(manifest.name, '@screenmemory/gui');

// 1. Build identity: the frontend version and the application protocol version it expects.
const version = /guiBuild = \{\s*version: '([^']+)',\s*protocol: \{ major: (\d+), minor: (\d+) \},/.exec(guiSource);
assert(version, 'model.ts must keep its build identity parseable');
assert.equal(version[1], manifest.version, 'frontend build version must match package.json');
const protocol = /APPLICATION_VERSION: ProtocolVersion = ProtocolVersion \{ major: (\d+), minor: (\d+) \};/.exec(
  repositoryFile('crates/protocol/src/application.rs'),
);
assert(protocol, 'APPLICATION_VERSION must stay parseable in the protocol owner');
assert.equal(
  `${version[2]}.${version[3]}`,
  `${protocol[1]}.${protocol[2]}`,
  'the window must expect the protocol version the protocol owner defines',
);

// 2. Envelope version of this slice.
const envelope = /pub const GUI_ENVELOPE_VERSION: u32 = (\d+);/.exec(repositoryFile('crates/gui/src/model.rs'));
assert(envelope, 'GUI_ENVELOPE_VERSION must stay parseable in the gui owner');
const tsEnvelope = /export const GUI_ENVELOPE_VERSION = (\d+);/.exec(guiSource);
assert(tsEnvelope, 'model.ts must keep its envelope version parseable');
assert.equal(tsEnvelope[1], envelope[1], 'the window must carry the envelope version of its Rust owner');

// 3. The typed state union and the rendered text are the same documented set.
const stateUnion = /export type GuiState =([\s\S]*?);/.exec(guiSource);
assert(stateUnion, 'GuiState must stay parseable');
const typedStates = [...stateUnion[1].matchAll(/'([a-z_]+)'/g)].map((match) => match[1]);
const stateText = /export const stateText: Record<GuiState, string> = \{([\s\S]*?)\n\};/.exec(guiSource);
assert(stateText, 'stateText must stay parseable');
const renderedStates = [...stateText[1].matchAll(/(\w+):/g)].map((match) => match[1]);
assert.deepEqual(renderedStates.sort(), [...typedStates].sort(), 'every typed state must be rendered');
assert.deepEqual(typedStates.sort(), [
  'connecting',
  'host_not_running',
  'protocol_mismatch',
  'ready',
  'root_credential_error',
  'runtime_unavailable',
]);

// 4. The bridge states are exactly the generated wire union.
const generated = repositoryFile('packages/protocol/src/generated/application-v1.ts');
const wire = /export type BridgeState = ([^;]+);/.exec(generated);
assert(wire, 'BridgeState must stay parseable in the generated contract');
const wireStates = [...wire[1].matchAll(/'([a-z_]+)'/g)].map((match) => match[1]);
const mapped = /export const bridgeText: Record<BridgeState, string> = \{([\s\S]*?)\n\};/.exec(guiSource);
assert(mapped, 'bridgeText must stay parseable');
const mappedStates = [...mapped[1].matchAll(/(\w+):/g)].map((match) => match[1]);
assert.deepEqual(mappedStates.sort(), [...wireStates].sort(), 'every wire bridge state must be rendered');

// 5. The capability list the window shows is the owner list, and it claims no counts.
const owner = repositoryFile('crates/gui/src/model.rs');
const list = /pub const NOT_IMPLEMENTED: \[&str; \d+\] = \[([^\]]*)\];/.exec(owner);
assert(list, 'NOT_IMPLEMENTED must stay parseable in the gui owner');
const capabilities = [...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
assert.deepEqual(capabilities, [
  'library_counts',
  'gallery',
  'search',
  'drop_zone',
  'model_settings',
  'bridge_runtime',
]);
// The window renders that owner list and keeps the explicit "not implemented" wording.
const appSource = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
assert(appSource.includes('notImplemented(report)'), 'App.tsx must render the owner capability list');
assert(appSource.includes('не реализовано на этом этапе'), 'App.tsx must keep the not-implemented wording');

console.log(
  `GUI parity PASS: build ${version[1]}, protocol ${version[2]}.${version[3]}, envelope ${tsEnvelope[1]}, ` +
    `${mappedStates.length} bridge states, ${capabilities.length} unfinished capabilities`,
);
