# Reported cash capacity and net base inventory

These additive SDK contracts describe observations. They do not activate a
cash-only funding policy, authorize an order, establish zero borrowing debt,
release capital, or prove strategy-owned inventory. Existing account and
coherent settlement wire formats retain their prior meanings.

`account.cash_capacity.paper_v1` / `account.cash_capacity.paper@1` is a read
with purpose `execution`. It binds an explicit paper account and crypto
instrument, preserves unknown debt and permissions, and exposes reported
cash/non-marginable capacity with sampled evidence. Its reported ceiling is
not a capital reservation or a venue/external-writer fence.

`broker.net_base_inventory.paper_v1` / `broker.net_base_inventory.paper@1`
is a read with purpose `recovery`. It binds the original V3 lookup and entry
order. Gross fills, original-order fees, net credited base asset and account
aggregate inventory have separate fields and meanings. Direct order/fill
links differ from an order-filtered query. Exhausting pages does not establish
that later fees or corrections cannot arrive. Known net credit requires
complete fills and independently evidenced order-specific final fee closure.
An aggregate position may cap independently proven ownership; it cannot
establish that ownership or allocate unrelated holdings to a strategy.

Core currently refuses managed reported-entry crypto planning/current
admission and live entry-basis use with
`managed_order_attributed_net_inventory_required`. The required host-owned
lot reducer and authoritative provider fee/net observations are not yet
admitted. Historical inspection and original observation/lookup recovery
remain separate. Merely installing these definitions or declaring a provider
operation cannot enable managed crypto execution.

A future host lot reducer must atomically retain original order attribution,
base fees, applied closes, pending reservations and disputed revisions.
Fresh account availability is only a ceiling after this ownership proof.
No provider support, current entitlement, authoritative fee completeness or
actual broker qualification follows from the presence of these SDK schemas.
