# Data operation contracts and compiled plans (local implementation slice)

This slice adds opt-in metadata and a pure plan compiler. It does not admit or
invoke plugins, expose a new API, execute calculations, change wire V1, persist
plans, migrate state, or establish data coverage or broker eligibility.

## Contract boundary

`plugin-sdk::data_contract::OperationDataContract` is explicitly versioned and
rejects unknown fields and versions. Named inputs/outputs carry schema identity
(reference plus canonical-JSON SHA-256), shape and interval/completion semantics.
The operation declares a separate semantics version, embedded parameter schema,
asset/family/mode applicability, required-input readiness, warmup and optional
partitioned, versioned state. Data acquisition and calculations use this common
shape; formulas are not added to the strategy compiler.

The host must load metadata from a verified installed package and bind it to the
selected manifest and operation. `VerifiedDataContract` is an internal compiler
input, not an admission endpoint. Package-descriptor integration and schema
artifact verification are the next slice. Matching declared hashes at compile
time is not proof that an arbitrary caller owns or installed a package.

## Plan boundary

`compile_data_plan` consumes the existing capability resolution, a saved graph
revision identity, explicit node parameters/contexts and port bindings. It never
selects an alternate provider. Each data edge must correspond to a resolved
consumer-to-provider capability dependency with the input's dependency ID.
Compilation does not replace graph revision freshness checks or invocation-time
authorization. The host must obtain the saved graph and check it is current.

V1 requires exact schema reference/digest/shape and matching instrument sets and
intervals, including each operation's input-to-output interval. `None` is event
data, not a wildcard. Temporal conversions are rejected even when explicitly
represented as operations until unit-aware warmup is supported. An input context
may explicitly differ from its node's
output instruments. Traded instruments are recorded separately and do not grant
or constrain data-only readiness. Real coverage, entitlements, credentials,
freshness and quality must still pass acquisition/runtime checks.

Required inputs need one producer. Optional inputs may be absent. Duplicate
producers require an explicit merge operation. All binding cycles are rejected.
Compilation returns a stable producer-first order and canonical plan digest that
includes parameters, contexts, contracts, graph fingerprint and revision.
Each edge must have `plugin_dependency` origin and match the selected operation's
declared dependency ID, capability, optional operation ID, required flag and
resolved child identity. Strategy ordering edges cannot impersonate these edges.

Numeric parameters and schema constants use finite binary64 decimal semantics
within ±(2^53−1); integer values outside that exact supported range are rejected
before canonicalization. Integral numeric forms such as `5.0` normalize to `5`
before schema validation, warmup extraction and identity calculation. The same
normalization and bounds apply recursively to all canonical-hash inputs. Cumulative
history is bounded to this range as well. Larger exact values need a future
explicit representation, never lossy numeric hashing.

Warmup is the maximum of a fixed observation minimum and an optional positive
integer parameter plus additional observations. With one output per input event,
matching intervals and contiguous valid inputs, required-path composition is
`upstream_observations + own_observations - 1`; branches take the maximum. This
is a structural acquisition requirement, not a claim that data has arrived or
that missing/invalid observations count as valid history. Zero warmup is rejected.

The first readiness policy is all required inputs ready. It does not coerce
missing data or errors into false signals. Multi-rate joins, sparse/event-window
warmup, delayed feedback and dynamic windows need separate future contracts.

## Next implementation slices

1. Bind contract and schema artifacts into installed package verification; load
   them through the existing catalog and immutable graph revision service.
2. Add bounded ordered input batches using the existing SDK transport. Preserve
   instrument identity, event interval, known time, arrival time, completion,
   content digest, upstream lineage and source revision. Validate actual values
   against the pinned input/output schemas and temporal policies.
3. Implement one host-owned evaluator for historical and incoming batches. The
   host invokes dependencies and scopes credentials. Commit output references,
   node state and consumed-input cursor atomically. Pin checkpoint compatibility
   to the complete plan and state schemas. Handle duplicates, corrections,
   retries, gaps and backpressure explicitly.
4. Implement a separate default calculation plugin with versioned primitives and
   composites. Use the same evaluator and golden numerical/restart cases for
   batch and incremental execution. Integrate a provider-backed vertical slice
   only after its actual instrument/data coverage is verified.

Existing legacy requests and checkpoints retain their original interpretation.
No automatic migration, state reset, provider rebinding or activation follows
from successful compilation.
