# Wave 12 handoff — deterministic browser harness

## Running demo

- Integrated source: `2c19e457d7707b1d57666541fa7c1450003f1234`
  for the server/browser journey; the final documentation-only handoff
  commit does not change the running binary.
- Tailnet-only URL: `https://nixos-1.sphynx-bitterling.ts.net/`.
  Existing Tailscale Serve proxies it to `http://127.0.0.1:4600`;
  no Funnel route was enabled or unrelated route changed.
- Browser login: enter the secret stored on this host at
  `/tmp/wave12-live/session-secret` (`chmod 600`). It is not in Git.
  From an authorized shell on this host:
  `cat /tmp/wave12-live/session-secret`.
- Process PID is in `/tmp/wave12-live/server.pid` (PID 5976 at handoff);
  the process is owned by the local `inanna` user. Database:
  `/tmp/wave12-live/session.sqlite`; log:
  `/tmp/wave12-live/server.log`. The browser test itself added example
  records to that database; they are intentional visible history.
- Launch command, from the repository root after building `web/dist`:

  ```sh
  HARNESS_DEMO_SESSION_SECRET="$(cat /tmp/wave12-live/session-secret)" \
    .exomonad/build/cargo/debug/harness-demo \
      --db /tmp/wave12-live/session.sqlite --serve 127.0.0.1:4600
  ```

  `--serve` uses a deterministic, credential-free Engine/Store path;
  `--ask` is a separate Engine-backed CLI mode. The demo does not run
  model inference or shell tools. To stop only this demo, inspect the
  recorded PID and command, then `kill -TERM "$(cat /tmp/wave12-live/server.pid)"`.
  Do **not** disable Tailscale Serve or stop a shared daemon.

## Human journey and verification

The Command tab labels deterministic mode and accepts `echo TEXT`,
`test`, `wait`, `message TEXT`, `cancel`, `child TEXT`, and `fail`.
The Timeline shows server command IDs and specific outcomes. Inbox
shows persisted progress and real sender/recipient messages with
monotone ordinals. A `wait` is `pending` until cancellation, then its
request has coarse `failed` state and specific `cancelled` outcome,
with a cancelled job. `message` reports `queued` unless actual
presentation or action is observed. Refresh loads a stored snapshot
and does not resubmit commands.

On exact integrated source `2c19e45`, checks passed:

| Boundary | Selection and execution | Evidence |
| --- | --- | --- |
| Demo server unit tests | `--target bin:harness-demo --filter server --expect 8`: 8 matched, 8 executed, 8 passed | `.exomonad/build/cargo/debug/deps/focused-67e9iklr/evidence.json` |
| Authenticated production-binary journey | `--target test:browser_journey --filter browser --expect 1`: 1 matched, 1 executed, 1 passed | `.exomonad/build/cargo/debug/deps/focused-yytx183f/evidence.json` |
| Prior Driver process-kill recovery | `--target bin:harness-demo --filter process_restart_driver_discovers_committed_answer_without_wake --expect 1`: 1 matched, 1 executed, 1 passed | `.exomonad/build/cargo/debug/deps/focused-k2_y339y/evidence.json` |

`cargo fmt --check` and `git diff --check` passed on the integrated
source. The previously integrated web renderer passed npm tests 14/14,
`npm run check` and `npm run build` before the server merge; that
server merge did not change `web/src`.

Actual Chromium automation against the running loopback server
signed in through the browser cookie flow, observed deterministic
mode, sent echo and test, saw progress and final answers, observed
pending wait, queued message, terminal cancel, real child message
and reply, controlled fail then successful echo, and recovered
history after refresh without an increased durable request count.
Final passing result and screenshot:
`/tmp/wave12-live/browser-result.json`,
`/tmp/wave12-live/browser-after-refresh.png`.
Early script runs failed on a tab selector and on incorrectly expecting
the *transient* pending outcome to remain after cancellation; another
run compared counts before the final echo had settled. Those were
automation barriers, not product failures. The final script used a
unique echo and a final-answer barrier before its refresh comparison.

Separately, a temporary server on port 4601 was SIGKILLed after a
settled echo and a visibly pending wait. A new process using the same
file database preserved the settled answer, marked the interrupted
request failed without replay, and accepted a later echo.
`/tmp/wave12-live/restart-result.json` records the passing black-box
result. The persistent demo on port 4600 was not restarted.

The local host also returned HTTP 200 from the tailnet HTTPS URL's
`/api/session` and from loopback. This verifies local Tailscale Serve
proxying, **not** reachability from a second tailnet device. A remote
operator should confirm that path from their own device. No claim of
power-loss durability, remote exactly-once execution or live adapter
inference is made.

## Limits and next owner

This is a standalone demonstration, not a generic scheduler/tool or
credentialed model feature. Demo records have a single root session
with children; the UI does not promise more delivery state than the
Store has observed. Secrets and file-backed state are under `/tmp`,
so a host cleanup or reboot can remove the running demo. The next
operator owns remote-device reachability verification and, if the
demo must survive reboot, moving the secret/database and process
under an approved persistent service without exposing the plain-HTTP
loopback login directly.
