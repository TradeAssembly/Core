# Non-atomic provider observations

A provider may report an order fill before it can prove final fees, account
posting, or a coherent account checkpoint. The host records these observations
without treating them as settled cash. Unknown values remain unavailable.

## Explicit selection

A typed submit operation selects this path through required dependencies named
`broker.cash_settlement` and `broker.close_settlement`, bound to the respective
`broker.cash_settlement.paper@2` and `broker.close_settlement.paper@2` read
operations. Both reads use purpose `recovery`. They must resolve to the original
provider instance, immutable package, canonical paper account and strategy graph.
Merely installing an operation does not select it. Legacy coherent V1 settlement
continues to use its existing contract and purpose.

Each request carries the original submit intent and observable account identity.
The provider checks actual account and order fields before returning opaque host
labels. A lookup never authorizes another submit.

## Observation ledger

An immutable receipt binds the owner, plan, original intent, provider source,
response, acquisition time, prior receipt and conservative projection. A head
tracks the last accepted facts and any disputed observation. Receipt, head,
idempotency key and managed-position changes commit atomically against the
original submission and cash-reservation versions.

A new observation may advance known quantities and evidence clocks. Known fills,
positive debits and provider identities cannot disappear or regress. Conflicting
same-time evidence and unsupported terminal corrections block reconciliation;
they do not silently replace the accepted facts. A repeated observation does not
apply a fill twice.

The local HTTP retry envelope can add `duplicate` and `commandId`. The original
domain receipt remains unchanged. Returning that historical receipt does not
make its provider data current or grant execution authority.

## Positions and reservations

Strategies must explicitly select `entryBasis: provider_reported_average_fill`
and declare both observation dependencies to manage terminal observed entry
fills before cash reconciliation. Such positions use the V2 position form with
an observation receipt pin and `entryCashReconciled: false`. They are not labeled
as having a final settlement receipt. A disputed entry basis blocks both reversal
and price-based exit admission.

Terminal close observations can reduce the owned quantity once. The host retains
the applied-close receipt and quantity so a retry or later coherent settlement
cannot double-count the close. A cancelled partial close leaves the remaining
owned quantity available for management. Refreshing the entry selection or
pausing new entries does not remove existing managed positions.

Non-atomic observations never credit proceeds, release cash holds, or create a
new capital checkpoint. Only a separately validated coherent posting can do so.
Consequently, available capital may remain conservatively reserved even when an
order appears filled. Fresh quotes, current permissions and independent C5
admission are still required for every close submission.

## Compatibility and current boundaries

Existing V1 positions omit the new optional fields and retain their original
wire representation. Existing V5 financial and V6 management transports reject
the new observed-entry semantics; a new finite transport contract is required
before exposing them remotely. Open partial-entry cancellation and management
are not established by this path. Terminal cancelled-partial entries and closes
remain distinct from open orders. No provider-native finality, real brokerage
coverage, or unattended execution is implied by these interfaces.
