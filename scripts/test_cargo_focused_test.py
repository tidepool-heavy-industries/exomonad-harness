from __future__ import annotations

import os
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
            "if '--list' in args:\n"
            "    mode = os.environ['STUB_MODE']\n"
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
            "    if mode == 'runzero':\n"
            "        print('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s')\n"
            "    elif mode != 'missing' and int(os.environ.get('STUB_EXIT', '0')) != 0:\n"
            "        print('test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s')\n"
            "    elif mode != 'missing':\n"
            "        print('test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s')\n"
            "    sys.exit(int(os.environ.get('STUB_EXIT', '0')))\n"
        )
        self.stub.chmod(0o755)

    def invoke(self, mode: str, exit_code: int = 0) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env["PATH"] = f"{self.bin_dir}{os.pathsep}{env['PATH']}"
        env["STUB_MODE"] = mode
        env["STUB_EXIT"] = str(exit_code)
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--package",
                "harness",
                "--target",
                "lib",
                "--filter",
                "focused_case",
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


if __name__ == "__main__":
    unittest.main()
