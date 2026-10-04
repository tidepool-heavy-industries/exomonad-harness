load("@prelude//:rules.bzl", "rust_test")

def integration_test(name, deps, resources = [], run_env = {}, extra_srcs = []):
    rust_test(
        name = name,
        crate = name,
        crate_root = "tests/" + name + ".rs",
        edition = "2024",
        srcs = ["tests/" + name + ".rs"] + extra_srcs,
        deps = deps,
        resources = resources,
        run_env = run_env,
    )
