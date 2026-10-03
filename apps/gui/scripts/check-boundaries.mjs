// S01-T06d frontend boundary: the WebView bundle may only reach React, the narrow Tauri
// command API and the generated protocol *types*; it never imports an SDK, SQLite, a native
// capture binding, a node builtin or any transport of its own, and it can only name
// handlers that really exist in `crates/gui/src/shell.rs`.
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const repositoryFile = (relative) => readFileSync(fileURLToPath(new URL(`../../../${relative}`, import.meta.url)), 'utf8');
const generatedTypes = '../../../packages/protocol/src/generated/application-v1';
const applicationImports = new Set([
  'react',
  'react-dom/client',
  '@tauri-apps/api/core',
  '@tauri-apps/api/event',
  './ImportPanel',
  './ExportPanel',
  './ProcessingPanel',
  './KnowledgePanel',
  '../../../packages/protocol/src/generated/application-methods-v1',
  './App',
  './model',
  './style.css',
  generatedTypes,
]);
const testImports = new Set([...applicationImports, 'vitest', 'node:assert/strict', 'node:fs', 'node:url', 'react-dom/server']);
const forbiddenModules = /(@lmstudio\/sdk|lmstudio|better-sqlite3|sqlite|child_process|ipcRenderer|node:net|node:worker_threads)/i;

// The typed commands the Rust shell really registers; a rename in either direction fails here.
const shell = repositoryFile('crates/gui/src/shell.rs');
const registered = new Set([...shell.matchAll(/#\[tauri::command\]\s*pub async fn (\w+)\(/g)].map((match) => match[1]));
assert(registered.size >= 5, 'the gui owner must register its typed commands');
const invoked = new Set();

for (const name of readdirSync(new URL('../src/', import.meta.url))) {
  if (!/\.tsx?$/.test(name)) continue;
  const text = readFileSync(new URL(`../src/${name}`, import.meta.url), 'utf8');
  const isTest = /\.test\.tsx?$/.test(name);
  const allowed = isTest ? testImports : applicationImports;
  for (const match of text.matchAll(/\b(?:from\s+|import\s*)['"]([^'"]+)['"]/g)) {
    assert(allowed.has(match[1]), `Forbidden import ${match[1]} in ${name}`);
    assert(!forbiddenModules.test(match[1]), `Forbidden dependency ${match[1]} in ${name}`);
    if (isTest) {
      assert(/^node:(assert\/strict|fs|url)$/.test(match[1]) || !match[1].startsWith('node:'), `Unexpected node builtin ${match[1]} in ${name}`);
    }
  }
  assert(!/\b(fetch|WebSocket|XMLHttpRequest|eval)\s*\(|import\s*\(/.test(text), `Forbidden transport in ${name}`);
  assert(!/require\s*\(/.test(text), `Forbidden require in ${name}`);
  if (isTest) continue;
  // All handlers are argument-free except one typed, canonical UUID assetId.
  // A caller cannot supply a path, command name, frame or transport parameter.
  const previewCall = "invoke<AssetPreview>('gui_asset_preview', { assetId: assetId })";
  if (text.includes("'gui_asset_preview'")) {
    assert.equal(text.split("'gui_asset_preview'").length, 2, 'one fixed preview handler call');
    assert(text.includes(previewCall), 'preview may send only the typed assetId');
    assert(text.includes('canonicalAssetId(item.preview_asset_id)'), 'validate asset identity before invocation');
    invoked.add('gui_asset_preview');
  }
  const withoutPreview = text.replace(previewCall, '');
  const calls = [...withoutPreview.matchAll(/invoke(?:<[^>]*>)?\(\s*'([a-z_]+)'\s*\)/g)].map((match) => match[1]);
  for (const command of calls) invoked.add(command);
  for (const call of withoutPreview.matchAll(/invoke(?:<[^>]*>)?\(([^)]*)\)/g)) {
    const argument = call[1].trim();
    const typed = /^'(gui_library_page|gui_import|gui_import_report|gui_import_control|gui_export|gui_metadata|gui_processing|gui_knowledge)',\s*\{\s*(cursor|request)\b/.exec(argument);
    if (typed) {
      assert.equal(typed[2], typed[1] === 'gui_library_page' ? 'cursor' : 'request', 'closed typed argument role');
      invoked.add(typed[1]);
      continue;
    }
    assert(!argument.includes(','), `invoke in ${name} must take exactly one bare command name`);
    // The only accepted dynamic form is the closed capture table of model.ts.
    assert(
      /^'[a-z_]+'$/.test(argument) || argument === 'command' || argument === 'captureCommand[mode]',
      `unexpected invoke argument ${argument} in ${name}`,
    );
  }
  const commandUnion = /type Command =([\s\S]*?);/.exec(text);
  if (commandUnion) {
    for (const named of commandUnion[1].matchAll(/'([a-z_]+)'/g)) invoked.add(named[1]);
  }
}
// The capture table of the renderer must map exactly the closed mode vocabulary onto the
// registered argument-free handlers.
const guiModel = repositoryFile('apps/gui/src/model.ts');
const table = /export const captureCommand: Record<CaptureMode, string> = \{([^}]*)\};/.exec(guiModel);
assert(table, 'captureCommand must stay parseable in the renderer model');
const pairs = [...table[1].matchAll(/(\w+):\s*'([a-z_]+)'/g)].map((match) => [match[1], match[2]]);
assert.deepEqual(pairs, [
  ['region', 'gui_capture_region'],
  ['window', 'gui_capture_window'],
  ['monitor', 'gui_capture_monitor'],
  ['desktop', 'gui_capture_desktop'],
]);
assert(invoked.size >= 3, 'the window must drive its typed commands');
for (const command of invoked) assert(registered.has(command), `Unknown tauri command ${command}`);

// The preview's local Blob URL must load in the actual Tauri window; only image
// sources gain blob:, never scripts, connections or the default directive.
const csp = JSON.parse(repositoryFile('crates/gui/tauri.conf.json')).app.security.csp;
const directives = Object.fromEntries(csp.split(';').map((part) => part.trim().split(/\s+/)).map(([name, ...values]) => [name, values]));
assert.deepEqual(directives['img-src'], ["'self'", 'data:', 'blob:'], 'only local image Blob URLs');
for (const name of ['default-src', 'connect-src', 'script-src', 'style-src']) {
  assert(!directives[name]?.includes('blob:'), `${name} must not permit blob:`);
}

// The frontend never links the storage owner, the SDK or a capture binding.
assert(!/sm_storage|sm-storage|rusqlite/.test(repositoryFile('apps/gui/src/model.ts')));
console.log(`GUI boundaries PASS: ${invoked.size} registered command(s), React-only WebView, no SDK/SQLite/native import`);
