# Extraction decisions

- The named 2026-10-03 personal-signup repair changes candidate source, not the
  retained frozen payload. A verified no-organization session defers personal
  self-service authority to Hub's registered-client policy; exact subject and
  derived tenant bind custody/restart/refresh. Organization bindings and role
  permissions remain enforced. Changed binaries and dependent observations must
  be qualified through existing owners; prior qualification is not inherited.

- Frozen OAuth source-fault proof is separate from packaged production positive
  proof. Do not change production profiles, trust or transport to inject faults.
  Compile the descriptor's archived Core revision and dependency lock with only
  an exactly reconstructible test overlay. Preserve native bytes and accepted
  producer pins. Source fault results never imply full production onboarding.

- Registry qualification is additive evidence, not a rewrite of native
  qualification. The first-release published gate requires a separate typed
  receipt bound to the original native receipt, five-package registry capture,
  fixed passing process logs and exact test sources. It runs existing installer
  upgrade/rollback acceptance against real captured registry bytes. The native
  candidate gate and frozen shipping payloads remain unchanged.

- Frozen F2 macOS arm64 bytes and the M0–M6 parent release lock are not rebuilt or
  re-signed for npm delivery. New targets need independent native evidence. The
  first distribution format accepts only prerelease versions on `beta`; no Apple
  payment/notarization or lifecycle install scripts. Warden digest/state migrations
  are excluded from v1 upgrades, which require a stopped rig and preserve identity.
  Distribution proof is distinct from runtime and public-release qualification.
  Native freezing requires the actual host, exact input digests and pinned
  Node/SRT versions. It cannot replace controlled-sink or OS-enforcement tests.

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
- The historical 2026-09-22 `oauth_relay_unavailable` observation is superseded
  by a live recheck: both `connect.tradeassembly.ai` and
  `staging.tradeassembly.ai` now forward a malformed start request to the
  versioned Relay, which returns its bounded Axum validation response. Hub remains
  product-neutral and must not become a provider-credential proxy. The
  product-scoped Relay authenticates with Hub identity and returns a short-lived
  authorization result to the local credential store; the flow must never ask a
  user to paste broker API keys into an agent chat. The authenticated packaged
  browser journey, account verification, restart, and local credential readback
  still require current bound evidence before F2 browser OAuth is complete.
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
- Customer hosted connection profiles do not force an operator's WorkOS
  organization. `organizationId` may be null/omitted; signed identity and Hub's
  personal-tenant rule remain authoritative. Explicit nonempty organization
  scopes retain their binding; blank scopes are invalid. Endpoint/client and
  entitlement checks are unchanged. A binary built before this contract repair
  is not qualified for the tenant-neutral profile and must not be relabeled.
- October 3 approved verification coverage: one full nextest unit/integration pass
  plus explicit workspace doctests replaces duplicate full cargo-test/nextest
  execution. Security/source/public gates and release opt-ins remain unchanged.
  Cheap strict release prerequisites run first; dirty development is an explicit
  mode, never release proof. Non-runtime harness/docs changes do not independently
  invalidate unchanged native behavior; retain exact producer and verifier refs.
- No scheduler rewrite, extra brokers, UI buildout or hosted agent orchestration
  is introduced by this extraction packet. Frozen F2 M0–M8 criteria are unchanged.

## Local CI setup phase

`just setup` prepares locked dependencies; `just check` runs developer feedback;
`just build` produces local native binaries; `just verify` preserves the complete
clean-source gate. None requires Codex or a CI-provider run. The coordinator's
local CI manifest owns separate integration tests and records gaps; no deployment
or product repair is authorized by a passing local baseline.
