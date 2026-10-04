import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import Ajv from 'ajv';
import standaloneCode from 'ajv/dist/standalone/index.js';
import { compile } from 'json-schema-to-typescript';

const [source, output, sourceIdentity, extra] = process.argv.slice(2);
if (!source || !output || !sourceIdentity || extra) throw new Error('Expected Rust contract directory, output directory and declared source identity.');
const runtimeIdentity = JSON.parse(await readFile(sourceIdentity, 'utf8'));
if (!/^[0-9a-f]{64}$/.test(runtimeIdentity.runtimeSourceSha256)) throw new Error('Invalid declared source identity.');
const encodedSchemas = await readFile(join(source, 'schemas.json'));
const schemas = JSON.parse(encodedSchemas);
const samples = JSON.parse(await readFile(join(source, 'wire-samples.json'), 'utf8'));
const roots = [
  ['server', 'server', 'ServerFrame', 'isServerFrame'],
  ['serverEvent', 'server-event', 'ServerEvent', 'isServerEvent'],
  ['client', 'client', 'ClientFrame', 'isClientFrame'],
  ['actorOutputHistory', 'actor-output-history', 'ActorOutputHistoryPage', 'isActorOutputHistoryPage'],
  ['actorDisplayExpansion', 'actor-display-expansion', 'ActorDisplayExpansion', 'isActorDisplayExpansion'],
  ['history', 'history', 'HistoryPage', 'isHistoryPage'],
  ['embeddedCommand', 'embedded-command', 'EmbeddedCommandRecord', 'isEmbeddedCommandRecord'],
];
const ajv = new Ajv({ strict: true, allErrors: true, code: { source: true, esm: true } });
const exports = {};
const declarations = [];
await mkdir(output, { recursive: true });
for (const [key, filename, type, validator] of roots) {
  const schema = schemas[key];
  if (!schema || schema.title !== type) throw new Error(`Missing Rust schema ${type}.`);
  const id = `urn:harness:browser:${key}`;
  ajv.addSchema({ ...schema, $id: id }, id);
  const validate = ajv.getSchema(id);
  if (!Array.isArray(samples[key]) || !samples[key].length) throw new Error(`Missing Rust wire samples ${key}.`);
  for (const sample of samples[key]) {
    if (!validate(sample)) throw new Error(`${key}: ${ajv.errorsText(validate.errors)}`);
  }
  exports[validator] = id;
  const types = await compile(schema, type, {
    bannerComment: '/* Generated from Rust browser projections. */',
    unknownAny: true,
    enableConstEnums: false,
    unreachableDefinitions: true,
  });
  await writeFile(join(output, `${filename}.d.ts`), types);
  declarations.push(`export function ${validator}(value: unknown): value is import('./${filename}').${type};`);
}
await writeFile(join(output, 'validators.mjs'), standaloneCode(ajv, exports));
await writeFile(join(output, 'validators.d.mts'), declarations.join('\n') + '\n');
await writeFile(join(output, 'wire-samples.json'), JSON.stringify(samples, null, 2) + '\n');
await writeFile(join(output, 'contract-manifest.json'), JSON.stringify({
  version: 1,
  runtimeSourceSha256: runtimeIdentity.runtimeSourceSha256,
  schemaSha256: createHash('sha256').update(encodedSchemas).digest('hex'),
  generators: { schemars: '1.2.2', jsonSchemaToTypescript: '16.0.0', ajv: '8.17.1' },
}, null, 2) + '\n');
