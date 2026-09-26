# Wave 13 launch contract

Source baseline: `b6e5a14b6f2353ea9c1ba5ca0c129795e7fc9756`.
This contract governs the isolated deterministic `harness-demo --serve` instance;
it does not authorize changes to the port-4600 demo or Tailscale Serve.

- Preparation: `nix develop .#web -c scripts/verify-browser-journey` builds
  `web/dist` and runs the existing production-browser check. Rust is a separate
  prerequisite. A release-style standalone build must retain its assets.
- Runtime: `harness-demo --db ABSOLUTE_SQLITE_PATH --serve
  127.0.0.1:EPHEMERAL_PORT --assets ABSOLUTE_ASSET_DIR`. The existing
  invocation without `--assets` must continue working from the project root.
  The database and asset paths are resolved before serving; neither may depend
  on the process's subsequent working directory. `--assets` is serve-only.
- Startup fails before accepting connections when assets are missing, with a
  nonzero exit and a diagnostic naming the path. Readiness is a successful
  authenticated `GET /api/snapshot`, not merely a printed PID.
- A host operator owns the process using ordinary OS process management. Send
  SIGINT for graceful stop and SIGKILL only for the process-loss test; never
  stop by matching a broad process name. The environment provides
  `HARNESS_DEMO_SESSION_SECRET` without placing its value in arguments, logs or
  committed files.
- Clean reopen and process-loss reopen use the same absolute SQLite path and
  recover the prior deterministic conversation and child messages. Tests use
  separate temporary data and a loopback port other than 4600.

The production consumer is `crates/harness-demo/src/main.rs::serve`, whose
Store-backed `Engine` path and browser server persist under
`harness-demo-server:/root`. Consequential failure: a missing asset root must
not leave a listening server or alter existing data. Acceptance must test this
against a production binary, not only the CLI parser.
