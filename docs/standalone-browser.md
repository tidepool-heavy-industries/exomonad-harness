# Standalone deterministic browser harness

This procedure is for an **isolated** instance. Do not stop or change the
existing port-4600 demo or its Tailscale Serve route. It does not run model
inference or shell tools.

## Prepare

From a checkout with Nix available, run:

```sh
scripts/prepare-browser-harness
```

This snapshots the pinned flake inputs to prepare `web/dist`, builds and stages
`target/release/harness-demo`, then runs the existing standalone browser journey
against that exact release executable through `scripts/launch-browser-harness`.
Preparation enters the pinned Nix `browser` shell, which provides the Rust
toolchain from `rust-toolchain.toml` and pinned Node; host Rust and Node
installations are not required.
The check uses a temporary database and unrelated working directory, tests
reconnect and process-loss reopen, and stops only its own process. A failed
preparation or release check invalidates the staged binary. The successful
output includes its SHA256 and focused evidence location. Missing Nix or
failure to materialize its pinned inputs is a preparation failure, not a
browser assertion failure. Preserve the built
`web/dist` assets with the binary; neither location should be treated as the
database home. Copy the prepared binary and assets to stable absolute host
paths if the checkout is disposable.

## Launch from an ordinary host shell

Choose a *new* loopback port and durable directory outside disposable source
and build trees. This example assumes a retained launcher at
`/opt/harness/scripts/launch-browser-harness`, with a prepared binary and
assets copied to stable `/opt/harness` paths; substitute your actual absolute
paths. The launcher invokes the prebuilt binary, not Cargo. It creates a missing
database parent with private permissions and refuses an existing parent,
database or SQLite sidecar that is not owned by the current user or grants
group/world access. It does not change existing permissions. Review and correct
such paths deliberately before retrying; `umask` cannot repair existing files.

```sh
install -d -m 700 "$HOME/.local/state/harness-browser"
read -rsp 'Browser login secret: ' HARNESS_DEMO_SESSION_SECRET; echo
export HARNESS_DEMO_SESSION_SECRET
cd /
HARNESS_DEMO_BIN=/opt/harness/bin/harness-demo \
  /opt/harness/scripts/launch-browser-harness \
    "$HOME/.local/state/harness-browser/session.sqlite" \
    127.0.0.1:4613 \
    /opt/harness/web/dist
```

Keep the foreground shell open for the simplest ownership and stop semantics:
its Ctrl-C sends SIGINT only to this instance. No secret appears in the
arguments or committed files. The example
does not create a secret file; the login secret must be supplied again for a
reopen. Avoid shell tracing while entering it.

Open `http://127.0.0.1:4613/`, log in with the same secret, and wait for the
authenticated snapshot to load before submitting a command. A printed PID or
listening message alone is not an application readiness check. The Command
tab accepts deterministic `echo TEXT`, `child TEXT`, `wait`, `cancel`, `test`,
`message TEXT`, and `fail`. Refresh loads persisted state; it must not
resubmit a command.

To reopen, run the same binary with the same absolute database and assets
paths and a suitable login secret. If a process was lost rather than stopped
cleanly, first confirm that exact instance is no longer running before
reopening the database. Never use a broad `pkill` pattern. Missing assets
should produce a nonzero startup error naming the path, rather than a
listening server. A bind failure means the selected port is occupied; choose
another isolated port rather than stopping the existing demo.

The example paths and port are illustrative, not a claim that the final
integrated wave-13 binary has passed the unrelated-directory and reopen
checks. See the wave-13 handoff for the exact tested source and evidence.
