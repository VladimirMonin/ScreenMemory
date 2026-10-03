// Offline package assembly: no install, dependency execution, build hooks or downloads.
import { cpSync, mkdirSync, readFileSync, realpathSync, readdirSync, statSync, writeFileSync, existsSync } from 'node:fs';
import { resolve, dirname, join, isAbsolute, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
if (process.versions.node !== '24.13.0') throw new Error('PINNED_NODE_REQUIRED');
const source = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const destination = process.argv[2];
if (!destination || !isAbsolute(destination) || existsSync(destination)) throw new Error('NEW_ABSOLUTE_OUTPUT_REQUIRED');
mkdirSync(destination, { recursive: true });
cpSync(join(source, 'src'), join(destination, 'src'), { recursive: true });
cpSync(join(source, 'DEPENDENCIES.json'), join(destination, 'DEPENDENCIES.json'));
cpSync(join(source, 'THIRD-PARTY-NOTICES.md'), join(destination, 'THIRD-PARTY-NOTICES.md'));
cpSync(join(dirname(process.execPath), 'LICENSE'), join(destination, 'NODE-LICENSE'));
cpSync(process.execPath, join(destination, 'node.exe'));
writeFileSync(join(destination, 'package.json'), JSON.stringify({ private: true, type: 'module' }) + '\n');
const collected = new Map();
function collect(name, from) {
  const require = createRequire(join(from, 'package.json'));
  let dir = dirname(realpathSync(require.resolve(name)));
  while (!existsSync(join(dir, 'package.json')) || JSON.parse(readFileSync(join(dir, 'package.json'))).name !== name) {
    const parent = dirname(dir); if (parent === dir) throw new Error('PACKAGE_NOT_FOUND'); dir = parent;
  }
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json')));
  if (collected.has(name)) { if (collected.get(name) !== pkg.version) throw new Error('VERSION_COLLISION'); return; }
  collected.set(name, pkg.version);
  cpSync(dir, join(destination, 'node_modules', name), { recursive: true, filter: path => !relative(dir, path).split(/[\\/]/).includes('node_modules') });
  for (const dep of Object.keys(pkg.dependencies ?? {})) collect(dep, dir);
}
for (const name of Object.keys(JSON.parse(readFileSync(join(source, 'package.json'))).dependencies)) collect(name, source);
const files = {};
function scan(dir, prefix = '') {
  for (const leaf of readdirSync(dir).sort()) {
    const path = join(dir, leaf), relative = prefix + leaf;
    if (statSync(path).isDirectory()) scan(path, relative + '/');
    else files[relative] = createHash('sha256').update(readFileSync(path)).digest('hex');
  }
}
scan(destination);
writeFileSync(join(destination, 'package-manifest.json'), JSON.stringify({ protocol:'sm-sdk/1.1', node_version:'24.13.0', sdk_version:'2.0.0', files }, null, 2) + '\n');
process.stdout.write(`STAGED ${collected.size} packages; private Node and SDK; offline\n`);
