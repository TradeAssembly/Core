# Local binary setup (F2 implementation preview)

This describes the binary command surface, not an available public release.
Public downloads and signing/notarization still need release acceptance.
An unsigned local candidate now bundles Alpaca and its sandbox dependencies.
Do not send users to build from source as the
released installation procedure.

In a release package, `tradeassembly` and its compatible `warden` executable live
side by side. An agent can run:

```text
/absolute/package/bin/tradeassembly setup --state-dir /absolute/new/installation
```

For an isolated developer acceptance run, `--warden-binary /absolute/path/warden`
selects a prebuilt Warden and `--warden-port PORT` selects a loopback port. No Cargo
or source files are used by the setup command itself. The executable version is
checked and its digest recorded; signed release provenance remains a separate
packaging requirement, not something this initial local digest establishes.

The structured result returns `runtimeConfigPath` and argument arrays in
`nextCommands`. Run commands as executable plus arguments; do not concatenate
paths into a shell string. It is legitimate for a path to contain spaces.

1. Run `tradeassembly policy serve --state-dir /absolute/new/installation` in
   the foreground. A process supervisor can own this process; the command does
   not create global PID files, kill other processes, or install launchd jobs.
   On macOS, `tradeassembly policy launchd install --state-dir
   /absolute/new/installation` is the unattended alternative. It installs a
   user-scoped policy service with an installation-specific label. `verify` and
   `status` inspect supervision; `uninstall` removes only that owned service and
   preserves state/logs. These commands do not activate/deactivate strategies or
   liquidate positions. Supervision status is not broker/strategy health.
   This is a LaunchAgent: it runs in the signed-in macOS user session. It does
   not keep the computer awake or execute while the machine is powered off.
2. Configure the agent's MCP server with the absolute `tradeassembly` executable
   and arguments `--config`, the returned config path, `mcp`, `serve`.
3. Run the same executable with `--config /absolute/new/installation/runtime.json
   plugins install-default --offline`. A bundle selects its pinned Alpaca archive
   and verifies its package/manifest digests; a missing bundled archive fails
   instead of falling back to a different remote version.
4. Call `tradeassembly.setup.inspect` with `{"surface":"agent"}`. Local identity
   does not require Hub. Broker verification and restart proof are still required.
   Omitting `surface` preserves the separate Studio acceptance path.

Setup generates local policy keys and a private token, embeds the compatible
policy/PEP assets, and separates Warden's database from Core's database. It does
not collect broker credentials, select investments, install a trading strategy,
start an agent, or activate trading. The result intentionally reports
`policyRunning: false` and `automationReady: false`.

Repeated setup against the same recognized installation preserves identity,
keys, token and databases. Incompatible executable/port changes, changed assets,
unsafe permissions or unrecognized existing state fail closed. Use a new state
directory for a clean test; never reuse or reset someone else's rig. Upgrades
and rollback need a dedicated migration procedure before public distribution.

Setup provisions policy state and, when run from the bundle, records the absolute
bundled sandbox launcher in runtime configuration. Node and SRT are bundled;
customer npm/Node installation is not required. `sandboxConfigured` reports that
configuration, not broker connectivity or strategy readiness. Plugin installation
is the separate command above. Policy/agent supervision and the end-to-end
broker/agent journey remain required F2 work; setup alone is not complete onboarding.

## Browser connection through MCP

A distributor may bundle public `bin/connection-profile.json` configuration.
The binary validates it; users do not supply client IDs, issuer URLs, or broker
application secrets. Invalid or missing hosted configuration fails closed without
disabling account-free local Core.

Call `tradeassembly.onboarding.start` with a user-selected `mode` (`paper` or
`live`), an `idempotency_key`, and optionally `relay: true`. Open the returned
`browserUrl`. The small local page handles sign-in, displays permissions from the
installed broker plugin, and starts broker authorization. It does not require
Studio. Do not ask users to paste OAuth codes, credentials, or configuration.

Poll `tradeassembly.onboarding.status` with `onboardingId`. After an MCP restart,
the same ID reopens the durable attempt at a newly bound browser URL. Reusing a
start key with different settings is rejected. `tradeassembly.onboarding.cancel`
ends the attempt without disconnecting an already stored broker credential.

`ready: true` requires verified hosted identity, broker account verification,
local Warden availability, acknowledged broker handoff, and any requested Relay
entitlement. It does not activate a strategy or place an order. A local-owner
session is not proof of Hub authentication. Provider authorization, subscription
checkout, and installed-client verification remain separate release evidence;
passing `cargo xtask onboarding-verify` proves only the local deterministic gate.

## Optional hosted sign-in diagnostics (distributor only)

Account-free local setup does not require WorkOS sign-in. When using an optional
hosted connection, the distributor supplies the registered product client and
issuer; a Hub browser cookie is not a local product session.
When Hub access is organization-scoped, set `oidcOrganizationId` (or
`TRADEASSEMBLY_AUTH_ORGANIZATION_ID`) to the selected WorkOS organization. Core
passes it to AuthKit for both initial authorization and refresh; without an
organization context, WorkOS does not issue role or permission claims.
Use the issuer advertised by the registered client's provider discovery document;
do not construct it by appending an application client ID. A WorkOS application
can advertise an environment-level issuer while retaining its own client binding.

The CLI reports fixed error codes, never token contents:

- `workos_token_issuer_mismatch` or `workos_token_client_mismatch`: check the
  distributor's issuer/client pairing and registered application environment.
- `workos_token_expired` or `workos_token_not_yet_valid`: check the system clock
  and perform a fresh sign-in; do not reuse an authorization code.
- `workos_token_header_invalid`, `workos_token_signing_key_unavailable`,
  `workos_token_signing_key_invalid`, `workos_token_signature_invalid`, or
  `workos_token_claims_invalid`: report the code to the operator for diagnosis.
  Do not disable validation or copy credentials/tokens into a support request.

These errors do not prove their underlying cause without further verification.
They do not authorize changing broker credentials or activating a strategy.

## Optional Bitwarden custody for hosted sign-in

WorkOS sessions can use the customer's Bitwarden vault instead of the desktop
keyring. This does not change broker permissions or require a Hub account for
local-only Core. Install the official `bw` CLI and provide its unlocked
`BW_SESSION` through your agent/service environment. Never put it in prompts,
command arguments, logs, or runtime.json. `TRADEASSEMBLY_BITWARDEN_CLI` can select
the installed CLI's absolute path. Unlocking the browser extension alone does
not unlock the CLI.

For an existing keyring session, run `tradeassembly --config CONFIG auth
migrate-to-bitwarden`. This validates the existing identity, copies the session
into a namespaced secure note, verifies read-back and preserves the legacy entry.
An existing different Bitwarden session is a conflict, not permission to
overwrite it. Only after success set `oidcSessionStore` to `bitwarden` in CONFIG
(or `TRADEASSEMBLY_AUTH_SESSION_STORE=bitwarden`) and verify `auth status` in a
new process. Keep the same issuer, client and session path: they identify the
vault entry. For a new session, select Bitwarden first and sign in normally.

No automatic fallback to keyring occurs when Bitwarden is locked, unavailable
or returns ambiguous matches. CLI operations time out after 30 seconds, and
provider responses never appear in errors. The session's rotating refresh
credentials are saved and verified in the selected store. Logout soft-deletes
only that session's Bitwarden item. The retained legacy keyring entry is recovery
material and is not automatically deleted; explicitly retire it after verifying
the migration. Other macOS/application credentials are outside this command.

## Durable backtest requests

`tradeassembly --config INSTALL/runtime.json backtest --request-file REQUEST.json`
is the same create operation as `backtests create --request-file REQUEST.json`.
The request supplies the user's immutable strategy version, dataset snapshot,
configuration and idempotency key. Missing inputs fail; the command does not
choose a strategy or fabricate a completed research result. Creation queues the
run. Use `backtests get RUN_ID`, `backtests process`, `backtests replay RUN_ID`
and `backtests export RUN_ID` for its actual lifecycle and evidence.

The legacy `/product/strategies/research-runs/create` HTTP route and
`RunStrategyResearch` GraphQL operation now alias this durable lifecycle. Their
old synthetic `ResearchRun` completion response is intentionally not preserved.
GraphQL accepts the explicit create payload in `request`.

## Processing an authenticated backtest

Pass the exact returned run ID to `tradeassembly.backtest.process` as `run_id`,
with an idempotency key. The CLI equivalent is
`tradeassembly --config INSTALL/runtime.json backtests process --run-id RUN_ID`.
This processes only that owner's run through the same durable lease and fenced
completion path as the internal worker. Repeated calls do not rerun completed
work. Calls without a run ID cannot access the global worker queue while
authenticated. No broker orders are submitted by the deterministic backtest.

SQLite supports atomic partition-scoped claiming. Adapters that have not
implemented it return `queue_partition_claim_unsupported`; there is no fallback
to claiming other runs or clearing authentication. Internal unscoped worker
operation remains separate. GraphQL `ProcessBacktest` follows the same exact-run
authorization and scoped claim rules.

Backtest report export retains its protected canonical action. Its implicit
idempotency key includes the durable run revision, so exporting before completion
does not cache a missing-result error forever. Explicit caller keys retain exact
request replay semantics.

## Owner publication

Research tool discovery includes the full nested input contracts: use
`backtest.run.inputSchema.properties.request.properties.configuration` for
capital, risk, execution, costs, dataset provenance and instrument metadata.
Ingestion discovery specifies each normalization/quality policy field and enum.
These schemas are generated from the same Rust types used to accept requests.
An empty backtest `capabilityGraph` revision/fingerprint requests resolution of
a new backtest graph; the dataset binding retains its original ingestion graph.
Capability evaluation mode is separate from broker-account authority.

Dataset MCP create/list/status/get return metadata without observations by
default. For rows, call `tradeassembly.dataset_ingestion.get` with
`observation_offset` and `observation_limit` (0–1000). Follow `observationPage`
pagination. The retained snapshot hash identifies the complete immutable stored
dataset, not the returned page. Backtesting reads that stored dataset directly;
agents need not download its rows. An idempotent backtest create may replay its
original queued acknowledgment; use `backtest.get` for current durable state.

Agents can discover the embedded contract with `tradeassembly.strategy.schema`.
`tradeassembly.strategy.create` in blank mode creates an incomplete draft without
instruments, provider selection, or trading rules. Encode only owner-supplied
rules. Call `tradeassembly.strategy.validate` with inline `spec` JSON for
structured validation without saving, or `strategy_id` to validate an owned
saved draft. This tool does not load `spec_file` paths. Validation does not
publish, activate, or prove research results.

The schema response also includes `evaluators`: machine-readable configuration
schemas, exact pipeline/exit/calendar requirements, units, and supported execution
paths. Numeric strategy and risk values have no supplied defaults. Use these
contracts rather than inspecting source. The portfolio evaluator supports
provider-VWAP one-minute research with static equity/fund selectors and immutable
calendar evidence. It does not make the stateless deterministic runner support
the same strategy.

Validation returns `compilation.backtest` and `compilation.statelessExecution`
separately from schema `valid` (inside `body` for saved-draft validation). Check
the supported flag and reason for the intended path. A structurally valid draft
may still have unsupported semantics. Compilation is not an authorization grant,
a successful backtest, or permission to promote or activate a strategy.

After reviewing a draft and recording its exact hash, the local owner may
publish it with the standalone CLI:

```text
tradeassembly --config INSTALL/runtime.json strategy publish STRATEGY_ID \
  --expected-draft-hash HASH --acknowledge-publication \
  --idempotency-key UNIQUE_KEY
```

The acknowledgement flag is an owner protocol acknowledgement on the
owner-controlled machine; it is not proof of physical human presence. The
command uses the existing local owner authentication and publication service,
rejects a missing acknowledgement or empty hash before mutation, and never
activates execution. Agent MCP publication remains denied.
