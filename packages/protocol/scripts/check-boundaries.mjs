import { readFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import ts from 'typescript';
const root = resolve(import.meta.dirname, '../../..');
const manifest = JSON.parse(await readFile(resolve(root, 'packages/protocol/package.json'), 'utf8'));
const rootManifest = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'));
const workspace = await readFile(resolve(root, 'pnpm-workspace.yaml'), 'utf8');
const deps = Object.keys(manifest.dependencies);
const devDeps = Object.keys(manifest.devDependencies);
// Explicit allowlist of workspace members: a new package must update this file and the
// verification catalog, never be added by a wildcard.
const workspaceEntries = [...workspace.matchAll(/^\s*-\s*'([^']+)'\s*$/gmu)].map(match => match[1]).join(',');
if (rootManifest.dependencies || rootManifest.devDependencies ||
    deps.sort().join(',') !== 'ajv,ajv-formats' ||
    devDeps.sort().join(',') !== 'json-schema-to-typescript,typescript' ||
    workspaceEntries !== 'packages/protocol,apps/gui') {
  throw new Error('protocol-only workspace dependency boundary changed');
}
for (const file of ['index.ts', ...(await readdir(resolve(root, 'packages/protocol/src/generated'))).map(name => `generated/${name}`)]) {
  if (!file.endsWith('.ts')) continue;
  const source = await readFile(resolve(root, 'packages/protocol/src', file), 'utf8');
  // Inspect actual syntax, not strings such as the wire method 'library.import'.
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const check = node => {
    let specifier;
    if (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) specifier = node.moduleSpecifier;
    else if (ts.isImportEqualsDeclaration(node) && ts.isExternalModuleReference(node.moduleReference)) specifier = node.moduleReference.expression;
    else if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument)) specifier = node.argument.literal;
    else if (ts.isCallExpression(node) && (node.expression.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(node.expression) && node.expression.text === 'require'))) {
      specifier = node.arguments[0];
      if (!specifier || !ts.isStringLiteral(specifier)) throw new Error(`computed runtime import in ${file}`);
    }
    if (specifier) {
      if (!ts.isStringLiteral(specifier)) throw new Error(`nonliteral import in ${file}`);
      const path = specifier.text;
      if (!['ajv/dist/2020.js', 'ajv-formats'].includes(path) && !path.startsWith('./generated/') && !path.startsWith('../../../schemas/')) throw new Error(`forbidden runtime import in ${file}`);
    }
    ts.forEachChild(node, check);
  };
  check(tree);
}
console.log('S01-TS-DEP-01 PASS: only protocol workspace, Ajv runtime and Rust-owned schemas; no SDK/host/IO imports');
