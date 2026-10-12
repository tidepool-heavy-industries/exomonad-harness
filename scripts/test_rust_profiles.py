"""Exercise owning profile rules and refusal checks before native action inspection."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("profile_check", ROOT / "scripts/check-rust-profile.py")
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


def rules(profile):
    calls = []
    class Cargo:
        @staticmethod
        def rust_library(**kwargs):
            calls.append(kwargs)
        rust_binary = rust_library
    def emit(**kwargs):
        calls.append(kwargs)
    def fail(message):
        raise ValueError(message)
    namespace = {"read_root_config": lambda section, key, default: profile or default,
                 "fail": fail, "cargo": Cargo, "_rust_library": emit,
                 "_rust_binary": emit, "_rust_test": emit}
    source = "\n".join(line for line in (ROOT / "build/rust/profiles.bzl").read_text().splitlines()
                       if not line.startswith("load("))
    exec(source, namespace)
    return namespace, calls


class Profiles(unittest.TestCase):
    def test_real_rules_select_every_target_kind_and_preserve_other_flags(self):
        for profile in ("fast-dev", "debug", "production"):
            for name in ("rust_library", "rust_binary", "rust_test", "dependency_rust_library", "dependency_rust_binary"):
                with self.subTest(profile=profile, rule=name):
                    namespace, calls = rules(profile)
                    namespace[name](name="case", rustc_flags=["--cfg=fixture"])
                    kind = "dependency-library" if name == "dependency_rust_library" else "first-party"
                    flags = calls[0]["rustc_flags"]
                    self.assertIn("--cfg=fixture", flags)
                    CHECK.inspect(["/pinned/rustc", "-Cforce-frame-pointers=yes", "-Clinker=/pinned/cc", *flags],
                                  profile, kind, "/pinned/rustc", "/pinned/cc")
    def test_dependency_buildscript_uses_library_profile(self):
        for profile, expected in (("fast-dev", "2"), ("debug", "0"), ("production", "2")):
            namespace, calls = rules(profile)
            namespace["buildscript_run"] = lambda **kwargs: calls.append(kwargs)
            source = "\n".join(line for line in (ROOT / "build/rust/buildscript.bzl").read_text().splitlines() if not line.startswith("load("))
            exec(source, namespace)
            namespace["tidepool_buildscript_run"](name="case")
            self.assertEqual(calls[0]["env"]["OPT_LEVEL"], expected)
    def test_default_and_unknown_profile(self):
        namespace, _ = rules(None)
        self.assertEqual(namespace["selected_profile"](), "fast-dev")
        namespace, _ = rules("unknown")
        with self.assertRaises(ValueError):
            namespace["rust_test"](name="case")
    def test_target_override_cannot_bypass_profile_owner(self):
        namespace, _ = rules("fast-dev")
        with self.assertRaises(ValueError):
            namespace["rust_test"](name="case", rustc_flags=["-Copt-level=2"])
    def test_effective_argv_rejects_conflicting_or_missing_policy(self):
        fixed = ["/pinned/rustc", "-Cforce-frame-pointers=yes", "-Clinker=/pinned/cc"]
        for extra in ([], ["-Copt-level=2"], ["-Copt-level=2", "-Copt-level=0"]):
            with self.assertRaises(ValueError):
                CHECK.inspect(fixed + extra, "fast-dev", "first-party", "/pinned/rustc", "/pinned/cc")
        CHECK.inspect(fixed + ["-C", "opt-level=0"], "fast-dev", "first-party", "/pinned/rustc", "/pinned/cc")


if __name__ == "__main__":
    unittest.main()
