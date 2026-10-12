#!/usr/bin/env python3
"""Check retained effective Buck rustc argv and publish its profile receipt."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def inspect(arguments, profile, kind, rustc, linker):
    expected = "2" if profile == "production" or (profile == "fast-dev" and kind.startswith("dependency-")) else "0"
    levels = []
    for index, argument in enumerate(arguments):
        if argument.startswith("-Copt-level="):
            levels.append(argument.split("=", 1)[1])
        elif argument.startswith("--codegen=opt-level="):
            levels.append(argument.split("=", 2)[2])
        elif argument in ("-C", "--codegen") and index + 1 < len(arguments) and arguments[index + 1].startswith("opt-level="):
            levels.append(arguments[index + 1].split("=", 1)[1])
    if levels != [expected]:
        raise ValueError(f"expected exactly one O{expected} option, observed {levels}")
    if any(argument.startswith("--crate-name=profile_probe_") for argument in arguments):
        if '--cfg=profile_probe_mode="' + profile + '"' not in arguments:
            raise ValueError("profile probe marker does not match the requested root profile")
    if rustc not in arguments:
        raise ValueError("declared compiler is absent from effective action arguments")
    if "-Cforce-frame-pointers=yes" not in arguments:
        raise ValueError("frame-pointer policy is absent")
    if "-Clinker=" + linker not in arguments:
        raise ValueError("declared linker is absent from effective action arguments")
    return expected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("fast-dev", "debug", "production"), required=True)
    parser.add_argument("--kind", choices=("first-party", "dependency-library", "dependency-binary"), required=True)
    parser.add_argument("--args", type=Path, required=True)
    parser.add_argument("--source-checkout", type=Path, required=True)
    parser.add_argument("--rustc", required=True)
    parser.add_argument("--linker", required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    raw = args.args.read_bytes()
    arguments = raw.decode().splitlines()
    level = inspect(arguments, args.profile, args.kind, args.rustc, args.linker)
    oid = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=args.source_checkout, text=True).strip()
    status = subprocess.check_output(["git", "status", "--porcelain"], cwd=args.source_checkout, text=True)
    receipt = {"source_oid": oid, "working_tree_status": status, "profile": args.profile,
               "kind": args.kind, "optimization_level": level, "rustc": args.rustc,
               "linker": args.linker, "args_path": str(args.args.resolve()),
               "args_sha256": hashlib.sha256(raw).hexdigest(), "argv": arguments}
    args.receipt.write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"effective {args.profile} {args.kind}: O{level}; receipt {args.receipt}")


if __name__ == "__main__":
    main()
