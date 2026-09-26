# Wave 17 operator handoff: standalone deterministic browser

This is a local, isolated demo, not the port-4600 demo. It exercises the
production `harness-demo --serve` browser/server path with deterministic
responses; it makes no model or external API calls.

## Prepare and start

From the repository root, prepare the web assets and standalone release binary:

```sh
scripts/prepare-browser-harness
```

The script prepares assets with the pinned web shell, builds and stages the
release binary, then runs the standalone browser journey against that binary
through the production launcher. Nix and Rust/Cargo build prerequisites must be
available. The standalone binary needs the prepared `web/dist` assets.
See [the maintained procedure](standalone-browser.md) for the current tested
entrypoint and its evidence contract.

In a second terminal, from the repository root, choose a private database path
and session secret, then start on loopback (replace `43127` if occupied):

```sh
export HARNESS_DEMO_SESSION_SECRET="$(openssl rand -hex 32)"
scripts/launch-browser-harness \
  "$PWD/.local/wave17-demo.sqlite" \
  127.0.0.1:43127 \
  "$PWD/web/dist"
```

The secret is required by the browser-session login; keep it out of command
arguments, logs, and committed files. `--serve` rejects non-loopback addresses
because this demo uses plain HTTP. Visit `http://127.0.0.1:43127` in a browser
and use its Command interface. Stop the server with Ctrl-C. Keep the same
absolute `--db` path to reopen this demo's data; choose a new path for a clean
session.

## Try the deterministic journey

Submit these commands in the Command interface, waiting for each to complete:

1. `echo hello` — deterministic echo response.
2. `echo inject-context` — exercises the before-request Inject example: the
   browser provider adds the standalone injected context and restricts that
   request's advertised tools to `sleep`.
3. `child hello` — creates a child conversation, delivers a parent message,
   and records its deterministic reply.

Reload the page or reconnect while the server remains running to receive the
current snapshot and subsequent ordered updates. To exercise process-loss
reopen, stop the process (Ctrl-C for a graceful stop; killing it is an abrupt
loss), then run the same command with the same database path and secret and
visit the same loopback URL again. Reopen/login presents the persisted
conversation snapshot, including completed requests, child conversation and
messages, and recorded before-request decisions. Do not resubmit a command
merely to restore the view.

## Persistence and limits

The deterministic server uses a SQLite Store: conversation/session state,
requests and their outcomes, child/message envelopes, and before-request
decision/evidence records survive restart when the same database is reused.
Reconnect is a snapshot followed by sequenced updates; it is not a replay of
provider work. The Inject context is for that transport attempt and is not
appended to durable conversation history.

This is a deterministic browser demonstration, not a live model-backed
assistant or a production deployment. It is loopback-only and uses HTTP
browser-session authentication; do not expose it to a network. The example
commands exercise the supported echo, inject-context, and child paths rather
than arbitrary model/tool behavior. Missing assets or a missing session-secret
environment variable prevent startup.
