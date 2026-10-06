# Extracted Core architecture

Verification remains a fixed Rust xtask command plane. The approved October 3
coverage amendment executes full unit/integration tests once through nextest and
doctests explicitly, preserving scanners/security and separate release opt-ins.
Public Core preflight checks only local inputs/tools/revision/headroom; hosted
authentication and service qualification remain the owning release job's boundary.

Core is a portable Rust workspace. CLI, HTTP, GraphQL and MCP share the runtime
service/port boundary. identity-sdk, plugin-sdk and sightline-sidecar are internal
workspace crates. Brokers and Warden remain explicit external executable or
package contracts, not private sibling-source dependencies.

OAuth fault qualification uses an archived frozen Core source tree with two
test-only identity/HTTP boundary hooks. Production control flow and shipping
bytes remain unchanged. Independent real local custody/SQLite readback at a
scripted ACK sink proves source ordering and distinct-process lost-response
recovery, not deployed identity, provider HTTP or packaged fault execution.

Hosted connection profiles contain public deployment inputs only. Their optional
`organizationId` is a deliberate operator scope, not a required customer default.
Customer profiles leave it null/omitted, allowing WorkOS and Hub to derive the
tenant from verified identity (including Hub's documented personal scope).
Explicit organization profiles still enforce that organization. No profile may
supply an actor, tenant authority, credential or entitlement in place of Hub.

The named personal-signup candidate amendment permits a verified token without
an organization to request Hub's real self-service projection without a role
permission claim. Custody requires exact subject and `user:{sub}` tenant binding;
stored-session restart and refresh retain that binding. Organization sessions
still require their configured binding and identity permission. This source
repair does not inherit the frozen binary's qualification; distribution gates
must requalify changed native bytes and dependent captures.

Distribution is a separate Rust installer around locked payload artifacts. npm
glue selects an exact-version native package; it never builds customer source.
Immutable payloads, a stable digest-bound Warden authority path, persistent state,
and a shared launcher/exclusive installer lock keep delivery separate from runtime
semantics. Installation registers the bundled Alpaca package through real local
Warden, then reaps that temporary authority. Ongoing policy supervision is an
explicit next command, not a hidden trading or agent activation. See
docs/distribution.md for platform qualification and stopped-only upgrade limits.
New native inputs are separately inventoried with `distribution-freeze-native`,
binding binary digests, architecture headers and pinned Node/SRT versions.
Freezing inputs neither grants authority nor supplies native qualification.
Published first-release delivery has a separate `registry/qualification.json`:
actual npm/pnpm installs and frozen-baseline upgrade/rollback run in isolated
local rigs against captured exact registry bytes. This preserves the accepted
native receipt; source/input/log bindings are checked independently. No hosted
source, credentials, broker orders or strategy activation enter this runner.
Native qualification is a separate Rust command that runs the installed
artifact against isolated local state and a controlled broker, binds the
tested npm archives and test-harness revision, and emits a receipt only on
passing target-local processes. The five-target aggregate verifier, not one
native receipt, decides candidate and registry readiness.

Native Windows distribution and local owner/setup storage share the Core-owned
`platform/windows_private.rs` adapter. It validates owner-only DACLs and rejects
reparse points using the actual file handles, protects new state before writing
secrets, and publishes synced files through write-through atomic moves. It does
not depend on private Warden source. Warden independently implements the same
external authority-storage contract; native qualification remains required.

Agents and deterministic evaluation use the same versioned strategy, authority,
risk, journal and side-effect machinery. Agent submission does not require a
scheduler tick. Idempotency, durable intent, fencing and observation-only recovery
are mandatory at the broker boundary. No actual broker execution is permitted
during qualification. A saved Live configuration is not activation or authority.

Execution configuration `orchestrator` distinguishes deterministic evaluation
from `external_agent` evaluation. Deployment `executor` separately identifies who
owns the agent process: `supervised` (the legacy default) or `external_client`.
The local supervisor never schedules or launches external-client deployments.
They need no model prompt or workspace; creation is not session attachment or
trading authorization. Executor ownership is part of the immutable deployment
binding, while existing supervised binding digests remain compatible. Every
scoped MCP invocation checks the actual current lease and fence, not just the
cached active-run expiry. The stdio transport owns external sessions through
`tradeassembly.agent.session.attach` and `.detach`: capabilities stay in server
memory, leases renew while idle, and EOF quarantines unresolved session outcomes.
Attachment requires authenticated config/activation ownership and, for Live, an
existing exact owner-issued delegated mandate. Tool discovery narrows after
attachment. This implementation is not yet qualified as a full customer trading
journey through Warden and the controlled broker; release evidence remains required.

The production local adapter installs `LocalBrokerSubmissionBoundary` with the
same storage, credentials, registry, leases, clock and finance authority used by
the runtime. Without valid installation-owner state it leaves that path unavailable
while preserving inspection/research. Broker-path availability is a fail-closed
port check, not an order permit: the external host and enforcing boundary must be
wired, and Warden must be reachable at the required version. Custom adapters default
to unavailable. Every order still requires current bindings, risk evidence and its
own C5 authorization. Live approval and evidence-policy readiness are independent.

Studio consumes a qualified pinned Core revision; it owns its frontend and
presentation checks. Product owns hosted Relay, enrollment, billing, deployment
and integrated release evidence. Core does not depend on Product source or
require hosted enrollment for local execution.

The optional `tradeassembly-reach-node` is an outbound-only HTTPS client for
Relay's versioned Reach contract. It binds tenant, workspace, paired node,
actor, command digest, deployment binding digest, authority and expiry before
calling the existing local MCP lifecycle. Local Warden policy remains final.
Remote activation starts an existing local agent deployment without a scheduler
tick; remote stop only changes its desired state and never claims cancellation
or liquidation. Interrupted delivery and acknowledgements replay through the
same local idempotency boundary.

See docs/rust-rewrite-harness.md for owning gates and docs/test-ownership.md for
the extraction test split. Neither the source archive smoke nor an isolated build
proves provider integration, Relay deployment, or the full customer journey.

## Local CI setup phase

`just setup` prepares locked dependencies; `just check` runs developer feedback;
`just build` produces local native binaries; `just verify` preserves the complete
clean-source gate. None requires Codex or a CI-provider run. The coordinator's
local CI manifest owns separate integration tests and records gaps; no deployment
or product repair is authorized by a passing local baseline.
