import { readFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';
const root = resolve(import.meta.dirname, '../../..');
const manifest = JSON.parse(await readFile(resolve(root, 'packages/protocol/package.json'), 'utf8'));
const rootManifest = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'));
const workspace = await readFile(resolve(root, 'pnpm-workspace.yaml'), 'utf8');
const deps = Object.keys(manifest.dependencies);
const devDeps = Object.keys(manifest.devDependencies);
if (rootManifest.dependencies || rootManifest.devDependencies ||
    deps.sort().join(',') !== 'ajv,ajv-formats' ||
    devDeps.sort().join(',') !== 'json-schema-to-typescript,typescript' ||
    !/^packages:\s*\n\s*- 'packages\/protocol'\s*$/u.test(workspace)) {
  throw new Error('protocol-only workspace dependency boundary changed');
}
for (const file of ['index.ts', ...(await readdir(resolve(root, 'packages/protocol/src/generated'))).map(name => `generated/${name}`)]) {
  if (!file.endsWith('.ts')) continue;
  const source = await readFile(resolve(root, 'packages/protocol/src', file), 'utf8');
  for (const specifier of source.matchAll(/\b(?:import|from)\s*['"]([^'"]+)['"]/gu)) {
    const path = specifier[1];
    if (!['ajv/dist/2020.js', 'ajv-formats'].includes(path) && !path.startsWith('./generated/') && !path.startsWith('../../../schemas/')) {
      throw new Error(`forbidden runtime import in ${file}`);
    }
  }
}
console.log('S01-TS-DEP-01 PASS: only protocol workspace, Ajv runtime and Rust-owned schemas; no SDK/host/IO imports');
