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

macOS releases are ad-hoc signed, not Apple notarized. Other platforms require
their documented sandbox prerequisites. Never disable OS security globally to
run a prerelease. Windows sandbox support is alpha and needs one-time elevated
SRT setup. Platform support is not promised until its native qualification passes.

No release is available merely because this source README exists. Use the exact
published prerelease version linked from the release announcement. The `beta`
tag is not a stable production channel.
