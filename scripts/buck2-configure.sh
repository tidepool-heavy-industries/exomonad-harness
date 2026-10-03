#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# == 1 && ( $1 == --help || $1 == -h ) ]]; then
  printf 'Usage: scripts/buck2-configure.sh\n'
  exit 0
elif [[ $# != 0 ]]; then
  printf 'Usage: scripts/buck2-configure.sh\n' >&2
  exit 2
fi

if ! mountpoint -q "$PWD/buck-out"; then
  printf 'Provision a real per-checkout buck-out bind mount before configuring Buck: %s/buck-out\n' "$PWD" >&2
  exit 1
fi

remote_enabled=false
remote_toolchain=""
if [ "${HARNESS_BUCK_REMOTE:-false}" = true ]; then
  platform_file=${HARNESS_BUCK_PLATFORM_FILE:-}
  test -n "$platform_file" && test -r "$platform_file" || {
    printf 'Set HARNESS_BUCK_PLATFORM_FILE to a readable remote platform identity file\n' >&2
    exit 1
  }
  remote_toolchain="$(cat "$platform_file")"
  [[ $remote_toolchain =~ ^[0-9a-f]{64}$ ]] || {
    printf 'Invalid remote platform identity in %s\n' "$platform_file" >&2
    exit 1
  }
  remote_address=${HARNESS_BUCK_REMOTE_ADDRESS:-grpc://127.0.0.1:50051}
  [[ $remote_address =~ ^grpc://(localhost|127\.0\.0\.1):[0-9]+$ ]] || {
    printf 'Remote endpoint must use an SSH tunnel on loopback\n' >&2
    exit 1
  }
  remote_enabled=true
fi

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

tmp_config="$(mktemp "$PWD/.buckconfig.local.XXXXXX")"
trap 'rm -f -- "$tmp_config"' EXIT
cat > "$tmp_config" <<EOF
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
  cat >> "$tmp_config" <<EOF

[buck2_re_client]
action_cache_address = $remote_address
engine_address = $remote_address
cas_address = $remote_address
tls = false
instance_name = swarm
EOF
fi
chmod 0644 "$tmp_config"
mv -f -- "$tmp_config" .buckconfig.local
trap - EXIT
printf 'Wrote %s/.buckconfig.local\n' "$PWD"
