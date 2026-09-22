# Extraction decisions

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
