import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name('buck-focused-test')


class BuckFocusedTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'scripts').mkdir()
        shutil.copy(SCRIPT, self.root / 'scripts/buck-focused-test')
        (self.root / 'crates/harness').mkdir(parents=True)
        (self.root / 'target').mkdir()
        subprocess.run(['git', 'init', '-q'], cwd=self.root, check=True)
        subprocess.run(['git', 'config', 'user.email', 'focused@example.invalid'], cwd=self.root, check=True)
        subprocess.run(['git', 'config', 'user.name', 'Focused test'], cwd=self.root, check=True)
        (self.root / 'tracked').write_text('baseline')
        subprocess.run(['git', 'add', 'tracked'], cwd=self.root, check=True)
        subprocess.run(['git', 'commit', '-qm', 'baseline'], cwd=self.root, check=True)
        self.bin_dir = self.root / 'bin'
        self.bin_dir.mkdir()
        self.executable = self.root / 'libtest'
        self.program(self.executable, '''
if [ "$1" = "--filter" ]; then shift; fi
if [ "${2:-}" = "--list" ]; then
  if [ "${3:-}" = "--ignored" ]; then printf '%s' "$IGNORED_LIST";
  else printf '%s' "$LISTING"; fi
  exit 0
fi
printf '%s\\n' "$RESULT"
exit "$RUN_EXIT"
''')
        self.buck = self.bin_dir / 'buck2'
        self.program(self.buck, '''
printf '%s\\n' "$@" > "$BUCK_ARGV"
if [ "${1:-}" = build ]; then
  if [ "$BUILD_EXIT" != 0 ]; then echo build-failed >&2; exit "$BUILD_EXIT"; fi
  printf '%s\\n' "$TEST_EXECUTABLE"
fi
''')
        self.env = dict(os.environ, BUCK_ARGV=str(self.root / 'buck-argv'),
                        TEST_EXECUTABLE=str(self.executable), BUILD_EXIT='0',
                        LISTING='selected_case: test\n',
                        IGNORED_LIST='',
                        RESULT='test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s',
                        RUN_EXIT='0')

    @staticmethod
    def program(path, body):
        path.write_text('#!/usr/bin/env bash\nset -eu\n' + body + '\n')
        path.chmod(0o755)

    def invoke(self, **extra):
        env = dict(self.env)
        env.update(extra)
        env.setdefault('PATH', str(self.bin_dir) + ':' + os.environ['PATH'])
        result = subprocess.run(
            ['python3', str(self.root / 'scripts/buck-focused-test'),
             '--target', '//crates/harness:unit_tests', '--filter', 'selected_case', '--expect', '1'],
            cwd=self.root, env=env, capture_output=True, text=True,
        )
        marker = 'focused test evidence: '
        evidence_path = next((Path(line.removeprefix(marker)) for line in result.stderr.splitlines()
                              if line.startswith(marker)), None)
        return result, evidence_path

    def test_explicit_override_runs_and_retains_complete_evidence(self):
        (self.root / 'dirty').write_text('working change')
        result, path = self.invoke(BUCK2=str(self.buck))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIsNotNone(path)
        evidence = json.loads(path.read_text())
        self.assertTrue(evidence['source_before'])
        self.assertIn('?? dirty', evidence['working_tree_status_before'])
        self.assertEqual(evidence['buck2'], str(self.buck))
        self.assertEqual(evidence['selected_count'], 1)
        self.assertEqual(evidence['matched_count'], 1)
        self.assertEqual(evidence['executed_count'], 1)
        self.assertEqual(evidence['exit_code'], 0)
        self.assertEqual(len(evidence['sha256']), 64)
        self.assertEqual(evidence['build_argv'][0], str(self.buck))
        self.assertEqual(evidence['list_argv'][-1], '--list')
        self.assertEqual(evidence['run_argv'][-1], '--nocapture')
        for log in ('build.log', 'list.log', 'run.log'):
            self.assertTrue((path.parent / log).is_file())
        self.assertTrue((path.parent / 'ignored.log').is_file())
        self.assertEqual((self.root / 'buck-argv').read_text().splitlines()[0], 'build')

    def test_default_uses_pinned_output_instead_of_ambient_buck(self):
        ambient_dir = self.root / 'ambient'
        ambient_dir.mkdir()
        ambient = ambient_dir / 'buck2'
        self.program(ambient, 'exit 99')
        pinned = self.root / 'nix-store/buck2/bin/buck2'
        pinned.parent.mkdir(parents=True)
        shutil.copy(self.buck, pinned)
        nix = self.bin_dir / 'nix'
        self.program(nix, '''
case "$*" in
  *builtins.currentSystem*) echo 'warning: dirty flake source' >&2; printf '%s\\n' x86_64-linux ;;
  *packages.x86_64-linux.buck2.outPath*) echo 'warning: dirty flake source' >&2; printf '%s\\n' "$PINNED_OUTPUT" ;;
  *) exit 98 ;;
esac
''')
        result, path = self.invoke(BUCK2='', PATH=str(ambient_dir) + ':' + str(self.bin_dir) + ':' + os.environ['PATH'],
                                   PINNED_OUTPUT=str(pinned.parents[1]))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(path.read_text())['buck2'], str(pinned))

    def test_missing_pinned_output_records_materialization_prerequisite(self):
        ambient_dir = self.root / 'ambient'
        ambient_dir.mkdir()
        self.program(ambient_dir / 'buck2', 'exit 99')
        nix = self.bin_dir / 'nix'
        self.program(nix, '''
case "$*" in
  *builtins.currentSystem*) printf '%s\\n' x86_64-linux ;;
  *packages.x86_64-linux.buck2.outPath*) printf '%s\\n' "$PINNED_OUTPUT" ;;
  *) exit 98 ;;
esac
''')
        missing = self.root / 'unmaterialized-buck2'
        result, path = self.invoke(BUCK2='',
                                   PATH=str(ambient_dir) + ':' + str(self.bin_dir) + ':' + os.environ['PATH'],
                                   PINNED_OUTPUT=str(missing))
        self.assertEqual(result.returncode, 1)
        self.assertIsNotNone(path)
        evidence = json.loads(path.read_text())
        self.assertEqual(evidence['phase'], 'buck2_unavailable')
        self.assertEqual(evidence['buck2_resolution_source'], 'pinned_flake')
        self.assertEqual(evidence['buck2'], str(missing / 'bin/buck2'))
        self.assertIn('nix build .#packages.<system>.buck2 --no-link', evidence['resolution_error'])
        self.assertEqual(evidence['exit_code'], 1)
        self.assertIn('not materialized', (path.parent / 'buck2-resolution.log').read_text())
        self.assertFalse((self.root / 'buck-argv').exists())

    def test_zero_selection_wrong_count_and_run_failure_are_rejected_with_logs(self):
        cases = [
            ({'LISTING': 'selected_case: test\n',
              'IGNORED_LIST': 'selected_case: test\n'}, 'selection_rejected', 1),
            ({'LISTING': 'selected_case: test\nother_case: test\n'}, 'selection_rejected', 1),
            ({'RUN_EXIT': '7'}, 'execution', 7),
        ]
        for overrides, phase, code in cases:
            with self.subTest(overrides=overrides):
                result, path = self.invoke(BUCK2=str(self.buck), **overrides)
                self.assertEqual(result.returncode, code, result.stderr)
                evidence = json.loads(path.read_text())
                self.assertEqual(evidence['phase'], phase)
                self.assertEqual(evidence['exit_code'], code)
                self.assertTrue((path.parent / 'build.log').exists())
                if phase != 'selection_rejected' or overrides.get('LISTING'):
                    self.assertTrue((path.parent / 'list.log').exists())
                if phase == 'execution':
                    self.assertEqual(evidence['executed_count'], 1)
                    self.assertTrue((path.parent / 'run.log').exists())

    def test_build_failure_exit_and_diagnostic_are_retained(self):
        result, path = self.invoke(BUCK2=str(self.buck), BUILD_EXIT='6')
        self.assertEqual(result.returncode, 6)
        evidence = json.loads(path.read_text())
        self.assertEqual(evidence['build_exit_code'], 6)
        self.assertEqual(evidence['exit_code'], 6)
        self.assertIn('build-failed', (path.parent / 'build.log').read_text())

    def test_pinned_resolution_failure_retains_nix_diagnostic(self):
        ambient_dir = self.root / 'ambient'
        ambient_dir.mkdir()
        ambient = ambient_dir / 'buck2'
        self.program(ambient, 'exit 99')
        nix = self.bin_dir / 'nix'
        self.program(nix, 'echo "nix resolution diagnostic" >&2\nexit 23')
        result, path = self.invoke(BUCK2='', PATH=str(ambient_dir) + ':' +
                                   str(self.bin_dir) + ':' + os.environ['PATH'])
        self.assertEqual(result.returncode, 1, result.stderr)
        evidence = json.loads(path.read_text())
        self.assertEqual(evidence['phase'], 'buck2_resolution_failed')
        self.assertIn('nix resolution diagnostic', evidence['resolution_error'])
        self.assertIn('nix resolution diagnostic',
                      (path.parent / 'buck2-resolution.log').read_text())


if __name__ == '__main__':
    unittest.main()
