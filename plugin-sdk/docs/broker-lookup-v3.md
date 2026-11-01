# Original-order paper lookup V3

`broker_lookup_v3::LookupRequestV3` is a closed, paper-only read request for
`broker.order_lookup.paper_v3`, capability `broker.order_lookup.paper@3`.
Its request schema reference is `schema://tradeassembly/broker-lookup@3`.
The wire version is `tradeassembly.broker_lookup.v3`.

The required fields are:

- `schemaVersion`: that exact V3 wire version.
- `originalSubmit`: the exact original `broker_v2::SubmitRequest`, including its
  V2 version, `orderplan_<64 lowercase hex>` plan ID, canonical account digest,
  opaque host instrument identity, provider symbol, position action, exact
  string decimals, limit price, time in force and fixed client order ID.
- `expectedAccount`: a closed object with `brokerNamespace`,
  `environment: "paper"`, and `providerAccountId`.

There is no second client-order ID, amendable order, endpoint, credential,
account alias or order quantity override in the lookup request. Parsing and
validation preserve the existing V2 limit-only/decimal rules. `decode_lookup_json`
rejects payloads exceeding16384 bytes before decoding. Account strings are
nonempty, at most256 UTF-8 bytes, contain no control characters and are compared
exactly without trimming or alias/case normalization, matching Core identity
construction.

`ExpectedPaperAccount::canonical_account_sha256()` hashes the canonical JSON:

```json
{"version":"tradeassembly.canonical_account.v1","brokerNamespace":"fixture.broker","environment":"paper","providerAccountId":"fixture-account"}
```

The digest for that synthetic example is
`794362c12a3edb69c64643c583b7fe562f0db4fbf1a3028809fd56f8fdd5d7ad`.
It must equal `originalSubmit.canonicalAccountSha256`. Computing a matching hash
is a structural binding check; it does not authenticate the provider or account.
The host must populate the request from its original immutable, owned C5 intent
and canonical-account witness, rather than accept an arbitrary caller submission
or derive original identity from a newly selected account.

## Explicit adoption

A submit operation opts in through its declared, required dependency:

```json
{
  "dependencyId": "broker.lookup",
  "capability": "broker.order_lookup.paper@3",
  "operationId": "broker.order_lookup.paper_v3",
  "required": true
}
```

This belongs in the existing operation `dependencies` array. The host must
validate and bind that declaration to the original provider/package/instance
and current read permission before dispatch. Declaring a dependency does not
grant access. Core graph authors must supply an explicit binding for the child
node `<submitRequirementId>::broker.lookup`, including the same verified
plugin instance, package and account reference; dependency expansion does not
inherit account authority. A missing child account binding correctly blocks
graph preparation before provider acquisition. No V3 selection based on
availability, probing or silent fallback
is permitted. Without that explicit V3 dependency, existing V2 lookup behavior
and wire bytes remain unchanged. A present but invalid or unavailable V3 binding
must refuse rather than downgrade.

This SDK module does not implement host routing, dependency resolution, intent
migration or provider I/O. Previously retained legacy requests remain legacy;
they must not be silently rewritten on recovery.

## Provider comparison and response

The response remains `broker_v2::OrderObservation`, with wire version
`tradeassembly.broker_order_observation.v2` and schema
`schema://tradeassembly/broker-order-observation@2`.

The provider must independently confirm its exact broker namespace, paper
environment/endpoint and observed account ID against `expectedAccount`. It then
performs a lookup by the original `order.clientOrderId`; this read never submits
an order. Before echoing the supplied original order, it must compare every
observable provider field for its supported order model: client order ID,
symbol/asset, side and position intent where supported, quantity, limit price,
time in force and order class/type. Exact decimal values must not round-trip
through floating point. Missing, ambiguous, substituted or incompatible required
fields cannot be converted into a matching order or definitive no-fill result.

Opaque host instrument ID, family, quantity-unit and multiplier/currency labels
are supplied host identity, not facts recoverable from an arbitrary provider's
order API. They may be echoed only after all available provider comparisons and
supported-model checks succeed. The provider must refuse unsupported models or
leave the lookup inconclusive instead of inventing identity mappings. A matching
host echo does not itself prove that the provider performed those comparisons.

`LookupRequestV3::validate_observation` performs ordinary V2 observation checks
at the actual bounded host clock and requires exact original order/account
identity. The host must also validate the operation source, original retained
request/intent, response schema and recovery authority. Unknown fills and cash
remain `unavailable`, never assumed zero. Existing V2 rules require unavailable
fill quantity to use `status: "unknown"`; a definitive filled status needs
actual quantity evidence. Cash may remain unavailable even when order status
and fill quantity are known. No fee-inclusive debit is inferred from quantity,
average fill price, a limit quote or a balance difference.

An order observation is not final-fee or account-posting evidence. This request
adds no atomic account checkpoint, cash/close-settlement capability, hold release
or permission to trade. A fresh provider process can verify the supplied
original host identity without a process-local submission cache; actual provider
and Core process-restart qualification is still required separately.
