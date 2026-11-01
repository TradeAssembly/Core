# Non-atomic paper settlement observations V2

This SDK contract describes provider reads. It is separate from coherent
`cash_settlement`/`close_settlement` V1 and never authorizes reservation release,
position registration/closure, cash credit or an order submission.

The two exact operation/capability pairs are:

| Role | Original position action | Operation | Capability |
| --- | --- | --- | --- |
| `entry` | `buy_to_open` | `broker.cash_settlement.paper_v2` | `broker.cash_settlement.paper@2` |
| `close` | `sell_to_close` | `broker.close_settlement.paper_v2` | `broker.close_settlement.paper@2` |

Other position actions and swapped operation/capability pairs refuse. These
bindings are not aliases for V1 and their declaration grants no access. Core
must explicitly select, authorize and bind the intended read. The SDK does not
implement Core routing or an availability-based fallback.

`settlement_observation_v2::Request` has four required fields:
`schemaVersion: tradeassembly.settlement_observation_request.v2`, `role`,
`lookup` (the complete original LookupRequestV3), and `hostAccountRef` (the exact
original host alias). The account alias is supplied, not derivable from provider
account ID. The host must populate these from the original verified immutable
intent/account/source binding, not arbitrary caller fields or a newly selected
account. Existing LookupV3 paper, exact account-hash, decimal and limit-order
validation applies. Request identity is canonical SHA256 of the entire request.

The shared response schema is `schema://tradeassembly/settlement-observation@2`,
wire version `tradeassembly.settlement_observation.v2`; request schema is
`schema://tradeassembly/settlement-observation-request@2`. `request_schema()` and
`response_schema()` return the closed JSON schema values. SDK semantic checks
also enforce account/order equality, request hash, evidence references, clock
ordering, component coverage and role binding; structural schema alone does not.

Every response has `requestSha256`, `status`, `coherence`, `order`,
`averageFillPrice`, `feeCash`, `feeFinality`, `account`, and `evidence`.

- Status is only `pending` for unknown/open/partial order states or `provisional`
  for terminal order states. Neither is a finality/readiness claim. A filled
  order with unavailable cash/fees/account is still only provisional.
- Coherence is always the exact string `non_atomic`. No sequence, ledger epoch,
  posting ID, checkpoint or host-reconciled status is accepted.
- Order is the existing exact V2 observation, bound to the original host order
  and canonical account. Supplied host identity is echoed only after the
  provider independently verifies all observable account/order fields. Unknown
  fill/cash remains `unavailable`; a negative sale credit cannot be cashSpent.
- `averageFillPrice` is `Observed<ProviderReportedAveragePrice>`, with explicit
  `basis: provider_reported`, exact positive Decimal, currency and order evidence
  references. It is a unit price, not a gross amount or cash debit. No SDK code
  multiplies quantity by average/limit price or fills missing cash.
- Fee amount and fee finality are independently `Observed`. Actual zero fees
  need provider fees evidence; they are not a paper default. `final: true` is
  only a provider-reported claim requiring actual supporting provider material,
  never an inference from order terminality, an activity ID or equal balances.
  Structural validation of its reference does not prove a provider guarantee.
  Without such a guarantee, finality must remain unavailable. Even a supported
  fee-finality report does not prove final coherent settlement.
- `account` is unavailable or a `NonAtomicAccountSample` containing the existing
  AccountSnapshot and explicit independent account/positions/open-order sample
  intervals/evidence. Paper account ID and supplied host alias must match.
  Available facts and complete coverage flags require corresponding samples;
  missing debt, positions, permissions or cash never become zero/empty/false.
  Missing host mappings for unrelated instruments must remain unavailable rather
  than being reconstructed from provider symbols. No mapping registry is added.
- All availability wrappers are required. Missing/null objects and unknown
  fields refuse. Existing exact Decimal and Currency bounds apply; reported
  values are never silently rounded. Fee/account currencies are retained; this
  read supplies no conversion or cross-currency reconciliation.

The account snapshot time must use `host_acquired` and represent aggregate read
completion. **Per-component evidence/read intervals remain authoritative.**
Independent order, fee and account reads do not establish a shared knowledge
instant or provider checkpoint, even with equal timestamps or balances. The
aggregate label cannot replace individual sampling times or be admitted as a
coherent financial witness. Provider time claims are checked at actual host now;
no clock is rewritten or backdated.

Evidence contains at most 64 unique local IDs, a finite read kind, optional
provider-record availability, `hashBasis: raw_response_bytes`, raw response
SHA256 and actual start/completion timestamps. Field/component reference lists
are nonempty, unique, bounded and resolve to the appropriate kind. Samples must
enclose their referenced reads. Digests and record IDs identify source material;
they are not authentication, monotonic revisions or finality guarantees. An
unknown lookup without any provider response may have no evidence; known order
facts require order evidence. No raw response, endpoint, header or credential
appears in this DTO.

Requests are bounded to 32 KiB and responses to 1 MiB before decoder allocation;
normal outer-envelope limits still independently apply. Existing account list
bounds remain in force. SDK validation can run on typed values, but does not
verify that a provider actually performed a read or owns a finality guarantee.

The host must separately persist immutable observations, retain original
request/source/owner/current-authority bindings, reconcile corrections and
independent account mutation, and decide whether independently sufficient proof
exists. Reservations remain unchanged by this DTO. Current Core managed-position
registration/basis and coherent settlement paths still require final V1
settlement evidence; these new types do not solve that host-ledger integration.

The SDK test fixtures separate structural schema validity from semantic
validation. The owning runtime JSON Schema gate must validate the generated schema
against those same fixtures; SDK serde/semantic tests alone are not a schema
engine or provider/process qualification. No actual provider capability/finality
or full financial-adapter completeness follows from this module.
