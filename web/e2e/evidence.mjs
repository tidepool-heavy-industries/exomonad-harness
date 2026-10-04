import { readFile, readdir, writeFile, stat } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { chromium } from '@playwright/test';
import { validateReport } from './report-validation.mjs';

const digest = async path => createHash('sha256').update(await readFile(path)).digest('hex');
const evidenceDir = process.argv[3] ?? 'target/gui-browser';
const testExitStatus = Number(process.argv[2]);
let report, reportError;
try { report = JSON.parse(await readFile(`${evidenceDir}/results.json`,'utf8')); }
catch (error) { reportError = `Browser reporter did not produce readable results: ${error.message}`; }
const validation = validateReport(report, process.env.HARNESS_BROWSER_RUN_ID);
const assetRoot = process.env.HARNESS_BROWSER_ASSETS ?? 'web/dist';
const bundle = JSON.parse(await readFile(`${assetRoot}/browser-bundle.json`, 'utf8'));
const assets = {};
for(const name of await readdir(`${assetRoot}/assets`)) assets[name] = await digest(`${assetRoot}/assets/${name}`);
const inputs = {};
for (const name of await readdir('web/e2e', {recursive:true})) {
  const path = `web/e2e/${name}`;
  if ((await stat(path)).isFile()) inputs[path] = await digest(path);
}
for (const path of ['web/playwright.config.ts','web/tsconfig.browser.json','scripts/verify-frontend-browser','flake.nix','flake.lock']) inputs[path] = await digest(path);
const exitStatus = testExitStatus !== 0 ? testExitStatus : validation.valid ? 0 : 1;
const evidence = {
  runId: process.env.HARNESS_BROWSER_RUN_ID,
  sourceOid: execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),
  workingTree: execFileSync('git',['status','--short'],{encoding:'utf8'}),
  node: {version:process.version,executable:process.execPath},
  browserExecutable: chromium.executablePath(),
  browserVersion: execFileSync(chromium.executablePath(),['--version'],{encoding:'utf8'}).trim(),
  lockSha256: await digest('web/package-lock.json'),
  fixtureSha256: await digest('web/e2e/fixtures.mjs'),
  inputs,
  bundle,
  assets,
  testExitStatus,
  exitStatus,
  counts: validation.counts,
  reporterStats: report?.stats,
  validation: {valid:validation.valid,reason:reportError ?? validation.reason},
  report: 'results.json',
  scope: 'Synthetic loopback API/WebSocket transport serving production assets; no live provider, credentials, host admission or cleanup proof.',
};
await writeFile(`${evidenceDir}/evidence.json`,JSON.stringify(evidence,null,2)+'\n');
await writeFile('target/gui-browser/latest.json',JSON.stringify({evidenceDir,exitStatus,counts:evidence.counts},null,2)+'\n');
console.log(`Retained frontend browser evidence: ${evidenceDir}/evidence.json`);
if(!validation.valid) console.error(reportError ?? validation.reason);
process.exitCode = exitStatus;
