# Standalone deterministic browser harness

This procedure is for an **isolated** instance. Do not stop or change the
existing port-4600 demo or its Tailscale Serve route. It does not run model
inference or shell tools.

## Prepare

From a checkout with Rust installed, run:

```sh
nix develop .#web -c scripts/verify-browser-journey
```

This installs locked web dependencies, checks/tests/builds `web/dist`, and runs
the production-binary browser journey. Missing Nix, Node/npm or Rust are
preparation failures, not browser assertion failures. Preserve the built
`web/dist` assets with the binary; neither location should be treated as the
database home. If your host is not a Nix host, supply the equivalent locked
Node/npm and Rust toolchains before running the script; this alternative has
not been verified for this wave.

## Launch from an ordinary host shell

Choose a *new* loopback port and durable directory outside disposable source
and build trees. This example assumes the checkout and assets are retained at
`/opt/harness`; substitute your actual absolute paths.

```sh
install -d -m 700 "$HOME/.local/state/harness-browser"
read -rsp 'Browser login secret: ' HARNESS_DEMO_SESSION_SECRET; echo
export HARNESS_DEMO_SESSION_SECRET
cd /
/opt/harness/target/debug/harness-demo \
  --db "$HOME/.local/state/harness-browser/session.sqlite" \
  --serve 127.0.0.1:4613 \
  --assets /opt/harness/web/dist
```

Keep the foreground shell open for the simplest ownership and stop semantics:
its Ctrl-C sends SIGINT only to this instance. The actual binary path may
instead be `.exomonad/build/cargo/debug/harness-demo` after the focused
runner. No secret appears in the arguments or committed files. The example
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
