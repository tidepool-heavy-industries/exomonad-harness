# Next: Exomonad's daily-driver harness

The [embedded-host contract](docs/embedded-host-prd.md) defines the accepted
target; the [daily-driver roadmap](docs/daily-driver-plan.md) sequences it.
Read both and the evidence below before assigning work. The outcome is a human
operating real Exomonad agents and recursive worker trees through the browser,
hosted by the main Exomonad binary with one shared harness instance. The
standalone library remains independent of Tidepool.

## First: converge existing work

The source handback from wave22 is WIP and must converge separately from this
design. Its run checkout is `/home/inanna/dev/exomonad-harness-runs/wave22`;
read its `NEXT.md`, `docs/wave22-final-manifest.md`, component reviews and
retained checks. Preserve candidate commits and evidence, and keep unresolved
custom-cell Engine/Store/replay/browser work with its current owner until an
explicit handback. Neither a component candidate nor these docs establish
integrated acceptance.

The Tidepool compiler/continuation batch is running separately. Coordinate with
its integration owner before selecting a matched source/workspace baseline.
Do not launch another wave, move a running checkout, or deploy from this document.

## Then: adoption work

1. Converge and review the standalone custom-cell work as a separate gate.
2. Scaffold the agreed typed host boundary against a deterministic standalone
   stub, preserving Tidepool as sole actor lifecycle authority.
3. Parallelize resident concurrent cells/publication, checkpoint and lifecycle
   integration, and browser composition after shared interfaces exist.
4. Demonstrate the complete browser-operated recursive worker-tree acceptance
   in [the embedded-host contract](docs/embedded-host-prd.md).

The next brief must identify exact source, exclusive owned paths, prerequisite
contracts, product acceptance and a stop/handoff boundary. Prefer useful parallel
component trees; do not delegate consumers behind an undefined shared interface.
Keep repairs with their owners and preserve source and review evidence.

## Scope and authority

Plan/scaffolding is authorized. Purchases, deployment, live credentialed checks
and migration of running waves require the applicable operator assignment.
The operator has ordered an OVH Rise-4 in Hillsboro; provisioning is separate
from harness readiness. The server can first run the existing Exomonad setup.
Production cutover follows the roadmap's gates and an explicit operator decision.

Historical assignments and handoffs moved intact to
[pre-adoption-next](docs/pre-adoption-next.md). Their old wave restrictions and
"active" headings describe those runs, not this roadmap's remaining scope.
PRD.md describes the generic standalone crate and demo. The embedded-host
contract is authoritative for Exomonad embedding and explicitly supersedes
conflicting standalone model-facing assumptions in that file.
