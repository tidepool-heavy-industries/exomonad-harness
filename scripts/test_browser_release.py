import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest


class ReleaseScripts(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'scripts').mkdir()
        (self.root / 'flake.nix').write_text('fake flake')
        (self.root / 'flake.lock').write_text('{}')
        (self.root / 'rust-toolchain.toml').write_text('[toolchain]\nchannel = "pinned"\n')
        self.assets = self.root / 'web/dist'
        self.assets.mkdir(parents=True)
        (self.assets / 'index.html').write_text('assets')
        for name in ('launch-browser-harness', 'prepare-browser-harness', 'prepare-browser-release'):
            shutil.copy(Path(__file__).with_name(name), self.root / 'scripts' / name)
        self.binary = self.root / 'target/release/harness-demo'
        self.binary.parent.mkdir(parents=True)
        self.program(self.binary, 'printf "%s\\n" "$@" > "$INVOCATION"\ntouch "$2"')
        self.env = dict(os.environ, INVOCATION=str(self.root / 'invocation'))

    def program(self, path, body):
        path.write_text('#!/usr/bin/env bash\nset -eu\n' + body + '\n')
        path.chmod(0o755)

    def launch(self, db, **extra):
        return subprocess.run(
            ['bash', str(self.root / 'scripts/launch-browser-harness'),
             str(db), '127.0.0.1:43127', str(self.assets)],
            cwd='/', env=dict(self.env, **extra), capture_output=True, text=True,
        )

    def test_first_use_creates_private_parent_and_executes_from_unrelated_cwd(self):
        db = self.root / 'new-private/session.sqlite'
        result = self.launch(db)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(stat.S_IMODE(db.parent.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(db.stat().st_mode), 0o600)
        self.assertEqual((self.root / 'invocation').read_text().splitlines(),
                         ['--db', str(db), '--serve', '127.0.0.1:43127', '--assets', str(self.assets)])

    def test_existing_public_parent_is_refused_without_chmod(self):
        parent = self.root / 'public'
        parent.mkdir(mode=0o755)
        parent.chmod(0o755)
        result = self.launch(parent / 'session.sqlite')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(stat.S_IMODE(parent.stat().st_mode), 0o755)
        self.assertFalse((self.root / 'invocation').exists())

    def test_existing_public_database_and_sidecars_are_refused(self):
        parent = self.root / 'private'
        parent.mkdir(mode=0o700)
        db = parent / 'session.sqlite'
        for suffix in ('', '-wal', '-shm'):
            with self.subTest(suffix=suffix):
                path = Path(str(db) + suffix)
                path.write_text('existing data')
                path.chmod(0o644)
                self.assertNotEqual(self.launch(db).returncode, 0)
                self.assertEqual(path.read_text(), 'existing data')
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o644)
                self.assertFalse((self.root / 'invocation').exists())
                path.unlink()

    def test_database_symlink_is_refused_without_touching_target(self):
        parent = self.root / 'private'
        parent.mkdir(mode=0o700)
        target = parent / 'target'
        target.write_text('retain me')
        db = parent / 'session.sqlite'
        db.symlink_to(target)
        self.assertNotEqual(self.launch(db).returncode, 0)
        self.assertEqual(target.read_text(), 'retain me')

    def test_missing_assets_and_binary_do_not_create_database_parent(self):
        db = self.root / 'absent/session.sqlite'
        (self.assets / 'index.html').unlink()
        self.assertNotEqual(self.launch(db).returncode, 0)
        self.assertFalse(db.parent.exists())
        (self.assets / 'index.html').write_text('assets')
        self.binary.unlink()
        self.assertNotEqual(self.launch(db).returncode, 0)
        self.assertFalse(db.parent.exists())

    def prepare(self, preparation_exit='0', cargo_exit='0', check_exit='0'):
        tools = self.root / 'bin'
        tools.mkdir()
        self.program(tools / 'git', 'echo candidate')
        self.program(tools / 'cargo',
                     'printf "%s\\n" "$@" > "$CARGO_ARGS"\n'
                     'if [ "$CARGO_EXIT" != 0 ]; then exit "$CARGO_EXIT"; fi\n'
                     'mkdir -p "$CARGO_TARGET_DIR/release"\n'
                     'printf "#!/bin/sh\\nexit 0\\n" > "$CARGO_TARGET_DIR/release/harness-demo"\n'
                     'chmod +x "$CARGO_TARGET_DIR/release/harness-demo"')
        self.program(tools / 'nix',
                     '[ "$1" = "develop" ]\n'
                     'printf "%s\\n" "$2" > "$NIX_SHELL"\n'
                     'case "$2" in path:*#browser) ;; *) exit 12 ;; esac\n'
                     'shell_dir=${2#path:}; shell_dir=${shell_dir%#browser}\n'
                     'test -s "$shell_dir/rust-toolchain.toml"\n'
                     'shift 2\n[ "$1" = "--command" ]\nshift\nexec "$@"')
        self.program(self.root / 'scripts/prepare-browser-check', 'exit "$PREPARATION_EXIT"')
        self.program(self.root / 'scripts/cargo-focused-test',
                     'test -x "$HARNESS_DEMO_TEST_BIN"\nprintf "%s\\n" "$HARNESS_DEMO_TEST_BIN" > release-checked\n'
                     'printf "%s\\n" "$@" >> release-checked\nexit "$CHECK_EXIT"')
        return subprocess.run(
            ['bash', str(self.root / 'scripts/prepare-browser-harness')], cwd='/',
            env=dict(self.env, PATH=str(tools) + ':' + os.environ['PATH'],
                     CARGO_TARGET_DIR=str(self.root / 'external-build'),
                     CARGO_ARGS=str(self.root / 'cargo-args'), CARGO_EXIT=cargo_exit,
                     NIX_SHELL=str(self.root / 'nix-shell'),
                     PREPARATION_EXIT=preparation_exit, CHECK_EXIT=check_exit),
            capture_output=True, text=True,
        )

    def test_preparation_checks_the_staged_release_binary(self):
        result = self.prepare()
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual((self.root / 'release-checked').read_text().splitlines(), [
            str(self.binary), '--package', 'harness-demo', '--target', 'test:standalone_browser',
            '--filter', 'standalone_missing_assets_and_clean_and_process_loss_reopen', '--expect', '1',
        ])
        self.assertEqual((self.root / 'cargo-args').read_text().splitlines(), [
            '--locked', 'build', '--release', '--package', 'harness-demo', '--bin', 'harness-demo',
        ])
        self.assertTrue((self.root / 'nix-shell').read_text().strip().endswith('#browser'))
        self.assertTrue(self.binary.is_file())

    def test_failed_preparation_invalidates_the_previous_release(self):
        self.assertNotEqual(self.prepare(preparation_exit='7').returncode, 0)
        self.assertFalse(self.binary.exists())
        self.assertFalse((self.root / 'release-checked').exists())

    def test_failed_locked_release_build_invalidates_the_previous_release(self):
        self.assertNotEqual(self.prepare(cargo_exit='9').returncode, 0)
        self.assertFalse(self.binary.exists())
        self.assertFalse((self.root / 'release-checked').exists())

    def test_failed_release_journey_invalidates_staged_binary(self):
        self.assertNotEqual(self.prepare(check_exit='8').returncode, 0)
        self.assertTrue((self.root / 'release-checked').exists())
        self.assertFalse(self.binary.exists())


if __name__ == '__main__':
    unittest.main()
