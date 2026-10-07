# Form boundary fixtures

The JSON in `web/src/fixtures/prepared-form-boundaries.json` and
`crates/harness/src/store/fixtures/prepared-form-boundaries.json` contains the
same `.fixtures` object emitted by Tidepool's
`bridge/haskell/test-display-tree/EmitFormWireFixtures.hs`. The producer calls
`Tidepool.Form.Wire.prepareForm`, runs its retained decoder, and uses the
standard structural JSON encoder.

Producer revision: `460f86ae11768c57f088aaa06066e0c09f811b6a`.
Producer source SHA-256:
`d0096e15e8bda948036c83fd81a387a75cc2605d8e63682c40c1eee0de9ec056`.

Regenerate in that Tidepool checkout with its pinned GHC:

```sh
swarm-build /nix/store/7r1qai5p2r2a39b5mv0vlpyx360a2v5m-ghc-9.12.3-with-packages/bin/ghc -O0 -Wall -XGHC2024 -ibridge/haskell/lib -outputdir /srv/build/infra-prototypes/inanna-ui-dialogue-20261007/forms/fixture-producer -o /srv/build/infra-prototypes/inanna-ui-dialogue-20261007/forms/emit-form-wire-fixtures bridge/haskell/test-display-tree/EmitFormWireFixtures.hs
swarm-build /srv/build/infra-prototypes/inanna-ui-dialogue-20261007/forms/emit-form-wire-fixtures prepared-form-boundaries.json 460f86ae11768c57f088aaa06066e0c09f811b6a d0096e15e8bda948036c83fd81a387a75cc2605d8e63682c40c1eee0de9ec056
```

Copy the emitted `.fixtures` object into both Harness files. Store tests open
and submit every descriptor, retaining the original draft. For drafts rejected
by Haskell, they apply the emitted field errors and verify that the opening
sequence stays unchanged. Browser tests cover two simultaneous rich choices
with the same field IDs, empty multiselection, and a cleared number input.
