# Standalone deterministic browser harness

This procedure is for an **isolated** instance. Do not stop or change the
existing port-4600 demo or its Tailscale Serve route. It does not run model
inference or shell tools.

## Prepare

From a checkout with Rust installed, run:

```sh
scripts/prepare-browser-harness
```

This runs `nix develop .#web -c scripts/verify-browser-journey` to install
locked web dependencies, check/test/build `web/dist`, and run the
production-binary browser journey, then builds `target/release/harness-demo`.
Missing Nix, Node/npm or Rust are
preparation failures, not browser assertion failures. Preserve the built
`web/dist` assets with the binary; neither location should be treated as the
database home. Copy the prepared binary and assets to stable absolute host
paths if the checkout is disposable. If your host is not a Nix host, supply
the equivalent locked Node/npm and Rust toolchains before running the script;
this alternative has not been verified for this wave.

## Launch from an ordinary host shell

Choose a *new* loopback port and durable directory outside disposable source
and build trees. This example assumes a retained launcher at
`/opt/harness/scripts/launch-browser-harness`, with a prepared binary and
assets copied to stable `/opt/harness` paths; substitute your actual absolute
paths. The launcher invokes the prebuilt binary, not Cargo.

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
