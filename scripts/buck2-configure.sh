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
tar_path="$(output_path tar)"
gzip="$(output_path gzip)"
python="$(output_path python)"
npm_cache="$(output_path npm-cache)"
remote_enabled=false
remote_toolchain=""
if [ -r /etc/swarm-build/platform ]; then
  remote_toolchain="$(cat /etc/swarm-build/platform)"
  remote_enabled=true
fi

if [ "$remote_enabled" = true ]; then
  mountpoint -q /srv/build || {
    printf 'Buck output volume /srv/build is not mounted\n' >&2
    exit 1
  }
  checkout_root="$(realpath "$(git rev-parse --show-toplevel)")"
  checkout_key="$(printf '%s' "$checkout_root" | sha256sum | cut -d ' ' -f1)"
  output_root="/srv/build/buck-out/$checkout_key"
  if ! mountpoint -q "$checkout_root/buck-out"; then
    printf 'Bind mount %s onto %s/buck-out before configuring Buck\n' "$output_root" "$checkout_root" >&2
    exit 1
  fi
fi

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
tar = $tar_path/bin/tar
gzip = $gzip/bin/gzip
python = $python/bin/python3
npm_cache = $npm_cache
action_path = $rust/bin:$cc/bin:$binutils/bin:$python/bin:$bash_path/bin:$coreutils/bin:$tar_path/bin:$gzip/bin

[remote]
enabled = $remote_enabled
toolchain = $remote_toolchain
EOF
if [ "$remote_enabled" = true ]; then
  cat >> .buckconfig.local <<'EOF'

[buck2_re_client]
action_cache_address = grpc://localhost:50051
engine_address = grpc://localhost:50051
cas_address = grpc://localhost:50051
tls = false
instance_name = swarm
EOF
fi
printf 'Wrote %s/.buckconfig.local\n' "$PWD"
