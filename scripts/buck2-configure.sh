#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
system="$(nix eval --raw --impure --expr 'builtins.currentSystem')"
output_path() {
  nix eval --raw ".#packages.${system}.buck-$1.outPath"
}
rust="$(output_path rust)"
cc="$(output_path cc)"
binutils="$(output_path binutils)"
node="$(output_path node)"
bash_path="$(output_path bash)"
coreutils="$(output_path coreutils)"
python="$(output_path python)"
npm_cache="$(output_path npm-cache)"

cat > .buckconfig.local <<EOF
# Generated from the pinned flake outputs; recreate after changing flake.lock.
[nix]
rustc = $rust/bin/rustc
rustdoc = $rust/bin/rustdoc
clippy = $rust/bin/clippy-driver
cc = $cc/bin/cc
cxx = $cc/bin/c++
ar = $binutils/bin/ar
node = $node/bin/node
npm = $node/bin/npm
bash = $bash_path/bin/bash
coreutils = $coreutils/bin
python = $python/bin/python3
npm_cache = $npm_cache
EOF
printf 'Wrote %s/.buckconfig.local\n' "$PWD"
