# Extraction decisions

- `portfolio_reservations` is the initial accounting kernel, not an authorization
  tool or activated broker feature. It uses an owner/account-scoped atomic storage
  expectation to reserve observed gross exposure plus all unreconciled holds,
  counts unique instrument slots and uses integer micros. Duplicate request IDs
  must match the exact intent. Pending holds prevent policy rebinding, expiry does
  not reclaim them, and there is deliberately no release operation yet. Callers
  must verify account snapshot provenance and authority before invoking it.
  Broker integration, trusted snapshot acquisition and terminal reconciliation
  remain required before advertising portfolio controls. Conservative double
  counting of observed pending orders is intentional until reconciliation exists.
- Portfolio admission, when wired, consumes a durably stored
  `account.portfolio_state.read@1` receipt through the same request, prepared-binding,
  package, credential-generation, capability-graph, and account binding checks used
  for quote receipts. The receipt must be fresh, complete, non-reconciling and bound
  to the active run's owner and account; caller-supplied snapshots are never accepted.
  Its non-atomic provider observation is permitted only within the fixed freshness
  bound and is combined atomically with local holds. Every position and open order
  must have a parseable gross notional; an unknown open-order notional, incomplete
  page, stale receipt, changed binding, or storage conflict denies admission. The
  proposed order is grossed even if it may reduce an existing position. The capacity
  hold uses the current mandate's execution-config digest as its authority digest and
  the order idempotency key as its request ID. The hold is taken before C5 and is
  re-read idempotently after C5; it does not itself authorize broker dispatch.
- This decision does not yet enable aggregate portfolio controls. That integration
  requires an explicit configuration schema and capability-graph requirement for the
  portfolio-state operation, receipt generation by the agent, deterministic receipt
  verification before C5, and terminal-reconciliation release semantics. Until those
  pieces have passing controlled-sink proof, the current live boundary continues to
  enforce only its declared per-order controls and must reject unsupported controls.
- External-agent configurations may explicitly supply `allowedSymbols` (MCP
  `allowed_symbols`): 1–256 unique exact broker symbols, no aliases or implicit
  primary symbol. The universe participates in generated configuration identity
  and the full configuration digest used by mandates. Malformed lists and lists
  for deterministic evaluators fail closed. Broker admission checks membership
  on each current-state recheck. This does not authorize aggregate risk controls.
- Without an explicit universe, broker admission requires an exact, nonempty execution-config symbol match,
  rechecked with the current binding before dispatch. No implicit aliases or
  default instrument expand authority. This closes a missing constraint in the
  existing single-instrument path; it does not implement portfolio authorization.
  Full owner-strategy activation and aggregate exposure/position reservations remain
  release gaps for the owner's ten-symbol strategy. Do not substitute a narrowed
  strategy or per-order limits and claim its full paper promotion is verified.
- Live cloud prerequisites apply to non-local or unidentified runtime profiles,
  not an explicitly resolved local runtime manifest. Callers cannot select this
  exemption through activation request fields. Local durability and all authority,
  legal, account and risk checks still apply. Live readiness rejects controls the
  broker boundary cannot enforce rather than silently ignoring them; current
  supported pre-trade limits are order quantity and order notional. Daily-loss and
  position controls are not claimed by this path.
- Local Live activation does not require hosted evidence or a Relay entitlement.
  Its legacy `hosted_evidence_policy` readiness ID now reports durable local
  evidence availability (kept for client compatibility). Evidence adapters default
  to unavailable; SQLite verifies a file-backed database, WAL and FULL-or-stronger
  synchronization. Availability is advisory, not authorization or a write guarantee:
  activation must append and read back the exact receipt before its atomic domain
  commit. Missing, conflicting or unreadable evidence prevents activation. This
  does not claim remote archival, disaster recovery or qualified Postgres support.
  Owner-mandate approval, legal/account checks and per-order Warden enforcement
  remain separate gates; no shipping Live policy is changed.
- Use the owner-approved Apache-2.0 Core grant; preserve OptionLab LLC legal
  attribution and third-party rights. Validate actual LICENSE/NOTICE, not a
  readiness flag alone. Exact binary distribution notices remain a separate gate.
- Preserve the Studio repair source; copy and qualify Core independently. Remove
  uncompiled Studio tooling only after exact source comparison. The removal
  inventory is docs/removed-studio-tooling.json.
- Core archive proof uses an immutable clean Git revision, an isolated locked
  Rust build, and actual CLI/MCP setup discovery. It must not start Studio or
  substitute fake Warden health for authority evidence. Actual Warden/control
  sink proof remains the separate unchanged M2 gate.
- A private local checkpoint enables source/archive checks; it is not a release
  approval, pin update, merge, or public publication.
- No scheduler rewrite, extra brokers, UI buildout or hosted agent orchestration
  is introduced by this extraction packet. Frozen F2 M0–M8 criteria are unchanged.
