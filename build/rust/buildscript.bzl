load("@prelude//rust:cargo_buildscript.bzl", "buildscript_run")
load("//build/rust:profiles.bzl", "dependency_library_optimization_level", "selected_profile")

def tidepool_buildscript_run(**kwargs):
    env = dict(kwargs.pop("env", {}))
    env["PATH"] = read_root_config("nix", "action_path")
    env["HOST"] = "x86_64-unknown-linux-gnu"
    env["OPT_LEVEL"] = dependency_library_optimization_level()
    env["DEBUG"] = "false"
    env["PROFILE"] = "release" if selected_profile() == "production" else "dev"
    buildscript_run(env = env, **kwargs)
