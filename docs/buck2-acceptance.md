# Buck2 migration acceptance

On 2026-09-28, checkout `build/buck2-swarm` at `649a65b` was staged as the
`swarm` user at `/srv/swarm/checkouts/harness` on `swarm-01`. Its `buck-out`
directory is a bind mount from `/srv/build/buck-out/harness`. The generated
`.buckconfig.local` resolved the same Rust 1.93, C, Node, and Python Nix store
paths as Tidepool. Build units ran under `build.slice` with its pinned
`action_path`, `--local-only -c remote.enabled=false`, and no swap use.

| Check | Result | Service runtime | Measured peak |
| --- | --- | ---: | ---: |
| `buck2 build //crates/harness:harness` | Passed, 113 native actions | 40.389 s | Unreliable: daemon had started outside `build.slice` |
| `scripts/buck-focused-test --target //crates/harness:unit_tests --filter hooks::tests::send_plan_and_opaque_evidence_cross_serde_boundary --exact --expect 1 --local-only` | Passed: 1 selected, 1 executed, 197 filtered | 1 min 59.578 s | 5.3 GiB |
| `buck2 build //crates/harness-demo:harness_demo` | Passed, 514 native actions | 1 min 3.280 s | 3.3 GiB |
| `buck2 build //web:check //web:test //web:dist` | Passed; Vitest 6 files, 21 tests | 5.365 s | 2.2 GiB |

After the first build, the harness Buck daemon was stopped and subsequent
units started it inside `build.slice`, so their peak memory figures include
compilation. The server logs remain available through `journalctl` for units
`harness-buck-lib2`, `harness-buck-unit1`, `harness-buck-demo1`, and
`harness-buck-web1`. The web test log is in the Buck `//web:test` output.

The Reindeer graph was generated from an isolated lock with the same 184
registry package/version pairs as the root Cargo lock. `harness` and
`harness-demo` use native Buck Rust rules; their Cargo manifests remain the
publication source. The source graph uses checksum-verified Buck crate
downloads, and the web actions use `npm ci --offline` with a fixed-output Nix
cache.

The NativeLink worker still had its bootstrap-only toolchain closure during
these checks. No remote action was attempted, so remote action scheduling,
cache hits, no-network worker behavior, and platform-property enforcement
remain unverified. Browser suites declare the demo binary, web dist, and
launcher as Buck resources but have not yet executed through Buck's test
runner. The remaining first-party integration targets have only passed Buck
analysis, not compilation or test execution.
