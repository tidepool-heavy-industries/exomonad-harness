# Harness build profiles

Build through the pinned Nix/Buck toolchain described in README.md. The shared
`tidepool.profile` Buck key selects `fast-dev` (default), `debug`, or `production`.
It also applies when Harness is an external cell in a Tidepool workspace.

`fast-dev` compiles first-party libraries, binaries and tests at O0 while retaining
O2 dependency libraries and build-script binaries. This bounds feedback work without repeatedly optimizing
the monolithic unit executable. `debug` compiles every Rust target at O0.
`production` preserves Harness's existing O2 policy. Optimization belongs to
`build/rust/profiles.bzl`; toolchains retain compiler identity, frame pointers and
linker selection. Dependency build scripts receive the matching optimization
level and `PROFILE=release` for production (`dev` otherwise); `DEBUG=false`
reflects the absence of debug-info flags. Dependency projection comes from Reindeer configuration; do not
hand-edit generated third-party targets.

Use `-c tidepool.profile=PROFILE`, alongside `--local-only -c remote.enabled=false`
where remote execution has not been accepted. Record the source revision, selected
profile, effective compiler argv, artifact hash and executed test count. A Rust
`link` action includes compiler code generation; its duration alone does not
identify the linker as the bottleneck. Verify profile policy against actual Buck
action arguments with `scripts/check-rust-profile.py`, including an external-cell
build before claiming the embedding workspace selected its profile correctly.

Keep ongoing release mounts and their generated configuration untouched while
comparing profiles. Use an idle checkout whose source and output ownership have
been coordinated. The production profile remains the release qualification path;
fast-dev execution establishes behavior only for the tested source and profile.

# Authored message fixtures

Use `Item::user_text` for ordinary authored user text in production and tests.
Keep raw `Item` JSON for actual provider wire payloads and deliberate malformed
controls. Exercise valid fixtures through the real context projection so item
classification or provenance refusals cannot mask the invariant under test.

# Provider response replay

Provider provenance comes from issued requests and retained response ownership,
not from a familiar JSON field name. Interpret optional response metadata within
that provider's dialect; preserve the original evidence and keep cross-provider
compatibility checks in their owning projection boundary.

For replay changes, exercise streamed response parsing, Store persistence, a tool
outcome, and the next prepared request together. Include sanitized observed wire
shapes as well as minimal documented examples: a fixture that omits optional
metadata cannot establish that a real response can be replayed. Cover tool
failures and reopen histories, and retain incompatible-provider refusal controls.
