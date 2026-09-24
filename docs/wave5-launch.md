# Wave-5 launch record

Filled at launch from verified revisions, never from memory. Every field is
either a value with its source command, or `pending`.

| Field | Value | Source |
|---|---|---|
| Run ID | pending | `exomonad host` output line at launch |
| Deployed binary revision | pending | `git -C ~/dev/tidepool rev-parse HEAD` at `scripts/redeploy.sh` |
| Workspace pin | pending | `DEFAULT_WORKSPACE_REV` in `bridge/facade/src/exomonad/scaffold.rs`; `.exomonad/workspace` submodule here |
| Harness HEAD | pending | `git rev-parse HEAD` in this checkout |
| Core prompt catalog version | pending | `CATALOG_VERSION` in `bridge/facade/src/actor_host/prompt_catalog.rs` |
| Project prompt revisions | pending | `git log -1 --format=%h -- .exomonad/prompts` here |
| Log path | pending | `.exomonad/logs/<run>.log` |
| Observer owner | pending | named at launch |
| Wake job | pending | cron id and cadence |
| Launch time (UTC) | pending | `date -u` |

Mid-wave prompt edits append a row below with the revision, the time, and
the actors known to have received it.

| Time (UTC) | Revision | Files | Actors exposed |
|---|---|---|---|
