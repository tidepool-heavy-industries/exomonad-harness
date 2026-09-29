# Embedding-ready implementation handoff

Work in progress on `integration/embedding-ready`; no deployment or Tidepool pin.
Base: `0fbcbbbb2630ae59ad75e18ab797b1e11ec0860a`.

## Retained work joined

Production baseline imported from wave22 root `619519b309016c9f205bf66d4faec61744c5cff4`,
preserving current prompts, workspace pin and adoption documents. Product candidates:
provider `880dc75`, Engine `572cb6b` plus recovery `87bf658`, Store `37a4653` and
`2baaf36`, replay `6f6d625` and fixture `3f0752e`, web `517c70b`, server `5d503b4`,
browser journey `62ac759`. Original branches and worktrees remain intact.

Join repairs: request-scoped replay no longer falls back to global call identity;
replay supports raw custom dispatch; retained fixtures compile on the joined APIs;
cancellation fixture verifies durable cancellation output rather than an obsolete
interrupted-claim state; browser pending wait uses the published pending request;
restart tests reauthenticate; failed demo requests advance their durable branch
head before subsequent work. These repairs have production-path regression coverage.

## Initial convergence evidence

- `cargo test -p harness --lib`: 182 passed, 2 ignored.
- Web Nix shell: npm ci, check, test (18 passed), build passed.
- Counted `browser_journey` target: 4 matched/executed/passed after join repairs.
- Other targets passed during workspace traversal; final combined suite remains
  a release gate after embedding changes. No live provider or Haskell execution.

## Remaining

Public embedding interfaces, owner-acknowledged cancellation, immutable request
provider views, reusable pending-call checkpoints, plain-text compaction, external
host browser demonstration, final combined checks/review, canonical integration.
