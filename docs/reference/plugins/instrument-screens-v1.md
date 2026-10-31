# Strategy-bound instrument screen policy V1

The additive `screened` InstrumentSelector form binds a candidate producer to a
published strategy. Existing normalized, symbolic-alias and dynamic-expression
forms keep their wire shape. This schema/validation slice does not automatically
execute a screen, invoke a scheduler, warm instruments or create an order. Runtime
and runner adoption remain separate requirements.

A screened selector contains `selector_id`, `type:"screened"` and `screen`.
It cannot also contain normalized_id, alias or expression. The closed screen is
version `tradeassembly.instrument_screen.v1` and requires:

- `capability_requirement_ref` and exact `semantics_version` for the screener;
- named `inputs`, each with an acquisition capability requirement, exact response
  schema, query, max_age_ms and explicit provider coverage checks;
- object `parameters`, `max_candidates`, `top_n`, `ttl_ms` and
  `coverage:"complete_configured_inputs"`.

The screener capability must be mandatory, have no fallback, reference Universe,
declare the instrument_set shape and the versioned instrument-screen request and
result schemas. Each acquisition must also be mandatory with no fallback, declare
its exact response schema and appear in the screener's dependency_refs. Existing
capability-cycle validation still applies. An enabled universe stage is required.
The capability graph remains the registry/authorization authority; naming a
producer does not grant invocation or brokerage permission.

Each query has `literal` and `date_bindings` maps. A date binding names an explicit
`selection_utc_date` basis and `offset_calendar_days`; it cannot overwrite a
literal key. Offsets are bounded to ten years in either direction. This permits
an author to state 45 and 60 calendar-day expiry bounds without assuming session
DTE or hardcoding dates. Query materialization/scheduling is a runtime requirement,
not performed by schema validation. There is no script/template evaluator.
Configuration and credentials remain host-owned, outside query literals.

Coverage checks name an RFC6901 `complete_pointer` whose actual provider value
must be boolean true, a distinct `scope_pointer`, and exact `scope_value`. They
refer to the pinned provider response schema and configured input scope. They do
not turn a provider subset into a global universe or prove historical point-in-
time availability. The runtime must retain the original query, actual field
values and acquisition receipt, reject missing/mismatched/incomplete evidence,
and apply field-specific OI-date policy in the declared screener operation.

Policy bounds include eight input ports, 4,096 candidates, 256 selected members,
64 KiB encoded policy and bounded query/parameter JSON structure. Freshness, TTL,
ranking/filter/model parameters and scope are explicit; the example's capital or
exit thresholds are not defaults. Larger data transport and provider availability
remain independently gated. This V1 input policy covers admitted collections;
indicator-result inputs and generic phase execution still need explicit runtime
contracts. Candidate membership remains separate from managed positions and
pending orders, whose exit monitoring must survive reselection.
