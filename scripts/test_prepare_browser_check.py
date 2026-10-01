import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class BrowserPreparationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "scripts").mkdir()
        (self.root / "web").mkdir()
        for name in ("prepare-browser-check", "verify-browser-journey"):
            shutil.copy(Path(__file__).with_name(name), self.root / "scripts" / name)
        (self.root / "flake.nix").write_text("pinned flake")
        (self.root / "flake.lock").write_text("pinned lock")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=str(self.bin) + ":" + os.environ["PATH"])
        self.stub("git", 'printf "candidate\\n"')
        self.stub("cargo", 'echo unexpected-cargo >> cargo-calls; exit 99')
        self.stub("nix", r'''
case "$2" in path:*\#web) ;; *) exit 91;; esac
source=${2#path:}; source=${source%#web}
test "$(ls "$source" | wc -l)" -eq 2 || exit 92
cmp flake.lock "$source/flake.lock" || exit 93
printf '%s' "$source" > shell-source
shift 3
exec "$@"
''')
        self.stub("npm", '''
printf '%s\\n' "$*" >> ../npm-calls
if [[ "$*" == "run build" && "${NO_ASSETS:-}" != 1 ]]; then
  mkdir -p dist; echo assets > dist/index.html
fi
exit "${NPM_EXIT:-0}"
''')

    def stub(self, name, body):
        path = self.bin / name
        path.write_text("#!/usr/bin/env bash\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def run_preparation(self, candidate="candidate", **extra):
        return subprocess.run(
            ["bash", "scripts/prepare-browser-check", candidate],
            cwd=self.root, env=dict(self.env, **extra), capture_output=True, text=True,
        )

    def test_prepares_local_assets_from_only_pinned_flake_files(self):
        result = self.run_preparation()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "npm-calls").read_text().splitlines(),
                         ["ci", "run check", "test", "run build"])
        self.assertFalse(Path((self.root / "shell-source").read_text()).exists())
        self.assertFalse((self.root / "cargo-calls").exists())

    def test_frontend_preparation_does_not_need_cargo_on_path(self):
        (self.bin / "cargo").unlink()
        result = self.run_preparation()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_wrong_source_does_not_start_preparation(self):
        self.assertNotEqual(self.run_preparation("older-candidate").returncode, 0)
        self.assertFalse((self.root / "npm-calls").exists())

    def test_failed_preparation_does_not_build_or_test(self):
        self.assertEqual(self.run_preparation(NPM_EXIT="7").returncode, 7)
        self.assertEqual((self.root / "npm-calls").read_text().splitlines(), ["ci"])

    def test_assets_in_a_different_checkout_do_not_establish_readiness(self):
        other = self.root / "other" / "web" / "dist"
        other.mkdir(parents=True)
        (other / "index.html").write_text("unrelated")
        self.assertNotEqual(self.run_preparation(NO_ASSETS="1").returncode, 0)


if __name__ == "__main__":
    unittest.main()
