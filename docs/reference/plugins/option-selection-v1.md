# Option selection V1

Selection is a read-only decision stage. It does not size, price, authorize or
submit an order. The existing static execution kernel rejects this contract.

A mandatory fail-closed `screen` substep in `stages.intent`, with operation
`option.contract.select@1`, may declare the closed `option_selection` object:

- literal version `tradeassembly.option_selection.v1`;
- `signal_requirement_ref` and `underlying_selector_ref` identify the context
  instrument; `trade_requirement_ref` separately identifies an option-contract
  trade market linked to that signal requirement;
- `chain_capability_requirement_ref` identifies an explicitly declared,
  versioned capability whose data shape is `option_chain`;
- `filters` declares call/put, inclusive ISO expiry dates, inclusive UTC calendar
  DTE and strike ranges, signed delta range, minimum volume/open interest, and
  maximum relative spread `(ask-bid)/((ask+bid)/2)`;
- `analytics` chooses provider-supplied values or explicitly identifies a model
  capability and semantics version. This implementation executes provider-supplied
  analytics only. Model requests fail unavailable, never fall back;
- `ranking` is an ordered nonempty list of typed fields, ascending/descending
  directions and optional distance targets. Exact ties use contract ID ascending;
- `missing_data` explicitly rejects the candidate or fails the stage;
  `no_match` explicitly returns no eligible contract or fails the stage;
- `max_age_ms`, `max_contracts`, and `max_pages` bound freshness and work.

A zero metric is distinct from missing data. Invalid market values, crossed
quotes, stale/future snapshots, wrong identity, duplicate IDs and incomplete
chains cannot silently become eligible contracts. Zero-mid spread is unavailable.
DTE uses UTC calendar dates, not an exchange-session eligibility decision.
Broker eligibility and actual expiration/session restrictions belong to subsequent
order planning and risk checks; selection grants none of that authority.

## Product entry point

`POST /product/strategies/option-selections/create` and MCP tool
`tradeassembly.strategy.option_selection.create` accept a `selection` command
plus `idempotencyKey`. The command names scope, current graph revision,
acquisition node, mode, published substep, original signal step/idempotency key
and selected signal event end time. Callers cannot supply signal values, chain
responses or replacement filter configuration.

The host loads the owned published policy and an existing hash-verified causal
entry-signal receipt from the same strategy/plan/graph. It requires host-admitted
source captures and an actual underlying output reference. The underlying
selector must currently name a normalized instrument. It then invokes the exact
pinned read operation through the existing credential host and verifies bounded
response schemas, observation times, source class and all pagination before
ranking. The current normalized wire profile uses `sourceClass: production`,
`observations` with typed `option_contract` data, and explicit `nextPageToken`.
The provider operation must declare that profile through its verified package;
merely configuring a provider name is insufficient.

Current-chain acquisition supports paper/live read contexts only. Backtest chain
replay is explicitly unavailable until historical chain evidence and replay are
integrated. Neither current market snapshots nor model estimates are substituted
for unavailable historical/provider inputs. Discovery advertises these limits.

Selection evidence binds signal hash, package/manifest/config/account context,
actual acquisition payload hashes and timestamps, and all ranking/rejection
results. Receipt and idempotency record commit atomically. Cached responses
recheck current ownership, freshness and bindings. The public V1 view omits raw
pages and pagination cursors, retaining acquisition summaries and the immutable
private receipt ID. This preserves redaction and stable retry responses.

Retries recheck acquisition, response and selected-quote timestamps against the
published maximum age. An expired decision returns `option_selection_receipt_stale`
without changing its receipt or reacquiring a chain. Command identity is stable
across HTTP and MCP for the same actor, account mode, strategy and idempotency key.
Earlier transport-scoped indexes remain readable; divergent legacy decisions fail
closed. New signal receipts retain their original request authority as hash-bound
evidence for cross-transport reads. Downstream calls still use current authority.
Legacy signal receipts preserve their serialized form and original verification
behavior.

A successful HTTP response means a receipt was recorded. Clients must inspect
`selection.result.outcome.status` and `stageFailed`; no eligible contract is not
a selected contract, and the response explicitly grants no execution authority.

## Qualification boundary

Local Rust subprocess/loopback fixtures exercise the product path from captured
underlying data through published signal evaluation, chain acquisition,
deterministic selection, restart/idempotent retry, no-match, incomplete-chain
refusal and source revocation. They are synthetic provider-wire qualification,
not authenticated market coverage, broker execution or release acceptance.
Actual adapter adoption and entitlement coverage require separate qualification.

The legacy `/marketdata/options/chain` and `/marketdata/options/select` endpoints
return unavailable by default. Their retained historical examples require
explicit `fixture: true`, identify `deterministic_fixture` and provider `fixture`,
and cannot fall back to the first contract when no filter matches.
