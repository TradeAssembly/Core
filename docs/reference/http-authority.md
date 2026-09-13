# Core HTTP authority

Account-free local owners use the CLI or local MCP transport. Starting `api`
does not grant every HTTP caller the installation owner's identity.

Core HTTP is a trusted application transport. Except GET `/health`, GET `/ready`
and standard GraphQL schema introspection, requests require the configured
application bearer credential and an identity assertion verified by IdentityPort.
Unknown routes return404. No wildcard cross-origin browser access is enabled.

GraphQL retains its existing contract: `Authorization: Bearer <credential>` and
`variables._studioSession` in the JSON request. REST uses the same bearer plus
`x-tradeassembly-session` containing the same JSON TrustedStudioSession assertion
(maximum8192 characters). It is a header so authenticated GETs need no body.
The assertion alone is not authentication. Never expose the application bearer
to browser JavaScript, logs, agent prompts or untrusted remote MCP callers.

The service derives actor and tenant scope from the verified identity; body
actor fields cannot select another owner. Missing/invalid transport credentials
or session assertions return401 before domain dispatch. Owner object isolation,
Warden policy, idempotency and durable effect controls still apply afterward.

Authenticated GET `/workspace/shell` is the navigation metadata contract. It
returns only `id`, `name`, `mode`, `strategies`, `providers`,
`credentialStatus`, and `legalBoundary`; it does not load journal, research,
backtest, order, execution, risk, or scheduler state.

Legacy unauthenticated REST helpers are not an authorization mechanism. Consumers
must use the authenticated transport or the local CLI/MCP path; do not add a
shared owner fallback, bypass token or implicit hosted sign-in requirement to
account-free local operation. This transport is not a claim of physical human
presence, malicious same-user process containment, or a public hosted API.
