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
  './App',
  './model',
  './style.css',
  generatedTypes,
]);
const testImports = new Set([...applicationImports, 'vitest', 'node:assert/strict', 'node:fs', 'node:url']);
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
  for (const match of text.matchAll(/(?:from\s*|import\s*)['"]([^'"]+)['"]/g)) {
    assert(allowed.has(match[1]), `Forbidden import ${match[1]} in ${name}`);
    assert(!forbiddenModules.test(match[1]), `Forbidden dependency ${match[1]} in ${name}`);
    if (isTest) {
      assert(/^node:(assert\/strict|fs|url)$/.test(match[1]) || !match[1].startsWith('node:'), `Unexpected node builtin ${match[1]} in ${name}`);
    }
  }
  assert(!/\b(fetch|WebSocket|XMLHttpRequest|eval)\s*\(|import\s*\(/.test(text), `Forbidden transport in ${name}`);
  assert(!/require\s*\(/.test(text), `Forbidden require in ${name}`);
  if (isTest) continue;
  // Every handler call is argument-free: no path, executable, frame or secret can be sent
  // from the WebView, and an unknown handler name can never be reached.
  const calls = [...text.matchAll(/invoke(?:<[^>]*>)?\(\s*'([a-z_]+)'\s*\)/g)].map((match) => match[1]);
  for (const command of calls) invoked.add(command);
  for (const call of text.matchAll(/invoke(?:<[^>]*>)?\(([^)]*)\)/g)) {
    const argument = call[1].trim();
    assert(!argument.includes(','), `invoke in ${name} must take exactly one bare command name`);
    assert(/^'[a-z_]+'$/.test(argument) || argument === 'command', `unexpected invoke argument ${argument} in ${name}`);
  }
  const commandUnion = /type Command =([\s\S]*?);/.exec(text);
  if (commandUnion) {
    for (const named of commandUnion[1].matchAll(/'([a-z_]+)'/g)) invoked.add(named[1]);
  }
}
assert(invoked.size >= 3, 'the window must drive its typed commands');
for (const command of invoked) assert(registered.has(command), `Unknown tauri command ${command}`);

// The frontend never links the storage owner, the SDK or a capture binding.
assert(!/sm_storage|sm-storage|rusqlite/.test(repositoryFile('apps/gui/src/model.ts')));
console.log(`GUI boundaries PASS: ${invoked.size} registered command(s), React-only WebView, no SDK/SQLite/native import`);
