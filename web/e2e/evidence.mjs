import { readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { chromium } from '@playwright/test';

const digest = async path => createHash('sha256').update(await readFile(path)).digest('hex');
const evidenceDir = process.argv[3] ?? 'target/gui-browser';
let report;
try { report = JSON.parse(await readFile(`${evidenceDir}/results.json`,'utf8')); }
catch { report = {stats:{expected:0,skipped:0,unexpected:0,flaky:0},error:'Browser reporter produced no results; no test pass is established.'}; }
const assets = {};
for(const name of await readdir('web/dist/assets')) assets[name] = await digest(`web/dist/assets/${name}`);
const evidence = {
  sourceOid: execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),
  workingTree: execFileSync('git',['status','--short'],{encoding:'utf8'}),
  node: process.version,
  browserExecutable: chromium.executablePath(),
  browserVersion: execFileSync(chromium.executablePath(),['--version'],{encoding:'utf8'}).trim(),
  lockSha256: await digest('web/package-lock.json'),
  fixtureSha256: await digest('web/e2e/fixtures.mjs'),
  assets,
  exitStatus: Number(process.argv[2]),
  counts: report.stats,
  reporterError: report.error,
  report: 'results.json',
  scope: 'Synthetic loopback API/WebSocket transport serving production assets; no live provider, credentials, host admission or cleanup proof.',
};
await writeFile(`${evidenceDir}/evidence.json`,JSON.stringify(evidence,null,2)+'\n');
await writeFile('target/gui-browser/latest.json',JSON.stringify({evidenceDir,exitStatus:evidence.exitStatus,counts:evidence.counts},null,2)+'\n');
console.log(`Retained frontend browser evidence: ${evidenceDir}/evidence.json`);
