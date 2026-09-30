load("@prelude//:rules.bzl", "genrule")

def npm_dependencies():
    command = """set -euo pipefail
NODE='$(exe toolchains//:node)'
NPM='$(exe toolchains//:npm)'
NPM_CACHE="$PWD/$(location toolchains//:npm_cache)"
OUTPUT="$PWD/$OUT"
export PATH="${NODE%/*}:$PATH"
export CI=1 TZ=UTC npm_config_update_notifier=false npm_config_logs_dir="$TMP/npm-logs"
mkdir -p "$TMP/npm"
cp -rL "$SRCDIR/." "$TMP/npm/"
cd "$TMP/npm"
"$NPM" ci --offline --ignore-scripts --cache "$NPM_CACHE" --no-audit --no-fund
cp -a node_modules "$OUTPUT"
"""
    genrule(
        name = "npm_dependencies",
        srcs = ["package.json", "package-lock.json"],
        out = "node_modules",
        bash = command,
        visibility = ["PUBLIC"],
    )

def web_action(name, script, output, srcs, is_directory = False):
    command = """set -euo pipefail
NODE='$(exe toolchains//:node)'
NPM='$(exe toolchains//:npm)'
NPM_DEPS="$PWD/$(location :npm_dependencies)"
OUTPUT="$PWD/$OUT"
export PATH="${NODE%/*}:$PATH"
export CI=1 TZ=UTC npm_config_update_notifier=false npm_config_logs_dir="$TMP/npm-logs"
mkdir -p "$TMP/web"
cp -rL "$SRCDIR/." "$TMP/web/"
cp -a "$NPM_DEPS/." "$TMP/web/node_modules/"
cd "$TMP/web"
"""
    if script == "test":
        command += "\"$NPM\" run test 2>&1 | tee \"$OUTPUT\"\n"
    else:
        command += "\"$NPM\" run " + script + "\n"
    if is_directory:
        command += "cp -a dist \"$OUTPUT\"\n"
    elif script != "test":
        command += "printf 'passed\\n' > \"$OUTPUT\"\n"
    genrule(
        name = name,
        srcs = srcs,
        out = output,
        bash = command,
        visibility = ["PUBLIC"],
    )
