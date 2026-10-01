import { test } from 'node:test';
import assert from 'node:assert/strict';
import { validateReport } from './report-validation.mjs';
const report = () => ({config:{metadata:{runId:'current'}},errors:[],stats:{expected:1,unexpected:0,flaky:0,skipped:0},suites:[{specs:[{tests:[{status:'expected',results:[{status:'passed'}]}]}],suites:[]}]});

test('concrete successful execution is counted',()=>{assert.deepEqual(validateReport(report(),'current'),{valid:true,counts:{selected:1,executed:1,passed:1,failed:0,skipped:0,attempts:1}})});
test('missing report is never a pass',()=>assert.equal(validateReport(undefined,'current').valid,false));
test('zero and skipped-only execution are rejected',()=>{
  const zero=report();zero.suites=[];zero.stats.expected=0;assert.equal(validateReport(zero,'current').valid,false);
  const skipped=report();skipped.stats={expected:0,unexpected:0,flaky:0,skipped:1};skipped.suites[0].specs[0].tests[0]={status:'skipped',results:[]};assert.equal(validateReport(skipped,'current').valid,false);
});
test('a stale otherwise passing report is rejected',()=>assert.match(validateReport(report(),'different-run').reason,/Stale/));
test('stats cannot fabricate passing execution or conceal failed results',()=>{
  const fake=report();fake.suites=[];assert.equal(validateReport(fake,'current').valid,false);
  const failed=report();failed.suites[0].specs[0].tests[0].results[0].status='failed';assert.equal(validateReport(failed,'current').valid,false);
});
test('malformed result and missing execution remain diagnostics',()=>{
  const bad=report();bad.suites[0].specs[0].tests[0].results=[{status:'invented'}];assert.equal(validateReport(bad,'current').valid,false);
  bad.suites[0].specs[0].tests[0].results=[];assert.equal(validateReport(bad,'current').valid,false);
});

test('evidence recorder rejects missing, zero and stale reports and preserves a nonzero browser exit', async () => {
  const { mkdtemp, mkdir, writeFile, readFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { resolve, join } = await import('node:path');
  const { execFileSync, spawnSync } = await import('node:child_process');
  const recorder = resolve(import.meta.dirname, 'evidence.mjs');
  const scratch = await mkdtemp(join(tmpdir(), 'harness-report-contract-'));
  try {
    for (const directory of ['web/dist/assets','web/e2e','scripts','target/gui-browser']) await mkdir(join(scratch,directory),{recursive:true});
    for (const path of ['web/dist/assets/fixture.js','web/e2e/fixtures.mjs','web/package-lock.json','web/playwright.config.ts','web/tsconfig.browser.json','scripts/verify-frontend-browser','flake.nix','flake.lock']) await writeFile(join(scratch,path),'Synthetic evidence-consumer test input\n');
    execFileSync('git',['init','--quiet',scratch]);
    execFileSync('git',['-C',scratch,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--quiet','--allow-empty','-m','Evidence consumer fixture']);
    const empty=report();empty.suites=[];empty.stats.expected=0;
    const cases=[['missing',undefined,0,1],['zero',empty,0,1],['stale',report(),0,1],['successful',report(),0,0],['original-failure',report(),7,7]];
    for (const [name, value, browserExit, expectedExit] of cases) {
      const directory=join(scratch,'target/gui-browser',name);
      await mkdir(directory);
      if(value) await writeFile(join(directory,'results.json'),JSON.stringify(value));
      const runId=name==='stale'?'different-run':'current';
      const result=spawnSync(process.execPath,[recorder,String(browserExit),directory],{cwd:scratch,env:{...process.env,HARNESS_BROWSER_RUN_ID:runId},encoding:'utf8'});
      assert.equal(result.status,expectedExit,result.stderr);
      const evidence=JSON.parse(await readFile(join(directory,'evidence.json'),'utf8'));
      assert.equal(evidence.exitStatus,expectedExit);
      assert.equal(evidence.testExitStatus,browserExit);
      if(name==='missing'||name==='zero'||name==='stale') assert.equal(evidence.validation.valid,false);
      assert.ok(evidence.inputs['web/e2e/fixtures.mjs']);
    }
  } finally { await rm(scratch,{recursive:true,force:true}); }
});
