from __future__ import annotations

import os
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("cargo-focused-test")


class FocusedTestRunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.bin_dir = Path(self.temp.name)
        self.stub = self.bin_dir / "cargo"
        self.stub.write_text(
            "#!/usr/bin/env python3\n"
            "import os, sys\n"
            "args = sys.argv[1:]\n"
            "if '--bin' in args and args[args.index('--bin') + 1] == 'harness-demo':\n"
            "    print('BIN_TARGET_SELECTED')\n"
            "if '--list' in args:\n"
            "    mode = os.environ['STUB_MODE']\n"
            "    if mode == 'replace':\n"
            "        open(os.environ['ORIGINAL_ARTIFACT'], 'w').write('#!/bin/sh\\nexit 23\\n')\n"
            "    if '--ignored' in args:\n"
            "        print('only_ignored: test' if mode == 'ignored' else '')\n"
            "    elif mode == 'zero':\n"
            "        print('0 tests, 0 benchmarks')\n"
            "    elif mode == 'ignored':\n"
            "        print('only_ignored: test')\n"
            "    else:\n"
            "        print('module::focused_case: test')\n"
            "        print('1 test, 0 benchmarks')\n"
            "else:\n"
            "    print('EXECUTED')\n"
            "    mode = os.environ['STUB_MODE']\n"
            "    if mode == 'signal': os.kill(os.getpid(), 15)\n"
            "    if mode == 'runzero':\n"
            "        print('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s')\n"
            "    elif mode == 'runtwo':\n"
            "        print('test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s')\n"
            "    elif mode != 'missing' and int(os.environ.get('STUB_EXIT', '0')) != 0:\n"
            "        print('test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s')\n"
            "    elif mode != 'missing':\n"
            "        print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s')\n"
            "    sys.exit(int(os.environ.get('STUB_EXIT', '0')))\n"
        )
        self.stub.chmod(0o755)
        artifact = self.bin_dir / "test-artifact"
        self.stub.rename(artifact)
        self.stub.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, sys\n"
            "base = pathlib.Path(__file__).parent\n"
            "with (base / 'cargo-calls').open('a') as log: log.write(' '.join(sys.argv[1:]) + '\\n')\n"
            "if sys.argv[1] == 'metadata':\n"
            "    if os.environ.get('STUB_MODE') == 'metadata_failure':\n"
            "        print('metadata unavailable', file=sys.stderr); sys.exit(11)\n"
            "    print(json.dumps({'packages': [{'name': 'harness', 'id': 'pkg', 'version': '0.1.0', 'manifest_path': str(base / 'Cargo.toml')}]}))\n"
            "else:\n"
            "    if os.environ.get('STUB_MODE') in ('build_failure', 'build_long'):\n"
            "        diagnostic = 'error[E0308]: mismatched types\\n --> crates/demo/src/lib.rs:12:3\\n'\n"
            "        if os.environ.get('STUB_MODE') == 'build_long': diagnostic = 'A' * 10000 + diagnostic\n"
            "        print(json.dumps({'reason': 'compiler-message', 'message': {'level': 'error', 'rendered': diagnostic}}))\n"
            "        sys.exit(17)\n"
            "    args = sys.argv[1:]\n"
            "    kind = 'bin' if '--bin' in args else 'lib'\n"
            "    name = args[args.index('--bin') + 1] if kind == 'bin' else 'harness'\n"
            "    if kind == 'bin': print(json.dumps({'reason': 'compiler-message', 'message': {'rendered': 'BIN_TARGET_SELECTED\\n'}}))\n"
            "    print(json.dumps({'reason': 'compiler-artifact', 'package_id': 'pkg', 'profile': {'test': True}, 'target': {'kind': [kind], 'name': name}, 'executable': str(base / 'test-artifact')}))\n"
        )
        self.stub.chmod(0o755)
        git = self.bin_dir / "git"
        git.write_text(
            "#!/usr/bin/env python3\n"
            "import os, pathlib, sys\n"
            "base = pathlib.Path(__file__).parent\n"
            "if sys.argv[1:3] == ['rev-parse', 'HEAD']:\n"
            "    path = base / 'source-reads'\n"
            "    count = int(path.read_text()) + 1 if path.exists() else 1\n"
            "    path.write_text(str(count))\n"
            "    print('after' if os.environ.get('STUB_MODE') == 'drift' and count > 1 else 'before')\n"
            "elif sys.argv[1:3] == ['status', '--porcelain']:\n"
            "    pass\n"
        )
        git.chmod(0o755)
        rustc = self.bin_dir / "rustc"
        rustc.write_text("#!/bin/sh\nprintf '%s\\n' /tmp\n")
        rustc.chmod(0o755)

    def invoke(
        self, mode: str, exit_code: int = 0, target: str = "lib", expected: int | None = None
    ) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env["PATH"] = f"{self.bin_dir}{os.pathsep}{env['PATH']}"
        env["ORIGINAL_ARTIFACT"] = str(self.bin_dir / "test-artifact")
        env["STUB_MODE"] = mode
        env["STUB_EXIT"] = str(exit_code)
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--package",
                "harness",
                "--target",
                target,
                "--filter",
                "focused_case",
                *([] if expected is None else ["--expect", str(expected)]),
            ],
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )

    def test_zero_match_refuses_without_running_tests(self) -> None:
        result = self.invoke("zero")
        self.assertEqual(result.returncode, 1)
        self.assertIn("0 matched, 0 runnable", result.stderr)
        self.assertNotIn("EXECUTED", result.stdout)

    def test_ignored_only_selection_refuses_without_running_tests(self) -> None:
        result = self.invoke("ignored")
        self.assertEqual(result.returncode, 1)
        self.assertIn("1 matched, 0 runnable, 1 ignored", result.stderr)
        self.assertNotIn("EXECUTED", result.stdout)

    def test_runnable_match_executes_and_preserves_success(self) -> None:
        result = self.invoke("pass")
        self.assertEqual(result.returncode, 0)
        self.assertIn("1 matched, 1 runnable, 0 ignored", result.stderr)
        self.assertIn("1 executed, 1 passed", result.stderr)
        self.assertIn("EXECUTED", result.stdout)
        calls = (self.bin_dir / "cargo-calls").read_text().splitlines()
        self.assertEqual(len(calls), 2)
        self.assertIn("--no-run", calls[1])
        self.assertIn("focused test evidence:", result.stderr)

    def test_expected_count_is_enforced_before_execution(self) -> None:
        result = self.invoke("pass", expected=2)
        self.assertEqual(result.returncode, 1)
        self.assertIn("expected 2 runnable, selected 1", result.stderr)
        self.assertNotIn("EXECUTED", result.stdout)

    def test_matching_expected_count_executes(self) -> None:
        self.assertEqual(self.invoke("pass", expected=1).returncode, 0)

    def test_replacement_of_build_artifact_does_not_change_selected_executable(self) -> None:
        result = self.invoke("replace", expected=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1 executed, 1 passed", result.stderr)

    def test_signal_termination_is_preserved(self) -> None:
        self.assertEqual(self.invoke("signal").returncode, -15)

    def test_execution_count_must_match_selection(self) -> None:
        result = self.invoke("runtwo")
        self.assertEqual(result.returncode, 1)
        self.assertIn("selected 1 runnable but executed 2", result.stderr)

    def test_test_failure_exit_is_preserved(self) -> None:
        result = self.invoke("pass", exit_code=7)
        self.assertEqual(result.returncode, 7)
        self.assertIn("EXECUTED", result.stdout)
        self.assertIn("1 executed, 0 passed, 1 failed", result.stderr)

    def test_successful_cargo_exit_with_zero_executed_is_rejected(self) -> None:
        result = self.invoke("runzero")
        self.assertEqual(result.returncode, 1)
        self.assertIn("0 executed", result.stderr)
        self.assertIn("EXECUTED", result.stdout)

    def test_successful_cargo_exit_without_result_summary_is_rejected(self) -> None:
        result = self.invoke("missing")
        self.assertEqual(result.returncode, 1)
        self.assertIn("without a libtest result", result.stderr)

    def test_filter_cannot_be_a_cargo_option(self) -> None:
        env = os.environ.copy()
        env["PATH"] = f"{self.bin_dir}{os.pathsep}{env['PATH']}"
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--package",
                "harness",
                "--target",
                "lib",
                "--filter=--list",
            ],
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("must not start with `-`", result.stderr)

    def test_binary_target_is_forwarded_explicitly(self) -> None:
        result = self.invoke("pass", target="bin:harness-demo")
        self.assertEqual(result.returncode, 0)
        self.assertIn("BIN_TARGET_SELECTED", result.stderr)
        self.assertIn("1 executed, 1 passed", result.stderr)

    def record(self, result: subprocess.CompletedProcess[str]) -> dict:
        marker = "focused test evidence: "
        path = next(line[len(marker):] for line in reversed(result.stderr.splitlines())
                    if line.startswith(marker))
        return json.loads(Path(path).read_text())

    def test_metadata_failure_retains_phase_without_test_counts(self) -> None:
        result = self.invoke("metadata_failure")
        record = self.record(result)
        self.assertEqual(result.returncode, 11)
        self.assertEqual(record["phase"], "metadata")
        self.assertEqual(record["metadata_exit_code"], 11)
        self.assertIn("metadata unavailable", record["metadata_diagnostic"])
        self.assertNotIn("build_exit_code", record)
        self.assertNotIn("runnable", record)

    def test_compiler_failure_retains_diagnostic_without_fake_executable(self) -> None:
        result = self.invoke("build_failure")
        record = self.record(result)
        self.assertEqual(result.returncode, 17)
        self.assertEqual(record["phase"], "build")
        self.assertEqual(record["build_exit_code"], 17)
        self.assertEqual(record["compiler_error_count"], 1)
        self.assertIn("error[E0308]", record["build_diagnostic"])
        self.assertIsNone(record["executable"])
        self.assertIsNone(record["output"])
        self.assertNotIn("matched", record)
        self.assertNotIn("summaries", record)

    def test_source_drift_is_visible_after_execution(self) -> None:
        record = self.record(self.invoke("drift"))
        self.assertEqual(record["source_before"], "before")
        self.assertEqual(record["source_after"], "after")

    def test_truncated_compiler_diagnostic_names_full_log(self) -> None:
        record = self.record(self.invoke("build_long"))
        self.assertEqual(record["compiler_error_count"], 1)
        self.assertTrue(record["build_diagnostic"].startswith("[earlier diagnostic omitted"))
        self.assertIn("build.log", record["build_diagnostic"])
        self.assertIn("error[E0308]", record["build_diagnostic"])
        self.assertLessEqual(len(record["build_diagnostic"]), 8192)


if __name__ == "__main__":
    unittest.main()
