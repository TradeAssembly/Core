# Extracted Core architecture

Core is a portable Rust workspace. CLI, HTTP, GraphQL and MCP share the runtime
service/port boundary. identity-sdk, plugin-sdk and sightline-sidecar are internal
workspace crates. Brokers and Warden remain explicit external executable or
package contracts, not private sibling-source dependencies.

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
cached active-run expiry. The external-client attach/reconnect transport remains
under implementation; this distinction alone does not make external trading ready.

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
