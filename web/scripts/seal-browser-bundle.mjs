import { createHash } from 'node:crypto';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const [root, identityPath, extra] = process.argv.slice(2);
if (!root || !identityPath || extra) throw new Error('Expected dist directory and declared contract manifest.');
const identity = JSON.parse(await readFile(identityPath, 'utf8'));
if (identity.version !== 1 || !/^[0-9a-f]{64}$/.test(identity.runtimeSourceSha256)
  || !/^[0-9a-f]{64}$/.test(identity.schemaSha256)) throw new Error('Invalid declared bundle identity.');
const assets = {};
async function inventory(directory, prefix = '') {
  const entries = await readdir(directory, { withFileTypes: true });
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name, 'en'))) {
    const path = prefix + entry.name;
    if (path === 'browser-bundle.json') throw new Error('Refusing an already sealed or stale dist directory.');
    if (entry.isDirectory()) await inventory(join(directory, entry.name), path + '/');
    else if (entry.isFile()) assets[path] = createHash('sha256').update(await readFile(join(directory, entry.name))).digest('hex');
    else throw new Error(`Unsupported asset ${path}.`);
  }
}
await inventory(root);
if (!assets['index.html']) throw new Error('Missing browser entry point.');
await writeFile(join(root, 'browser-bundle.json'), JSON.stringify({
  version: 1,
  identity: { runtimeSourceSha256: identity.runtimeSourceSha256, schemaSha256: identity.schemaSha256 },
  assets,
}, null, 2) + '\n');
