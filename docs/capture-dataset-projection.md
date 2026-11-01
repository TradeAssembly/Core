# Capture-backed research snapshots

The closed `tradeassembly.dataset_capture_projection.v1` contract projects an
existing verified V2 capture into an immutable research snapshot. The initial
profile accepts one crypto spot instrument, one to 256 contiguous completed
UTC daily OHLCV bars, exact supported research precision, and optional VWAP.
It requires an explicit modeled-calendar and retrospective-availability
acknowledgment. It does not acquire new data or grant execution authority.

The local user HTTP surface is:

- `POST /dataset-projections`: retain the exact request under an owner-scoped
  stable projection key.
- `POST /dataset-projections/{projection_id}/advance`: perform bounded,
  idempotent publication transitions.
- `GET /dataset-projections/{projection_id}`: read owned historical status and
  the retained proof.

Each mutation still uses the existing protected `research.dataset.create`
authority. Original completed C3 responses remain historical. A fresh outer
idempotency key can resume the same projection key after an uncertain result;
changed requests with that retained key refuse.

Publication proceeds through Prepared, SnapshotStored, ProofStored and
Published. Repository insertion, immutable proof and inherited object ownership
are independent durable writes. Research consumers require the final state and
matching proof even if ownership was bound before an interrupted final CAS.
No atomic transaction across these ports is claimed.

The proof preserves exact capture and package bindings, selected typed OHLCV
port, input witness, values, event bounds, completion, original known/arrival
times, source revisions and cutoff. New research rechecks current source
permissions and bindings. Typed fills additionally require all acquired/replayed
and point availability evidence to fit the consuming run's cutoff.

Historical metadata and workspace reference export require completed owned
publication and proof integrity. Export validates the exact redacted metadata
against retained snapshot evidence; it does not reinterpret a row without raw
observations as a complete snapshot. Raw observations and projection sidecars
are not included in public workspace export. Importing a reference alone cannot
make it usable research data.

This profile classifies snapshots as Test with provider authenticity unestablished.
A capture digest and installed schema cannot establish production authenticity.
Actual provider acquisition, native publisher composition, OS-process recovery,
MCP/agent and remote Relay projection operations remain separate qualification
and interface work. Existing snapshots without this projection marker retain
their prior contracts.
