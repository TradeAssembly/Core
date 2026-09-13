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

Studio consumes a qualified pinned Core revision; it owns its frontend and
presentation checks. Product owns hosted Relay, enrollment, billing, deployment
and integrated release evidence. Core does not depend on Product source or
require hosted enrollment for local execution.

See docs/rust-rewrite-harness.md for owning gates and docs/test-ownership.md for
the extraction test split. Neither the source archive smoke nor an isolated build
proves provider integration, Relay deployment, or the full customer journey.
