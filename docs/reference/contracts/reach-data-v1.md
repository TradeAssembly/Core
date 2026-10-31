# Reach data-command bindings V1

The separate `tradeassembly.relay.reach-data-command/v1` envelope is a closed,
bounded data-command wire contract. It does not extend old deployment-based
Reach V1 or paper/backtest runner V1, authenticate a caller or grant a permission.
The remote server/consumer transport must be qualified before advertising create.

Fields are schema, node(existing ReachNodeScope), requestId, idempotencyKey,
issuedAtMs, expiresAtMs, sourceBindingSha256, resource and command. The original
nested command ID/key must equal the outer request ID/key and workspace must match
the enrolled node. Maximum encoded bytes294912 and command lifetime300000ms.
Unknown fields/versions, invalid node/digests, unsafe numbers and live/broker
commands refuse. Parsing normalizes supported integral numeric representations;
canonical identity rejects integers outside the exact interoperable range.

`resource` is `tradeassembly.core-data-mutation-binding.v1`, obtained from Core's
read-only `data_binding` query with the exact nested data-runner command. It binds
actual Core revision, command SHA, scope/graph/action/mode, current domain digest,
configured Warden policy digest and nullable owner binding. Preparation resolves
actual current compiled content; evaluation resolves the referenced current plan;
screening binds its pinned calculation, input receipts and policy; calendars bind
the declared trigger and admitted calendar receipts. Digests are lowercase hex
without a prefix; Core revision is40hex, SHA256 fields64hex. No raw credential or
provider response is returned. Unsupported policy identity refuses, never defaults.

A nullable owner is honest metadata for a trusted local process with no bound
principal. Reach-data validation requires a non-null owner binding matching the
independently authenticated Core service. executionAuthorityGranted is always false.
The envelope's resource must equal Core's independently resolved current binding,
not a caller-supplied replacement. Finite action names derive from owning C3
operations and must match the nested operation. A changed source, principal,
policy, resource, mode or expiry refuses further effects.

`validate_bindings` checks current enrolled node, registered source fingerprint,
Core-resolved resource and trusted time. It is deliberately not an authorization
function. The consuming transport must independently recheck authenticated human
mutation permission, active enrolled installation and existing node signature/nonce;
Core must retain verified session, ownership and real Warden C3 authorization.
A results/journal source link or authorityRef is never promoted to write access.
No new key, identity, role or enrollment is created by the wire implementation.

The intended transport reuses existing Reach's authenticated human/active-node
command channel with a separately domain-bound data protocol. A pre-activation
data command must never invent deployment or activation IDs to fit old Reach.
The remote transport, current selected-profile binding and signed transition
consumer are not implemented by this wire contract. They remain explicit release
gates. Existing local Accepted/Uncertain/Completed/Failed data-runner history is not
proof of remote authorization, scheduler execution, brokerage or real provider data.
