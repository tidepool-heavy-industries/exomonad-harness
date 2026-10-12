"""Target-owned Rust optimization policy shared with the embedding workspace."""
load("@prelude//:rules.bzl", _rust_binary = "rust_binary", _rust_library = "rust_library", _rust_test = "rust_test")
load("@prelude//rust:cargo_package.bzl", "cargo")

_PROFILES = ["fast-dev", "debug", "production"]

def selected_profile():
    profile = read_root_config("tidepool", "profile", "fast-dev")
    if profile not in _PROFILES:
        fail("unknown native profile %r; expected one of %s" % (profile, ", ".join(_PROFILES)))
    return profile

def _flags(dependency_library, flags):
    profile = selected_profile()
    for flag in flags:
        if flag.startswith("-Copt-level=") or flag.startswith("opt-level="):
            fail("optimization belongs to tidepool.profile, not per-target rustc_flags")
    level = "2" if profile == "production" or (profile == "fast-dev" and dependency_library) else "0"
    return list(flags) + ["-Copt-level=" + level]

def rust_library(rustc_flags = [], **kwargs):
    _rust_library(rustc_flags = _flags(False, rustc_flags), **kwargs)

def rust_binary(rustc_flags = [], **kwargs):
    _rust_binary(rustc_flags = _flags(False, rustc_flags), **kwargs)

def rust_test(rustc_flags = [], **kwargs):
    _rust_test(rustc_flags = _flags(False, rustc_flags), **kwargs)

def dependency_rust_library(rustc_flags = [], **kwargs):
    cargo.rust_library(rustc_flags = _flags(True, rustc_flags), **kwargs)

def dependency_rust_binary(rustc_flags = [], **kwargs):
    cargo.rust_binary(rustc_flags = _flags(False, rustc_flags), **kwargs)
