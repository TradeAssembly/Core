# TradeAssembly prerelease

Install a pinned release using `npx --yes tradeassembly@VERSION install` or
`pnpm dlx tradeassembly@VERSION install`. Node 22 or newer is required. There
are no install lifecycle scripts, compiler requirements, or automatic strategies.

The result gives a permanent executable and an MCP configuration. Keep your
agent's MCP configuration pointed at that executable, not an npm cache. Setup
does not connect a broker, authorize orders, activate a strategy, or start an
agent. The user supplies and authorizes those actions separately.

Updates: stop owned agent/runtime and policy services, close their MCP processes,
then run `npx --yes tradeassembly@NEW_VERSION upgrade`. Persistent state remains
in the installation. Active or incompatible rigs fail closed; the installer
never kills processes or liquidates positions. Use the same `--root` and
`--warden-port` options if you customized them. Use `rollback` for the previous
compatible version while stopped. `status` verifies the installed payload.

The first beta qualifies macOS arm64 only. Windows x64 and GNU Linux x64/arm64
packages are experimental builds whose installation, upgrade, and runtime
acceptance is deferred; the launcher warns when one runs. Intel macOS is not
distributed in this beta. macOS releases are ad-hoc signed, not Apple
notarized. Never disable OS security globally to run a prerelease. Windows
sandbox support is alpha and needs one-time elevated SRT setup. A missing
runtime security prerequisite fails closed even on an experimental platform.

No release is available merely because this source README exists. Use the exact
published prerelease version linked from the release announcement. The `beta`
tag is not a stable production channel.

Customer beta packages must bind a production connection profile to their
release manifest by digest. A staging-profile package is rejected by the
first-release verifier even if its native binaries pass. The profile contains
public endpoints and client identifiers, never credentials. Local-only Core
bundles may omit a hosted profile; they are not the customer Relay release.
