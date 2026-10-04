#!/usr/bin/env python3
"""Exercise the source-only leaf and composition used by bundle admission."""
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

ISSUER = pathlib.Path(__file__).with_name("browser-source-identity.py")


class BrowserSourceIdentityTests(unittest.TestCase):
    def test_changed_web_source_changes_composition_with_unchanged_runtime_and_schema(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            web = root / "web"
            runtime = root / "runtime"
            web.mkdir()
            runtime.mkdir()
            (web / "operator.ts").write_text("renderRetainedOutput()\n")
            native_source = b"native DTOs and event projection remain unchanged"
            (runtime / "server.rs").write_bytes(native_source)
            schema = b'{"type":"object"}'
            (root / "schemas.json").write_bytes(schema)
            leaf = runtime / "browser-source-identity.json"
            composed = root / "composed.json"

            def issue():
                subprocess.run([sys.executable, str(ISSUER), str(web), str(leaf)], check=True)
                subprocess.run([sys.executable, str(ISSUER), str(runtime), str(composed)], check=True)
                return json.loads(composed.read_text())["runtimeSourceSha256"]

            original = issue()
            self.assertEqual(issue(), original)
            (web / "operator.ts").write_text("renderRevisedRetainedOutput()\n")
            changed = issue()
            self.assertNotEqual(changed, original)
            self.assertEqual((runtime / "server.rs").read_bytes(), native_source)
            self.assertEqual((root / "schemas.json").read_bytes(), schema)


if __name__ == "__main__":
    unittest.main()
