/** Validate counted Playwright execution instead of trusting process exit alone. */
export function validateReport(report, expectedRunId) {
  const counts = { selected: 0, executed: 0, passed: 0, failed: 0, skipped: 0, attempts: 0 };
  const invalid = reason => ({ valid: false, reason, counts });
  if (!report || typeof report !== 'object' || !Array.isArray(report.suites) || !Array.isArray(report.errors)) return invalid('Missing or malformed Playwright report.');
  if (expectedRunId && report.config?.metadata?.runId !== expectedRunId) return invalid('Stale or unrelated browser report: run identity does not match.');
  if (!report.stats || ['expected','unexpected','flaky','skipped'].some(key => !Number.isSafeInteger(report.stats[key]) || report.stats[key] < 0)) return invalid('Invalid Playwright execution statistics.');
  try {
    const visit = suites => {
      for (const suite of suites) {
        if (!suite || !Array.isArray(suite.specs) || (suite.suites !== undefined && !Array.isArray(suite.suites))) throw new Error('Malformed Playwright suite.');
        for (const spec of suite.specs) {
          if (!spec || !Array.isArray(spec.tests)) throw new Error('Malformed Playwright test specification.');
          for (const test of spec.tests) {
            if (!test || !Array.isArray(test.results) || !['expected','unexpected','flaky','skipped'].includes(test.status)) throw new Error('Malformed Playwright test result.');
            counts.selected++;
            if (test.status === 'skipped') { counts.skipped++; continue; }
            if (test.results.length === 0) throw new Error('Selected browser case has no execution result.');
            for (const result of test.results) {
              if (!['passed','failed','timedOut','interrupted','skipped'].includes(result?.status)) throw new Error('Invalid browser execution status.');
              if (result.status !== 'skipped') counts.attempts++;
            }
            const last = test.results.at(-1);
            if (last.status === 'skipped') { counts.skipped++; continue; }
            counts.executed++;
            if (last.status === 'passed') counts.passed++;
            else counts.failed++;
          }
        }
        visit(suite.suites ?? []);
      }
    };
    visit(report.suites);
  } catch (error) { return invalid(error.message); }
  const total = report.stats.expected + report.stats.unexpected + report.stats.flaky + report.stats.skipped;
  if (total !== counts.selected) return invalid('Reported selection count does not match concrete browser cases.');
  if (report.stats.expected !== counts.passed || report.stats.skipped !== counts.skipped) return invalid('Reported outcomes do not match concrete browser execution.');
  if (counts.executed === 0) return invalid('No browser tests executed; no pass is established.');
  if (report.errors.length || counts.failed || report.stats.unexpected || report.stats.flaky) return invalid('Browser report contains failures, interruptions or flaky cases.');
  return { valid: true, counts };
}
